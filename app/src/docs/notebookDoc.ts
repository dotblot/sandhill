// A Jupyter notebook: a list of cells, each with its own small editor and
// outputs. A cell gets an editor only once it is clicked or focused, and
// gives it up again far from view; until then it shows highlighted code.
// Making editors as cells scroll by cost the web view about 30 MB in a
// long notebook, mostly CodeMirror code that WebKit's JIT compiled.
//
// Outputs Outputs are saved into the .ipynb itself; when a notebook is
// reopened, runs that finished after it was last saved are merged in from
// the daemon's journal.

import { markdown } from "@codemirror/lang-markdown";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import DOMPurify from "dompurify";
import { marked } from "marked";
import { createSignal, type Setter } from "solid-js";
import { type Blob, type FileText, type FileWritten, isBlobRef, type Output } from "../backend/types";
import { baseExtensions, highlightedLines, languageFor } from "../editor/setup";
import {
  type CellType,
  type NbCell,
  newCell,
  type Notebook,
  notebookLanguage,
  parseNotebook,
  serializeNotebook,
} from "../model/ipynb";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { CellOutput, type OutputInfo } from "./cellOutput";
import { type AgentCell, Doc, type Place, selectionOf } from "./doc";
import { followOutput } from "./follow";

interface CellView {
  cell: NbCell;
  root: HTMLElement;
  body: HTMLElement;
  count: HTMLElement;
  editor?: EditorView;
  /** The editor's state while its view is released far from view, with its undo history. */
  parked?: EditorState;
  output?: CellOutput;
  preview?: HTMLElement;
}

export class NotebookDoc extends Doc {
  readonly kind = "ipynb";
  private root?: HTMLElement;
  private views = new Map<string, CellView>();
  private observer?: IntersectionObserver;
  /** Tells outputs when they are within a couple of screens of view. */
  private nearObserver?: IntersectionObserver;
  private language: () => string;
  private setLanguage: Setter<string>;
  /** The cell whose editor last had focus. */
  private current?: string;

  static async open(ws: Workspace, path: string, shownOutputs: string[] = []): Promise<NotebookDoc> {
    const file = await readNotebook(ws, path);
    const doc = new NotebookDoc(ws, file.path, parseNotebook(file.text), shownOutputs);
    doc.mtime = file.mtime_ms;
    await doc.mergeJournal();
    return doc;
  }

  private constructor(
    ws: Workspace,
    path: string,
    private nb: Notebook,
    shownOutputs: string[],
  ) {
    super(ws, path, () => nb.metadata?.kernelspec, shownOutputs);
    [this.language, this.setLanguage] = createSignal(normalizeLanguage(notebookLanguage(nb) ?? "python"));
    for (const cell of nb.cells) this.views.set(cell.id, this.createView(cell));
  }

  languages(): string[] {
    return [this.language()];
  }

