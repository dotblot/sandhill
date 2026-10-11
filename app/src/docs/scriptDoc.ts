// An R or Python script: an editor for the file, and a console the window
// shows in its Console pane. ⌘↵ runs the selection, or the statement at the
// cursor and moves on to the next one; ⇧⌘↵ runs the whole file. Runs are
// journaled under the file, so the console comes back when the file or the
// app opens again, with any output that arrived in between. Scripts run in
// the project's folder, as in RStudio, so they share sessions with each
// other wherever they are.

import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import type { FileText, FileWritten } from "../backend/types";
import { baseExtensions, highlightedLines, languageFor } from "../editor/setup";
import { statements } from "../model/statements";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { CellOutput } from "./cellOutput";
import { Doc } from "./doc";

/**
 * The console keeps this many runs. Runs are journaled under the cell ids
 * console-0 to console-99 in turn, so the daemon hands back at most this
 * many when the file opens.
 */
const CONSOLE_SLOTS = 100;
const SLOT = /^console-(\d+)$/;
/** Code longer than this is echoed in the console as its first lines. */
const ECHO_LINES = 6;
/** Recorded in the document's shown-outputs list: when the console was last cleared. */
const CLEARED = "cleared:";

export function scriptLanguage(path: string): "r" | "python" | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  return ext === "r" ? "r" : ext === "py" ? "python" : undefined;
}

interface Entry {
  slot: string;
  root: HTMLElement;
  output: CellOutput;
}

export class ScriptDoc extends Doc {
  readonly kind = "script";
  readonly language: "r" | "python";
  private view?: EditorView;
  /** The console's runs, for the window's Console pane to show. */
  readonly consoleElement: HTMLElement;
  private readonly list: HTMLElement;
  private entries: Entry[] = [];
  private nextSlot = 0;
  /** Whether the console keeps scrolling to the newest output. */
  private follow = true;

  static async open(ws: Workspace, path: string, shownOutputs: string[] = []): Promise<ScriptDoc> {
    const file = await ws.call<FileText>("read_file", { path });
    const doc = new ScriptDoc(ws, file.path, file.text, shownOutputs);
    doc.mtime = file.mtime_ms;
    await doc.loadConsole();
    return doc;
  }

  private constructor(
    ws: Workspace,
    path: string,
    private initialText: string,
    shownOutputs: string[],
  ) {
    super(ws, path, undefined, shownOutputs, ws.project);
    this.language = scriptLanguage(path) ?? "r";
    this.list = el("div", "console-list");
    this.list.addEventListener("scroll", () => {
      this.follow = this.list.scrollTop + this.list.clientHeight >= this.list.scrollHeight - 24;
    });
    this.consoleElement = this.list;
  }

  languages(): string[] {
    return [this.language];
  }

  text(): string {
    return this.view?.state.doc.toString() ?? this.initialText;
  }

