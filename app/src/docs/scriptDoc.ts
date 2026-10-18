// An R or Python script: an editor for the file, whose runs show in the
// Console of the session they run in. ⌘↵ runs the selection, or the
// statement at the cursor and moves on to the next one; ⇧⌘↵ runs the whole
// file. Runs are journaled under the file, so the file finds its session
// again when it opens. Scripts run in the project's folder, as in RStudio,
// so they share sessions with each other wherever they are.

import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import type { FileText, FileWritten } from "../backend/types";
import { notify } from "../ui/dialogs";
import { baseExtensions, languageFor } from "../editor/setup";
import { statements } from "../model/statements";
import { normalizeLanguage, type Workspace } from "../model/workspace";
import { type AgentCell, changedOnDisk, Doc, type Place, replaceText, selectionOf } from "./doc";

/**
 * Runs are journaled under the cell ids console-0 to console-99 in turn, so
 * the daemon hands back at most this many when the file opens.
 */
const CONSOLE_SLOTS = 100;
const SLOT = /^console-(\d+)$/;

export function scriptLanguage(path: string): "r" | "python" | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  return ext === "r" ? "r" : ext === "py" ? "python" : undefined;
}

export class ScriptDoc extends Doc {
  readonly kind = "script";
  readonly language: "r" | "python";
  private view?: EditorView;
  private nextSlot = 0;

  static async open(ws: Workspace, path: string, shownOutputs: string[] = []): Promise<ScriptDoc> {
    const file = await ws.call<FileText>("read_file", { path });
    const doc = new ScriptDoc(ws, file.path, file.text, shownOutputs);
    doc.mtime = file.mtime_ms;
    await doc.loadRuns();
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
            if (update.docChanged && !changedOnDisk(update.transactions)) this.setDirty(true);
          }),
        ],
      }),
    });
  }

  reload(file: FileText): boolean {
    if (this.view) replaceText(this.view, file.text);
    else this.initialText = file.text;
    this.mtime = file.mtime_ms;
    this.setDirty(false);
    return true;
  }

  place(): Place {
    const view = this.view;
    if (!view) return {};
    const head = view.state.selection.main.head;
    return { line: view.state.doc.lineAt(head).number, selection: selectionOf(view) };
  }

  /** An agent's code goes at the end of the script; the id is the console entry it will run as. */
  addCell(code: string, language?: string): AgentCell {
    if (language && normalizeLanguage(language) !== this.language) {
      throw new Error(`${this.name} is a ${this.language} script; it can't hold ${language} code`);
    }
    const text = this.text();
    const nl = text.includes("\r\n") ? "\r\n" : "\n";
    const body = code.replace(/[\r\n]+$/, "").replace(/\r?\n/g, nl);
    const separator = !text.trim() ? "" : text.endsWith("\n") ? nl : `${nl}${nl}`;
    const insert = `${separator}${body}${nl}`;
    if (this.view) {
      const end = this.view.state.doc.length;
      this.view.dispatch({
        changes: { from: end, insert },
        selection: { anchor: end + insert.length },
        scrollIntoView: true,
        userEvent: "input.agent",
      });
    } else {
      this.initialText += insert;
      this.setDirty(true);
    }
    return { cellId: `console-${this.nextSlot}`, language: this.language };
  }

  editCell(): AgentCell {
    throw new Error(`a script has no cells; edit ${this.name} as a file instead`);
  }

  async runAgentCell(_cell: AgentCell, code: string) {
    await this.run(code);
  }

  focus() {
    this.view?.focus();
  }

  dispose() {
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

  /** A script's outputs are in the Console. */
  setAllOutputs() {}

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
    try {
      await this.execute(this.language, code, slot);
    } catch (err) {
      notify(`Could not run ${this.name}: ${String(err)}`);
    }
  }

  /** Finds the session the file's code last ran in, from the journal. */
  private async loadRuns() {
    const snapshot = await this.ws.fileOutputs(this.path);
    const runs = snapshot.cells.filter((c) => SLOT.test(c.cell_id)).sort((a, b) => a.started_ms - b.started_ms);
    const last = runs[runs.length - 1];
    if (last) this.nextSlot = (Number(SLOT.exec(last.cell_id)![1]) + 1) % CONSOLE_SLOTS;
    this.adoptSessions(snapshot.cells.map((c) => ({ ...c, language: normalizeLanguage(c.language) })));
  }
}
