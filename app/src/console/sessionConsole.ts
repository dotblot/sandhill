// A session's Console: everything that ran in one R or Python session,
// wherever it came from (typed here, a script's ⌘↵, an agent, a notebook
// cell), and a prompt to type more. The daemon journals every run, so the
// Console comes back with the output that arrived in between when the app
// reconnects or opens again.

import { insertNewlineAndIndent } from "@codemirror/commands";
import { EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap, placeholder } from "@codemirror/view";
import type { SessionRuns } from "../backend/types";
import { CellOutput } from "../docs/cellOutput";
import { baseExtensions, highlightedLines, languageFor } from "../editor/setup";
import { InputHistory, isComplete } from "../model/consoleInput";
import type { RunState } from "../model/runs";
import type { Workspace } from "../model/workspace";
import type { RenderContext } from "../render/outputs";

/** How many earlier runs the Console brings back. */
const CONSOLE_RUNS = 200;
/** Code longer than this is echoed as its first lines. */
const ECHO_LINES = 6;
/** A notebook cell's code is echoed shorter: its output stays under the cell. */
const CELL_ECHO_LINES = 2;
/** When each session's Console was last cleared, in this browser. */
const CLEARED_KEY = "sandhill.consoleCleared";

export interface ConsoleHost {
  /** Opens a document, from a run's file name. */
  openFile(path: string): void;
  /** A file as shown: its path in the project. */
  shortName(path: string): string;
  onError(message: string): void;
}

interface Entry {
  run: string;
  startedMs: number;
  root: HTMLElement;
  output: CellOutput;
}

/** Scripts' runs show their output in the Console; notebooks keep theirs under the cell. */
export function isScript(path: string): boolean {
  return /\.(r|py)$/i.test(path);
}

export class SessionConsole {
  readonly element: HTMLElement;
  private list: HTMLElement;
  private header: HTMLElement;
  private prompt: HTMLElement;
  private view: EditorView;
  private entries: Entry[] = [];
  private history = new InputHistory();
  /** Whether the list keeps scrolling to the newest output. */
  private follow = true;
  private clearedAt: number;
  private unwatch: () => void;
  private ended = false;
  readonly ready: Promise<void>;

  constructor(
    private ws: Workspace,
    readonly session: string,
    readonly language: string,
    private host: ConsoleHost,
  ) {
    this.clearedAt = clearedTimes()[session] ?? 0;
    this.element = el("div", "session-console");
    this.list = el("div", "console-list");
    this.header = el("div", "console-head");
    this.prompt = el("div", "console-prompt");
    this.prompt.dataset.prompt = this.python ? ">>>" : ">";
    this.list.append(this.header, this.prompt);
    this.element.append(this.list);
    this.list.addEventListener("scroll", () => {
      this.follow = this.list.scrollTop + this.list.clientHeight >= this.list.scrollHeight - 24;
    });
    // A click on the Console that selects nothing goes to the prompt.
    this.list.addEventListener("mouseup", (e) => {
      const target = e.target as HTMLElement;
      if (window.getSelection()?.toString() || target.closest("a, button, input, .cm-editor, .output-area")) return;
      this.focus();
    });
    this.view = new EditorView({
      parent: this.prompt,
      state: EditorState.create({
        extensions: [
          Prec.highest(
            keymap.of([
              { key: "Enter", run: () => this.enter() },
              { key: "Shift-Enter", run: insertNewlineAndIndent },
              { key: "ArrowUp", run: () => this.recall(-1) },
              { key: "ArrowDown", run: () => this.recall(1) },
              { key: "Ctrl-c", run: (view) => view.state.selection.main.empty && (this.interrupt(), true) },
              { key: "Ctrl-l", run: () => (this.clear(), true) },
              { key: "Escape", run: () => (this.setInput(""), true) },
            ]),
          ),
          languageFor(language) ?? [],
          ...baseExtensions({ lineNumbers: false, wrap: true }),
          placeholder(this.python ? "Type Python code; Enter runs it" : "Type R code; Enter runs it"),
        ],
      }),
    });
    this.unwatch = ws.runs.watchAll((run) => this.onRun(run));
    this.ready = this.load().catch((err) => host.onError(`Could not load the console: ${String(err)}`));
  }

  private get python(): boolean {
    return this.language.toLowerCase().startsWith("python");
  }

  /** The code typed at the prompt, not yet run. */
  get input(): string {
    return this.view.state.doc.toString();
  }

  setInput(code: string) {
    this.view.dispatch({
      changes: { from: 0, to: this.view.state.doc.length, insert: code },
      selection: { anchor: code.length },
    });
  }

  focus() {
    if (!this.ended) this.view.focus();
  }

  /** Called when the Console pane shows this console. */
  shown() {
    this.follow = true;
    this.scrollToEnd();
  }

  /** Runs code here, as if typed at the prompt. */
  async run(code: string) {
    if (!code.trim() || this.ended) return;
    this.history.push(code);
    this.follow = true;
    this.scrollToEnd();
    try {
      await this.ws.execute(this.session, code);
    } catch (err) {
      this.host.onError(`Could not run that: ${String(err)}`);
    }
  }

  interrupt() {
    this.ws.interrupt(this.session).catch((err) => this.host.onError(`Could not interrupt: ${String(err)}`));
  }

  /** Empties the Console; the runs stay in the journal, but don't come back here. */
  clear() {
    for (const entry of this.entries) {
      entry.output.dispose();
      entry.root.remove();
    }
    this.entries = [];
    this.clearedAt = Date.now();
    const times = clearedTimes();
    times[this.session] = this.clearedAt;
    try {
      localStorage.setItem(CLEARED_KEY, JSON.stringify(times));
    } catch {
      // Without storage, the runs come back when the app opens again.
    }
  }

