// A Quarto or R Markdown document: one editor for the whole file, with each
// chunk's outputs shown under it. The file itself is never changed by
// running code; outputs come from the daemon's sidecar and journal.

import { EditorState, Prec, StateEffect, StateField } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, keymap, WidgetType } from "@codemirror/view";
import { type Accessor, createSignal, type Setter } from "solid-js";
import type { FileText, FileWritten, SavedCell, SidecarIndex } from "../backend/types";
import { baseExtensions, quartoLanguage } from "../editor/setup";
import {
  type Chunk,
  chunkCodeChange,
  chunkInsertion,
  frontMatterKernel,
  newChunkLanguage,
  parseChunks,
  proseLines,
  sameCode,
  type TextChange,
} from "../model/qmd";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { CellOutput } from "./cellOutput";
import { type AgentCell, changedOnDisk, Doc, type Place, replaceText, selectionOf } from "./doc";

/** Chunk languages that have kernels. */
const RUNNABLE = new Set(["r", "python"]);

interface Slot {
  /** The cell id the outputs were saved or run under. */
  key: string;
  output: CellOutput;
}

interface Layout {
  chunks: Chunk[];
  slots: (Slot | undefined)[];
  decorations: DecorationSet;
}

const refresh = StateEffect.define<null>();

export class QmdDoc extends Doc {
  readonly kind = "qmd";
  private view?: EditorView;
  private slots = new Map<string, Slot>();
  /** Counts outputs kept aside by `rekey`. */
  private asides = 0;
  private layoutField: StateField<Layout>;
  private langs: Accessor<string[]>;
  private setLangs: Setter<string[]>;

  static async open(ws: Workspace, path: string, shownOutputs: string[] = []): Promise<QmdDoc> {
    const file = await ws.call<FileText>("read_file", { path });
    const doc = new QmdDoc(ws, file.path, file.text, shownOutputs);
    doc.mtime = file.mtime_ms;
    await doc.loadOutputs();
    return doc;
  }

  private constructor(
    ws: Workspace,
    path: string,
    private initialText: string,
    shownOutputs: string[],
  ) {
    super(
      ws,
      path,
      (language) => (language === "python" ? { name: frontMatterKernel(this.text()) } : undefined),
      shownOutputs,
    );
    [this.langs, this.setLangs] = createSignal<string[]>(chunkLanguages(parseChunks(initialText)));
    this.layoutField = this.createLayoutField();
  }

  languages(): string[] {
    return this.langs();
  }

  text(): string {
    return this.view?.state.doc.toString() ?? this.initialText;
  }

  chunks(): Chunk[] {
    return this.view?.state.field(this.layoutField).chunks ?? parseChunks(this.initialText);
  }