  mount(parent: HTMLElement) {
    this.host = parent;
    this.root = document.createElement("div");
    this.root.className = "notebook";
    parent.append(this.root);
    this.observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const view = this.views.get((entry.target as HTMLElement).dataset.id!);
          if (view && !entry.isIntersecting) this.parkEditor(view);
        }
      },
      { rootMargin: "600px" },
    );
    this.nearObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const id = (entry.target.closest(".nb-cell") as HTMLElement | null)?.dataset.id;
          this.views.get(id ?? "")?.output?.setNearView(entry.isIntersecting);
        }
      },
      { root: parent, rootMargin: "200% 0px" },
    );
    this.renderCells();
  }

  focus() {
    const first = this.nb.cells[0];
    if (first) this.focusCell(first.id);
  }

  dispose() {
    this.following?.();
    this.observer?.disconnect();
    this.nearObserver?.disconnect();
    for (const view of this.views.values()) {
      view.editor?.destroy();
      view.output?.dispose();
    }
  }

  /** A notebook changed on disk is opened again, cells and outputs alike. */
  reload(): boolean {
    return false;
  }

  async save(force = false) {
    for (const view of this.views.values()) view.cell.source = this.sourceOf(view);
    const expected_mtime_ms = force ? undefined : this.mtime;
    let written: FileWritten;
    try {
      // Plots stay as blob references; the daemon puts them back as it
      // writes the file, so they never travel through the app.
      written = await this.ws.call<FileWritten>("write_notebook", {
        path: this.path,
        text: serializeNotebook(this.nb),
        expected_mtime_ms,
      });
    } catch (err) {
      if (!String(err).includes("unknown variant")) throw err;
      const cells = await Promise.all(
        this.nb.cells.map(async (cell) => ({ ...cell, outputs: await this.inlineBlobs(cell.outputs) })),
      );
      written = await this.ws.call<FileWritten>("write_file", {
        path: this.path,
        text: serializeNotebook({ ...this.nb, cells }),
        expected_mtime_ms,
      });
    }
    this.mtime = written.mtime_ms;
    this.setDirty(false);
  }

  setAllOutputs(visible: boolean) {
    for (const view of this.views.values()) if (view.output?.info) view.output.setVisible(visible);
  }

  override codeCells() {
    return this.nb.cells.flatMap((cell, i) => (cell.cell_type === "code" ? [{ id: cell.id, name: `cell ${i + 1}` }] : []));
  }

  async runAll() {
    for (const cell of this.nb.cells) {
      if (cell.cell_type === "code") await this.runCell(cell.id);
      else if (cell.cell_type === "markdown") this.showPreview(this.views.get(cell.id)!);
    }
  }

  async runCell(id: string) {
    const view = this.views.get(id);
    if (!view) return;
    if (view.cell.cell_type === "markdown") {
      this.showPreview(view);
      return;
    }
    if (view.cell.cell_type !== "code") return;
    const code = this.sourceOf(view);
    const output = view.output!;
    output.setVisible(true);
    output.showSaved({ phase: "queued", code, outputs: [] });
    try {
      this.recordKernel(await this.kernels.ensure(this.language()));
      const run = await this.execute(this.language(), code, id);
      output.bindRun(this.ws.runs, run);
    } catch (err) {
      output.showSaved({
        phase: "done",
        status: "error",
        code,
        outputs: [{ output_type: "error", ename: "Could not run", evalue: String(err), traceback: [] }],
      });
    }
  }

  place(): Place {
    const view = this.current ? this.views.get(this.current) : undefined;
    if (!view) return {};
    const editor = view.editor;
    if (!editor) return { cell: view.cell.id };
    const head = editor.state.selection.main.head;
    return { cell: view.cell.id, line: editor.state.doc.lineAt(head).number, selection: selectionOf(editor) };
  }

  addCell(code: string, language?: string, after?: string): AgentCell {
    const own = this.language();
    if (language && normalizeLanguage(language) !== own) {
      throw new Error(`${this.name} is a ${own} notebook; it can't hold ${language} code`);
    }
    const index = after === undefined ? this.nb.cells.length - 1 : this.indexOf(after);
    const cell = newCell("code", code.replace(/\n+$/, ""));
    this.nb.cells.splice(index + 1, 0, cell);
    this.views.set(cell.id, this.createView(cell));
    this.renderCells();
    this.setDirty(true);
    // The new cell takes the focus, as if the user had added it.
    this.focusCell(cell.id);
    this.current = cell.id;
    return { cellId: cell.id, language: own };
  }

  editCell(cellId: string, code: string): AgentCell {
    const view = this.views.get(this.nb.cells[this.indexOf(cellId)].id)!;
    if (view.cell.cell_type !== "code") throw new Error(`cell ${cellId} of ${this.name} is not a code cell`);
    const source = code.replace(/\n+$/, "");
    if (view.editor) {
      view.editor.dispatch({ changes: { from: 0, to: view.editor.state.doc.length, insert: source }, userEvent: "input.agent" });
    } else {
      view.cell.source = source;
      view.parked = undefined;
      view.body.querySelector(".nb-placeholder")?.replaceWith(this.placeholder(view));
    }
    this.setDirty(true);
    view.root.scrollIntoView({ block: "nearest" });
    return { cellId, language: this.language() };
  }

  async runAgentCell(cell: AgentCell) {
    await this.runCell(cell.cellId);
    // The user sees the agent's result come in.
    const view = this.views.get(cell.cellId);
    this.following?.();
    if (view?.output && this.host) this.following = followOutput(view.output, this.host, () => view.root.getBoundingClientRect().top);
  }

  private indexOf(cellId: string): number {
    const index = this.nb.cells.findIndex((c) => c.id === cellId);
    if (index < 0) throw new Error(`${this.name} has no cell ${cellId}`);
    return index;
  }

  /** Brings in runs that finished (or are still going) since the notebook was saved. */
  private async mergeJournal() {
    const snapshot = await this.ws.fileOutputs(this.path);
    let merged = false;
    for (const cell of snapshot.cells) {
      const view = this.views.get(cell.cell_id);
      if (!view?.output) continue;
      if (cell.finished_ms === undefined || cell.finished_ms > this.mtime) {
        view.output.bindRun(this.ws.runs, cell.run);
        merged = true;
      }
    }
    this.adoptSessions(snapshot.cells.map((c) => ({ ...c, language: normalizeLanguage(c.language) })));
    // Outputs that arrived while the notebook was closed are written into it
    // right away, as if it had stayed open and saved them.
    if (merged) await this.save().catch((err) => console.warn("could not save merged outputs", err));
  }

  private createView(cell: NbCell): CellView {
    const root = document.createElement("div");
    root.className = `nb-cell nb-${cell.cell_type}`;
    root.dataset.id = cell.id;
    const gutter = document.createElement("div");
    gutter.className = "nb-gutter";
    const count = document.createElement("span");
    count.className = "nb-count";
    gutter.append(count);
    if (cell.cell_type === "code") {
      const run = button("▶", "Run cell (⇧↵)", () => void this.runCell(cell.id));
      run.classList.add("nb-run");
      gutter.append(run);
    }
    const body = document.createElement("div");
    body.className = "nb-body";
    const tools = document.createElement("div");
    tools.className = "nb-tools";
    tools.append(
      button("↑", "Move up", () => this.moveCell(cell.id, -1)),
      button("↓", "Move down", () => this.moveCell(cell.id, 1)),
      button("✕", "Delete cell", () => this.deleteCell(cell.id)),
    );
    const insert = document.createElement("div");
    insert.className = "nb-insert";
    insert.append(
      button("+ Code", "Insert a code cell below", () => this.insertCell(cell.id, "code")),
      button("+ Markdown", "Insert a text cell below", () => this.insertCell(cell.id, "markdown")),
    );
    root.append(gutter, body, tools, insert);
    body.style.containIntrinsicSize = `auto ${estimatedHeight(cell, this.outputShown(cell.id))}px`;

    const view: CellView = { cell, root, body, count };
    if (cell.cell_type === "code") {
      view.output = new CellOutput(
        this.renderContext(() => {}),
        (info) => this.onOutput(view, info),
        this.outputShown(cell.id),
        (visible) => {
          this.outputToggled(cell.id, visible);
          body.style.containIntrinsicSize = `auto ${estimatedHeight(cell, visible)}px`;
        },
        typeof IntersectionObserver !== "undefined",
      );
      if (cell.outputs.length || cell.execution_count !== null) {
        view.output.showSaved({
          phase: "done",
          executionCount: cell.execution_count ?? undefined,
          code: cell.source,
          outputs: cell.outputs,
        });
      }
      body.append(this.placeholder(view), view.output.element);
    } else if (cell.cell_type === "markdown" && cell.source.trim()) {
      this.showPreview(view);
    } else {
      body.append(this.placeholder(view));
    }
    this.updateCount(view);
    return view;
  }

  /** Highlighted code standing in for the editor; a click makes the editor there. */
  private placeholder(view: CellView): HTMLElement {
    const { cell } = view;
    const el = document.createElement("div");
    el.className = "nb-placeholder";
    // Tab reaches it like an editor; a click is handled below, without focus.
    el.tabIndex = 0;
    el.addEventListener("focus", () => this.ensureEditor(view)?.focus());
    el.append(...highlightedLines(cell.source, cell.cell_type === "code" ? languageFor(this.language()) : undefined));
    el.addEventListener("mousedown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      const editor = this.ensureEditor(view);
      if (!editor) return;
      const pos = editor.posAtCoords({ x: e.clientX, y: e.clientY });
      if (pos !== null) editor.dispatch({ selection: { anchor: pos } });
      editor.focus();
    });
    return el;
  }

  private ensureEditor(view: CellView): EditorView | undefined {
    if (view.editor || view.preview) return view.editor;
    const { cell } = view;
    const language = cell.cell_type === "code" ? languageFor(this.language()) : undefined;
    const parked = view.parked;
    view.parked = undefined;
    const editor = new EditorView({
      state: parked ?? EditorState.create({
        doc: cell.source,
        extensions: [
          Prec.highest(
            keymap.of([
              { key: "Shift-Enter", run: () => (this.runAndAdvance(cell.id), true) },
              { key: "Mod-Enter", run: () => (void this.runCell(cell.id), true) },
              { key: "Escape", run: () => (cell.cell_type === "markdown" ? (this.showPreview(view), true) : false) },
            ]),
          ),
          cell.cell_type === "markdown" ? markdown() : language ?? [],
          ...baseExtensions({ lineNumbers: false, wrap: cell.cell_type !== "code" }),
          this.trackCursor(),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) this.setDirty(true);
            if (u.view.hasFocus) this.current = cell.id;
          }),
        ],
      }),
    });
    view.editor = editor;
    view.body.classList.add("editing");
    view.body.querySelector(".nb-placeholder")?.replaceWith(editor.dom);
    if (!editor.dom.isConnected) view.body.prepend(editor.dom);
    return editor;
  }

  /**
   * Releases the editor of a cell far from view, keeping its state so the
   * text, selection and undo history come back with the next editor.
   */
  private parkEditor(view: CellView) {
    const editor = view.editor;
    if (!editor || editor.hasFocus) return;
    view.cell.source = editor.state.doc.toString();
    view.parked = editor.state;
    editor.dom.replaceWith(this.placeholder(view));
    editor.destroy();
    view.editor = undefined;
    view.body.classList.remove("editing");
  }

  private showPreview(view: CellView) {
    if (view.cell.cell_type !== "markdown") return;
    view.cell.source = this.sourceOf(view);
    view.editor?.destroy();
    view.editor = undefined;
    view.body.classList.remove("editing");
    view.parked = undefined;
    const preview = document.createElement("div");
    preview.className = "nb-markdown";
    preview.innerHTML = DOMPurify.sanitize(marked.parse(view.cell.source || "*Empty text cell*", { async: false }) as string);
    preview.addEventListener("dblclick", () => {
      view.preview = undefined;
      preview.replaceWith(this.placeholder(view));
      this.ensureEditor(view)?.focus();
    });
    view.body.replaceChildren(preview);
    view.preview = preview;
  }

  private onOutput(view: CellView, info: OutputInfo) {
    this.updateCount(view, info);
    if (!view.output?.runId) return;
    // Live outputs become part of the notebook.
    view.cell.outputs = info.outputs;
    if (info.phase === "done") view.cell.execution_count = info.executionCount ?? null;
    this.setDirty(true);
  }

  private updateCount(view: CellView, info?: OutputInfo) {
    if (view.cell.cell_type !== "code") return;
    const phase = info?.phase;
    const n = phase === "done" ? info?.executionCount : view.cell.execution_count;
    view.count.textContent = phase === "running" ? "[*]" : phase === "queued" ? "[ ]" : n ? `[${n}]` : "";
    view.root.dataset.phase = phase ?? "";
  }

  private sourceOf(view: CellView): string {
    return view.editor ? view.editor.state.doc.toString() : view.cell.source;
  }

  private runAndAdvance(id: string) {
    void this.runCell(id);
    const index = this.nb.cells.findIndex((c) => c.id === id);
    let next = this.nb.cells[index + 1];
    if (!next) next = this.insertCell(id, "code");
    this.focusCell(next.id);
  }

  private focusCell(id: string) {
    const view = this.views.get(id);
    if (!view) return;
    view.root.scrollIntoView({ block: "nearest" });
    if (!view.preview) this.ensureEditor(view)?.focus();
  }

  private insertCell(afterId: string, type: CellType): NbCell {
    const cell = newCell(type);
    const index = this.nb.cells.findIndex((c) => c.id === afterId);
    this.nb.cells.splice(index + 1, 0, cell);
    this.views.set(cell.id, this.createView(cell));
    this.renderCells();
    this.setDirty(true);
    this.focusCell(cell.id);
    return cell;
  }

  private deleteCell(id: string) {
    const index = this.nb.cells.findIndex((c) => c.id === id);
    if (index < 0 || this.nb.cells.length === 1) return;
    this.nb.cells.splice(index, 1);
    const view = this.views.get(id);
    view?.editor?.destroy();
    view?.output?.dispose();
    view?.root.remove();
    this.views.delete(id);
    this.setDirty(true);
  }

  private moveCell(id: string, delta: number) {
    const index = this.nb.cells.findIndex((c) => c.id === id);
    const target = index + delta;
    if (index < 0 || target < 0 || target >= this.nb.cells.length) return;
    const [cell] = this.nb.cells.splice(index, 1);
    this.nb.cells.splice(target, 0, cell);
    this.renderCells();
    this.setDirty(true);
  }

  private renderCells() {
    if (!this.root) return;
    this.root.replaceChildren(...this.nb.cells.map((c) => this.views.get(c.id)!.root));
    for (const view of this.views.values()) {
      this.observer?.observe(view.root);
      if (view.output) this.nearObserver?.observe(view.root);
    }
  }

  /** Records the session's kernel in the notebook's metadata the first time it runs, as Jupyter does. */
  private recordKernel(session: string) {
    if (this.nb.metadata.kernelspec) return;
    const kernel = this.ws.sessions().find((s) => s.id === session)?.kernel;
    const specs = this.ws.kernelspecs();
    const spec = specs.find((k) => k.name === kernel) ?? this.ws.pick(this.language());
    if (!spec) return;
    this.nb.metadata.kernelspec = { name: spec.name, display_name: spec.display_name, language: spec.language };
    this.setLanguage(normalizeLanguage(spec.language));
    // Highlight the code cells not being edited for the kernel's language.
    for (const view of this.views.values()) {
      if (view.cell.cell_type !== "code" || view.editor) continue;
      view.body.querySelector(".nb-placeholder")?.replaceWith(this.placeholder(view));
    }
  }

  /**
   * Notebooks store values inline, so blob references are replaced with
   * their data. Only for an older daemon without `write_notebook`.
   */
  private async inlineBlobs(outputs: Output[]): Promise<Output[]> {
    return Promise.all(
      outputs.map(async (o) => {
        if (!o.data) return o;
        const entries = await Promise.all(
          Object.entries(o.data as Record<string, unknown>).map(async ([mime, value]) => {
            if (!isBlobRef(value)) return [mime, value];
            const blob = await this.ws.call<Blob>("get_blob", { hash: value.sandhill_blob, near: this.path });
            return [mime, blob.data];
          }),
        );
        return { ...o, data: Object.fromEntries(entries) };
      }),
    );
  }
}