  /** The session ended: what ran stays, and the prompt goes. */
  end() {
    this.ended = true;
    this.element.classList.add("ended");
    this.renderHeader();
  }

  dispose() {
    this.unwatch();
    for (const entry of this.entries) entry.output.dispose();
    this.view.destroy();
  }

  private async load() {
    const snapshot = await this.ws.call<SessionRuns>("session_runs", { session: this.session, limit: CONSOLE_RUNS });
    for (const run of snapshot.runs) this.ws.runs.applySessionRun(run, this.session, snapshot.seq);
    // Runs the store already had (a document opened them) don't announce themselves again.
    for (const run of snapshot.runs) {
      const state = this.ws.runs.get(run.run);
      if (state) this.onRun(state);
    }
    this.history = new InputHistory(snapshot.runs.filter((r) => !r.cell || isScript(r.cell.file)).map((r) => r.code.trim()));
    this.renderHeader();
    await this.ws.subscribe(this.session, snapshot.seq);
  }

  renderHeader() {
    const info = this.ws.sessions().find((s) => s.id === this.session);
    const spec = info && this.ws.kernelspecs().find((k) => k.name === info.kernel);
    const home = this.ws.host()?.home;
    const cwd = info?.cwd && home && info.cwd.startsWith(home) ? `~${info.cwd.slice(home.length)}` : info?.cwd;
    const parts = [
      info?.language_version ? `${this.python ? "Python" : "R"} ${info.language_version}` : this.python ? "Python" : "R",
      spec?.display_name ?? info?.kernel,
      cwd,
      this.ended ? "session ended" : undefined,
    ];
    this.header.textContent = parts.filter(Boolean).join(" · ");
  }

  private onRun(run: RunState) {
    if (run.session !== this.session || run.quiet || run.startedMs <= this.clearedAt) return;
    if (this.entries.some((e) => e.run === run.id)) return;
    this.addEntry(run);
  }

  private addEntry(run: RunState) {
    const fromCell = !!run.cell && !isScript(run.cell.file);
    const root = el("div", "console-entry");
    root.classList.toggle("from-agent", !!run.by);
    root.classList.toggle("from-cell", fromCell);
    const from = this.fromLabel(run);
    if (from) root.append(from);
    const echo = el("div", "console-code");
    echo.dataset.prompt = this.python ? ">>>" : ">";
    let lines = run.code.replace(/\s+$/, "").split("\n");
    // A chunk's options (`#| label: umap`) aren't code worth echoing.
    if (fromCell) while (lines.length > 1 && /^\s*#\|/.test(lines[0])) lines = lines.slice(1);
    const shown = fromCell ? CELL_ECHO_LINES : ECHO_LINES;
    echo.append(...highlightedLines(lines.slice(0, shown).join("\n"), languageFor(this.language)));
    if (lines.length > shown) {
      const more = el("div", "console-more");
      const hidden = lines.length - shown;
      more.textContent = `… ${hidden} more line${hidden === 1 ? "" : "s"}`;
      echo.append(more);
    }
    const near = run.cell?.file;
    const ctx: RenderContext = {
      blobUrl: (hash, mime) => this.ws.backend.blobUrl(hash, mime, near),
      blobText: async (hash) => (await this.ws.call<{ data: string }>("get_blob", { hash, near })).data,
      onResize: () => this.keepFollowing(),
    };
    const output = new CellOutput(ctx, () => this.keepFollowing());
    root.append(echo, output.element);
    const entry: Entry = { run: run.id, startedMs: run.startedMs, root, output };
    // In the order they started, which is how the kernel runs them.
    let at = this.entries.length;
    while (at > 0 && this.entries[at - 1].startedMs > run.startedMs) at--;
    this.list.insertBefore(root, this.entries[at]?.root ?? this.prompt);
    this.entries.splice(at, 0, entry);
    output.bindRun(this.ws.runs, run.id);
    this.keepFollowing();
  }

  /** Where a run came from, when not the prompt: an agent, or a document. */
  private fromLabel(run: RunState): HTMLElement | undefined {
    if (!run.by && !run.cell) return undefined;
    const label = el("div", "console-from");
    if (run.by) {
      const who = el("span", "console-by");
      who.textContent = run.by;
      label.append(who);
    }
    if (run.cell) {
      const file = run.cell.file;
      const link = el("button", "console-file");
      link.textContent = this.host.shortName(file);
      link.title = `Open ${this.host.shortName(file)}${isScript(file) ? "" : "; the output is under its cell"}`;
      link.addEventListener("click", () => this.host.openFile(file));
      if (run.by) label.append(" in ");
      label.append(link);
    }
    return label;
  }

  private enter(): boolean {
    const code = this.input;
    if (!isComplete(code, this.language)) return insertNewlineAndIndent(this.view);
    this.setInput("");
    void this.run(code);
    return true;
  }

  /** ↑ and ↓ go through earlier code from the first and last lines. */
  private recall(direction: -1 | 1): boolean {
    const { state } = this.view;
    const head = state.selection.main.head;
    const line = state.doc.lineAt(head).number;
    if (direction < 0 ? line !== 1 : line !== state.doc.lines) return false;
    const code = direction < 0 ? this.history.older(this.input) : this.history.newer();
    if (code === undefined) return false;
    this.setInput(code);
    return true;
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

function clearedTimes(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(CLEARED_KEY) ?? "{}") ?? {};
  } catch {
    return {};
  }
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  return node;
}