  mount(parent: HTMLElement) {
    const runKeys = Prec.highest(
      keymap.of([
        { key: "Mod-Enter", run: () => this.runAtCursor(false) },
        { key: "Mod-Shift-Enter", run: () => this.runAtCursor(false) },
        { key: "Shift-Enter", run: () => this.runAtCursor(true) },
      ]),
    );
    this.view = new EditorView({
      parent,
      state: EditorState.create({
        doc: this.initialText,
        extensions: [
          runKeys,
          quartoLanguage(),
          ...baseExtensions({ wrap: true }),
          this.trackCursor(),
          this.layoutField,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) {
              if (!changedOnDisk(update.transactions)) this.setDirty(true);
              this.setLangs(chunkLanguages(update.state.field(this.layoutField).chunks));
            }
            if (update.docChanged || update.transactions.some((t) => t.effects.some((e) => e.is(refresh)))) {
              this.markStale();
            }
          }),
        ],
      }),
    });
    this.markStale();
  }

  focus() {
    this.view?.focus();
  }

  dispose() {
    for (const slot of this.slots.values()) slot.output.dispose();
    this.view?.destroy();
  }

  async save(force = false) {
    const text = this.text();
    const written = await this.ws.call<FileWritten>("write_file", {
      path: this.path,
      text,
      expected_mtime_ms: force ? undefined : this.mtime,
    });
    this.mtime = written.mtime_ms;
    this.setDirty(false);
  }

  reload(file: FileText): boolean {
    if (this.view) replaceText(this.view, file.text);
    else {
      this.initialText = file.text;
      this.setLangs(chunkLanguages(parseChunks(file.text)));
    }
    this.mtime = file.mtime_ms;
    this.setDirty(false);
    return true;
  }

  setAllOutputs(visible: boolean) {
    for (const slot of this.slots.values()) if (slot.output.info) slot.output.setVisible(visible);
  }

  async runAll() {
    for (const chunk of this.chunks()) {
      if (RUNNABLE.has(chunk.language)) await this.runChunk(chunk);
    }
  }

  async runChunk(chunk: Chunk, code = chunk.code) {
    if (!RUNNABLE.has(chunk.language)) return;
    const slot = this.slotForRun(chunk);
    slot.output.setVisible(true);
    // The slot may have moved to the chunk's current id.
    this.outputToggled(slot.key, true);
    slot.output.showSaved({ phase: "queued", code, outputs: [] });
    try {
      const run = await this.execute(chunk.language, code, chunk.cellId);
      slot.output.bindRun(this.ws.runs, run);
    } catch (err) {
      slot.output.showSaved({
        phase: "done",
        status: "error",
        code,
        outputs: [{ output_type: "error", ename: "Could not run", evalue: String(err), traceback: [] }],
      });
    }
  }

  /** Saved outputs from the sidecar, then anything newer from the journal. */
  private async loadOutputs() {
    const saved = await this.readSidecar();
    for (const [cellId, cell] of Object.entries(saved)) {
      this.slotFor(cellId).output.showSaved(savedInfo(cell));
    }
    const snapshot = await this.ws.fileOutputs(this.path);
    for (const cell of snapshot.cells) {
      const known = saved[cell.cell_id];
      if (!known || (known.run !== cell.run && cell.started_ms >= known.started_ms)) {
        this.slotFor(cell.cell_id).output.bindRun(this.ws.runs, cell.run);
      }
    }
    this.adoptSessions(snapshot.cells.map((c) => ({ ...c, language: normalizeLanguage(c.language) })));
  }

  private async readSidecar(): Promise<Record<string, SavedCell>> {
    try {
      const file = await this.ws.call<FileText>("read_file", {
        path: `${this.dir}/.${this.name}.out/index.json`,
      });
      return (JSON.parse(file.text) as SidecarIndex).cells ?? {};
    } catch {
      return {};
    }
  }

  private slotFor(key: string): Slot {
    let slot = this.slots.get(key);
    if (!slot) {
      const output = new CellOutput(
        this.renderContext(() => this.view?.requestMeasure()),
        () => this.view?.requestMeasure(),
        this.outputShown(key),
        (visible) => this.outputToggled(slot!.key, visible),
      );
      slot = { key, output };
      this.slots.set(key, slot);
    }
    return slot;
  }

  /** The slot a run of `chunk` writes to, moved to the chunk's current id. */
  private slotForRun(chunk: Chunk): Slot {
    const layout = this.view?.state.field(this.layoutField);
    const current = layout?.slots[layout.chunks.findIndex((c) => c.from === chunk.from)];
    let slot = current ?? this.slotFor(chunk.cellId);
    if (slot.key !== chunk.cellId) {
      this.slots.get(chunk.cellId)?.output.dispose();
      this.slots.delete(slot.key);
      slot.key = chunk.cellId;
      this.slots.set(chunk.cellId, slot);
    }
    this.view?.dispatch({ effects: refresh.of(null) });
    return slot;
  }

  private runAtCursor(advance: boolean): boolean {
    const view = this.view;
    if (!view) return false;
    const { main } = view.state.selection;
    const chunks = this.chunks();
    const chunk = chunks.find((c) => main.head >= c.from && main.head <= c.to);
    if (!chunk) return false;
    const selected = main.empty ? undefined : view.state.sliceDoc(main.from, main.to);
    void this.runChunk(chunk, selected?.trim() ? selected : chunk.code);
    if (advance) {
      const next = chunks.find((c) => c.from > chunk.to);
      const target = next ? Math.min(next.codeFrom, view.state.doc.length) : chunk.to;
      view.dispatch({ selection: { anchor: target }, scrollIntoView: true });
    }
    return true;
  }

  private markStale() {
    const layout = this.view?.state.field(this.layoutField);
    if (!layout) return;
    layout.chunks.forEach((chunk, i) => {
      const info = layout.slots[i]?.output.info;
      if (info && info.phase === "done") layout.slots[i]!.output.setStale(!sameCode(info.code, chunk.code));
      else layout.slots[i]?.output.setStale(false);
    });
  }

  /** Matches chunks to saved outputs, following unlabeled chunks that moved. */
  private assign(chunks: Chunk[]): (Slot | undefined)[] {
    const result: (Slot | undefined)[] = chunks.map(() => undefined);
    const taken = new Set<Slot>();
    const codeOf = (slot: Slot) => slot.output.info?.code ?? "";
    const take = (i: number, slot: Slot | undefined) => {
      if (slot && !taken.has(slot) && !result[i]) {
        result[i] = slot;
        taken.add(slot);
      }
    };
    chunks.forEach((c, i) => {
      const slot = this.slots.get(c.cellId);
      if (slot && (c.label || sameCode(codeOf(slot), c.code))) take(i, slot);
    });
    chunks.forEach((c, i) => {
      if (result[i] || c.label) return;
      for (const slot of this.slots.values()) {
        if (!taken.has(slot) && /^chunk-\d+(#\d+)?$/.test(slot.key) && sameCode(codeOf(slot), c.code)) {
          take(i, slot);
          break;
        }
      }
    });
    chunks.forEach((c, i) => take(i, this.slots.get(c.cellId)));
    return result;
  }

  /**
   * Files each shown output under its chunk's current id. Unlabeled chunks are
   * numbered in order, so a chunk added above others gives them new ids; their
   * outputs follow them, and the new chunk's id must not find one of theirs.
   */
  private rekey(chunks: Chunk[], slots: (Slot | undefined)[]) {
    const moved = slots.flatMap((slot, i) => (slot && slot.key !== chunks[i]!.cellId ? [{ slot, id: chunks[i]!.cellId }] : []));
    for (const { slot } of moved) if (this.slots.get(slot.key) === slot) this.slots.delete(slot.key);
    for (const { slot, id } of moved) {
      // Whatever had this id is shown nowhere now; it is kept aside, so its
      // chunk finds it again by its code (after an undo, say).
      const old = this.slots.get(id);
      if (old && old !== slot && !slots.includes(old)) {
        old.key = `${id}#${++this.asides}`;
        this.slots.set(old.key, old);
      }
      slot.key = id;
      this.slots.set(id, slot);
    }
  }

  private createLayoutField(): StateField<Layout> {
    const build = (state: EditorState): Layout => {
      const text = state.doc.toString();
      const chunks = parseChunks(text);
      const slots = this.assign(chunks);
      this.rekey(chunks, slots);
      const ranges = [];
      for (const n of proseLines(text)) {
        if (n <= state.doc.lines) ranges.push(Decoration.line({ class: "cm-prose" }).range(state.doc.line(n).from));
      }
      for (const [i, chunk] of chunks.entries()) {
        const startLine = state.doc.line(chunk.openLine);
        const endLine = state.doc.line(chunk.closeLine);
        for (let n = chunk.openLine; n <= chunk.closeLine; n++) {
          ranges.push(Decoration.line({ class: "cm-chunk" }).range(state.doc.line(n).from));
        }
        if (RUNNABLE.has(chunk.language)) {
          ranges.push(
            Decoration.widget({ widget: new RunButton(this, chunk.cellId), side: 1 }).range(startLine.to),
          );
        }
        const slot = slots[i];
        if (slot) {
          ranges.push(Decoration.widget({ widget: new OutputBlock(slot), block: true, side: 1 }).range(endLine.to));
        }
      }
      return { chunks, slots, decorations: Decoration.set(ranges, true) };
    };
    return StateField.define<Layout>({
      create: build,
      update: (layout, tr) =>
        tr.docChanged || tr.effects.some((e) => e.is(refresh)) ? build(tr.state) : layout,
      provide: (field) => EditorView.decorations.from(field, (layout) => layout.decorations),
    });
  }

  place(): Place {
    const view = this.view;
    if (!view) return {};
    const head = view.state.selection.main.head;
    const chunk = this.chunks().find((c) => head >= c.from && head <= c.to);
    return { cell: chunk?.cellId, line: view.state.doc.lineAt(head).number, selection: selectionOf(view) };
  }

  addCell(code: string, language?: string, after?: string): AgentCell {
    const text = this.text();
    const chunks = this.chunks();
    const anchor = after === undefined ? chunks[chunks.length - 1] : this.chunkFor(after);
    const lang = newChunkLanguage(text, chunks, anchor, language && normalizeLanguage(language), this.path);
    if (!lang) throw new Error("say which `language` the cell is in (r or python)");
    if (!RUNNABLE.has(lang)) throw new Error(`cells can be r or python, not ${lang}`);
    const change = chunkInsertion(text, code, lang, anchor);
    const chunk = this.change(change, change.start);
    // The cursor goes into the new chunk, as if the user had typed it there.
    this.view?.dispatch({ selection: { anchor: chunk.codeFrom }, scrollIntoView: true });
    return { cellId: chunk.cellId, language: lang };
  }

  editCell(cellId: string, code: string): AgentCell {
    const chunk = this.chunkFor(cellId);
    const edited = this.change(chunkCodeChange(this.text(), chunk, code), chunk.from);
    return { cellId: edited.cellId, language: normalizeLanguage(edited.language) };
  }

  async runAgentCell(cell: AgentCell) {
    const chunk = this.chunks().find((c) => c.cellId === cell.cellId);
    if (chunk) await this.runChunk(chunk);
  }

  private chunkFor(cellId: string): Chunk {
    const chunks = this.chunks();
    const chunk = chunks.find((c) => c.cellId === cellId);
    if (chunk) return chunk;
    const ids = chunks.map((c) => c.cellId).join(", ");
    throw new Error(ids ? `${this.name} has no cell ${cellId}; its cells are ${ids}` : `${this.name} has no code chunks`);
  }

  /** Applies an agent's change like typing (one ⌘Z undoes it) and returns the chunk starting at `start`. */
  private change(change: TextChange, start: number): Chunk {
    if (this.view) {
      this.view.dispatch({ changes: change, userEvent: "input.agent" });
    } else {
      this.initialText = this.initialText.slice(0, change.from) + change.insert + this.initialText.slice(change.to);
      this.setLangs(chunkLanguages(parseChunks(this.initialText)));
      this.setDirty(true);
    }
    const chunk = this.chunks().find((c) => c.from === start);
    if (!chunk) throw new Error("the changed chunk could not be found again");
    return chunk;
  }

  /** Called by a chunk's run button. */
  runCell(cellId: string) {
    const chunk = this.chunks().find((c) => c.cellId === cellId);
    if (chunk) void this.runChunk(chunk);
  }
}