/**
 * The notebook with plots and large tables as blob references, so they load
 * by URL instead of sitting in memory as base64. An older daemon without
 * `read_notebook` sends the file as it is.
 */
async function readNotebook(ws: Workspace, path: string): Promise<FileText> {
  try {
    return await ws.call<FileText>("read_notebook", { path });
  } catch (err) {
    if (!String(err).includes("unknown variant")) throw err;
    return ws.call<FileText>("read_file", { path });
  }
}

/**
 * About how tall a cell's code and outputs are, which they are taken to be
 * until first drawn (cells far from view aren't laid out), so the scroll bar
 * starts out about right.
 */
export function estimatedHeight(cell: NbCell, outputsShown: boolean): number {
  const lines = (text: string) => text.split("\n").length;
  let px = 10 + lines(cell.source) * 17;
  if (cell.cell_type !== "code" || !cell.outputs.length) return px;
  px += 26;
  if (!outputsShown) return px;
  for (const output of cell.outputs) {
    const data: Record<string, unknown> = output.data ?? {};
    const html = data["text/html"];
    if (output.output_type === "stream") px += lines(String(output.text ?? "")) * 16;
    else if (output.output_type === "error") px += lines([output.traceback ?? []].flat().join("\n")) * 16;
    else if (Object.keys(data).some((mime) => mime.startsWith("image/"))) px += 460;
    else if (typeof html === "string") px += Math.min(html.match(/<tr\b/g)?.length ?? 8, 201) * 21;
    else px += 200;
  }
  return px;
}

function button(label: string, title: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.textContent = label;
  b.title = title;
  b.addEventListener("click", onClick);
  return b;
}