  mount(parent: HTMLElement) {
    const runKeys = Prec.highest(
      keymap.of([
        { key: "Mod-Enter", run: () => this.runCurrent() },
        { key: "Mod-Shift-Enter", run: () => (void this.runAll(), true) },
        { key: "Mod-Shift-s", run: () => (void this.runAll(), true) },
      ]),
    );
    this.view = new EditorView({
      parent,
      state: EditorState.create({
        doc: this.initialText,
        extensions: [
          runKeys,
          languageFor(this.language) ?? [],
          ...baseExtensions(),
          this.trackCursor(),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) this.setDirty(true);
          }),
        ],
      }),
    });
    this.consoleShown();
  }

  /** Called when the Console pane shows this script's console. */
  consoleShown() {
    this.follow = true;
    this.scrollToEnd();
  }

  focus() {
    this.view?.focus();
  }

  dispose() {
    for (const entry of this.entries) entry.output.dispose();
    this.view?.destroy();
  }

  async save(force = false) {
    const written = await this.ws.call<FileWritten>("write_file", {
      path: this.path,
      text: this.text(),
      expected_mtime_ms: force ? undefined : this.mtime,
    });
    this.mtime = written.mtime_ms;
    this.setDirty(false);
  }

  setAllOutputs(visible: boolean) {
    for (const entry of this.entries) if (entry.output.info) entry.output.setVisible(visible);
  }

  /** Runs the whole file. */
  async runAll() {
    const code = this.text();
    if (code.trim()) await this.run(code);
  }

  /**
   * Runs the selection, or the statement at the cursor and moves the cursor
   * to the next statement, as RStudio does.
   */
  runCurrent(): boolean {
    const view = this.view;
    if (!view) return false;
    const { state } = view;
    const { main } = state.selection;
    if (!main.empty) {
      const code = state.sliceDoc(main.from, main.to);
      if (code.trim()) void this.run(code);
      return true;
    }
    const line = state.doc.lineAt(main.head).number;
    const all = statements(state.doc.toString(), this.language);
    const here = all.find((s) => s.from <= line && line <= s.to);
    if (here) void this.run(state.sliceDoc(state.doc.line(here.from).from, state.doc.line(here.to).to));
    const after = here?.to ?? line;
    const next = all.find((s) => s.from > after);
    const target = next
      ? state.doc.line(next.from).from
      : after < state.doc.lines
        ? state.doc.line(after + 1).from
        : state.doc.length;
    view.dispatch({ selection: { anchor: target }, scrollIntoView: true });
    return true;
  }

  private async run(code: string) {
    const slot = `console-${this.nextSlot}`;
    this.nextSlot = (this.nextSlot + 1) % CONSOLE_SLOTS;
    const entry = this.addEntry(slot, code);
    entry.output.showSaved({ phase: "queued", code, outputs: [] });
    this.follow = true;
    this.scrollToEnd();
    try {
      const run = await this.execute(this.language, code, slot);
      entry.output.bindRun(this.ws.runs, run);
    } catch (err) {
      entry.output.showSaved({
        phase: "done",
        status: "error",
        code,
        outputs: [{ output_type: "error", ename: "Could not run", evalue: String(err), traceback: [] }],
      });
    }
  }

  /** The console as the journal has it: earlier runs, and anything still running. */
  private async loadConsole() {
    const snapshot = await this.ws.fileOutputs(this.path);
    const cleared = this.clearedAt();
    const runs = snapshot.cells.filter((c) => SLOT.test(c.cell_id)).sort((a, b) => a.started_ms - b.started_ms);
    for (const cell of runs) {
      if (cell.started_ms > cleared) this.addEntry(cell.cell_id, cell.code).output.bindRun(this.ws.runs, cell.run);
    }
    const last = runs[runs.length - 1];
    if (last) this.nextSlot = (Number(SLOT.exec(last.cell_id)![1]) + 1) % CONSOLE_SLOTS;
    this.adoptSessions(snapshot.cells.map((c) => ({ ...c, language: normalizeLanguage(c.language) })));
  }

  private addEntry(slot: string, code: string): Entry {
    // A slot holds one run; its earlier run leaves the console.
    const old = this.entries.find((e) => e.slot === slot);
    if (old) {
      old.output.dispose();
      old.root.remove();
      this.entries = this.entries.filter((e) => e !== old);
    }
    const root = el("div", "console-entry");
    const echo = el("div", "console-code");
    const lines = code.replace(/\s+$/, "").split("\n");
    echo.append(...highlightedLines(lines.slice(0, ECHO_LINES).join("\n"), languageFor(this.language)));
    if (lines.length > ECHO_LINES) {
      const more = el("div", "console-more");
      more.textContent = `… ${lines.length - ECHO_LINES} more lines`;
      echo.append(more);
    }
    const output = new CellOutput(
      this.renderContext(() => this.keepFollowing()),
      () => this.keepFollowing(),
    );
    root.append(echo, output.element);
    this.list.append(root);
    const entry = { slot, root, output };
    this.entries.push(entry);
    return entry;
  }

  clearConsole() {
    for (const entry of this.entries) {
      entry.output.dispose();
      entry.root.remove();
    }
    this.entries = [];
    // Kept with the project's other output choices, so reopening the file
    // doesn't bring the cleared runs back.
    for (const id of this.shownOutputs()) if (id.startsWith(CLEARED)) this.outputToggled(id, false);
    this.outputToggled(`${CLEARED}${Date.now()}`, true);
  }

  private clearedAt(): number {
    const id = this.shownOutputs().find((s) => s.startsWith(CLEARED));
    return id ? Number(id.slice(CLEARED.length)) || 0 : 0;
  }

  private keepFollowing() {
    if (this.follow) this.scrollToEnd();
  }

  private scrollToEnd() {
    requestAnimationFrame(() => {
      this.list.scrollTop = this.list.scrollHeight;
    });
  }
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  return node;
}