class OutputBlock extends WidgetType {
  constructor(readonly slot: Slot) {
    super();
  }
  eq(other: OutputBlock) {
    return other.slot === this.slot;
  }
  toDOM() {
    // A fresh wrapper each time: CodeMirror removes the old wrapper, and the
    // long-lived output element moves into the new one.
    const wrap = document.createElement("div");
    wrap.className = "cm-output-block";
    wrap.append(this.slot.output.element);
    return wrap;
  }
  ignoreEvent() {
    return true;
  }
}

class RunButton extends WidgetType {
  constructor(
    readonly doc: QmdDoc,
    readonly cellId: string,
  ) {
    super();
  }
  eq(other: RunButton) {
    return other.cellId === this.cellId && other.doc === this.doc;
  }
  toDOM() {
    const button = document.createElement("button");
    button.className = "cm-run-chunk";
    button.title = "Run chunk (⌘↵)";
    button.textContent = "▶ Run";
    button.addEventListener("mousedown", (e) => e.preventDefault());
    button.addEventListener("click", () => this.doc.runCell(this.cellId));
    return button;
  }
  ignoreEvent() {
    return true;
  }
}

function savedInfo(cell: SavedCell) {
  return {
    phase: "done" as const,
    status: cell.status,
    executionCount: cell.execution_count,
    startedMs: cell.started_ms,
    finishedMs: cell.finished_ms,
    code: cell.code,
    outputs: cell.outputs,
  };
}

function chunkLanguages(chunks: Chunk[]): string[] {
  return [...new Set(chunks.map((c) => normalizeLanguage(c.language)).filter((l) => RUNNABLE.has(l)))];
}
