// What the app shell needs from an open document, whatever its format.

import { Annotation, type Extension, Transaction } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";
import type { FileText } from "../backend/types";
import type { KernelWanted, Workspace } from "../model/workspace";
import type { RenderContext } from "../render/outputs";
import { DocKernels } from "./kernels";

export type DocKind = "qmd" | "ipynb" | "script";

export function docKind(path: string): DocKind | undefined {
  const ext = path.split(".").pop()?.toLowerCase();
  if (ext === "qmd" || ext === "rmd") return "qmd";
  if (ext === "ipynb") return "ipynb";
  if (ext === "r" || ext === "py") return "script";
  return undefined;
}

/** Where the user is in a document, for agents (see `set_focus`). */
export interface Place {
  /** The current cell: the chunk with the cursor, or the notebook cell last in focus. */
  cell?: string;
  /** The cursor's line, 1-based (in a notebook, within the cell). */
  line?: number;
  selection?: string;
}

/** A cell an agent added or changed: its id now, and its language. */
export interface AgentCell {
  cellId: string;
  language: string;
}

/** Selections longer than this are cut before they go to agents. */
const MAX_SELECTION = 4000;

/** The selection of an editor, cut to a length agents take. */
export function selectionOf(view: EditorView): string | undefined {
  const { main } = view.state.selection;
  if (main.empty) return undefined;
  return view.state.sliceDoc(main.from, Math.min(main.to, main.from + MAX_SELECTION));
}

/** Marks an editor change that came from the file on disk, not from typing. */
export const fromDisk = Annotation.define<boolean>();

/** Whether an editor update came from the file on disk. */
export function changedOnDisk(transactions: readonly Transaction[]): boolean {
  return transactions.some((t) => t.annotation(fromDisk));
}

/**
 * Replaces an editor's text with `text`, changing only the part that differs,
 * so the cursor, scroll position and folds elsewhere stay put. The change can
 * be undone like any edit.
 */
export function replaceText(view: EditorView, text: string) {
  const old = view.state.doc.toString();
  if (old === text) return;
  const max = Math.min(old.length, text.length);
  let start = 0;
  while (start < max && old.charCodeAt(start) === text.charCodeAt(start)) start++;
  let endOld = old.length;
  let endNew = text.length;
  while (endOld > start && endNew > start && old.charCodeAt(endOld - 1) === text.charCodeAt(endNew - 1)) {
    endOld--;
    endNew--;
  }
  view.dispatch({
    changes: { from: start, to: endOld, insert: text.slice(start, endNew) },
    annotations: [fromDisk.of(true), Transaction.addToHistory.of(true)],
  });
}

export abstract class Doc {
  abstract readonly kind: DocKind;
  readonly name: string;
  readonly dir: string;
  readonly dirty: () => boolean;
  protected setDirty: (dirty: boolean) => void;
  readonly kernels: DocKernels;
  /**
   * Cells whose outputs are shown. Saved outputs start hidden, so opening a
   * document full of plots loads none of them; running a cell shows its
   * output. The project window remembers this list, not the file.
   */
  readonly shownOutputs: () => string[];
  private setShownOutputs: (ids: string[]) => void;
  /** Where the cursor is, like "Ln 24, Col 32", for the status bar. */
  readonly cursor: () => string;
  private setCursor: (text: string) => void;
  /** Bumped whenever the cursor or selection moves, so the window can tell agents. */
  readonly moved: () => number;
  private setMoved: (update: (n: number) => number) => void;
  /** Modification time of the file as last read or written, to detect outside changes. */
  protected mtime = 0;
  /** The element the document is shown in, which scrolls it. */
  protected host?: HTMLElement;
  /** Stops following an agent's output into view (see `followOutput`). */
  protected following?: () => void;
  /** The last run sent to a kernel, or being sent. */
  private sending: Promise<unknown> = Promise.resolve();

  constructor(
    protected ws: Workspace,
    readonly path: string,
    preferredKernel: (language: string) => KernelWanted | undefined = () => undefined,
    shownOutputs: string[] = [],
    /** Where its kernels run; unless given, the document's own folder or, if the project says so, the project's. */
    kernelFolder?: string,
  ) {
    const slash = path.lastIndexOf("/");
    this.name = path.slice(slash + 1);
    this.dir = path.slice(0, slash) || "/";
    const [dirty, setDirty] = createSignal(false);
    this.dirty = dirty;
    this.setDirty = setDirty;
    this.kernels = new DocKernels(ws, () => kernelFolder ?? ws.documentFolder(this.dir), preferredKernel);
    [this.shownOutputs, this.setShownOutputs] = createSignal(shownOutputs);
    [this.cursor, this.setCursor] = createSignal("");
    [this.moved, this.setMoved] = createSignal(0);
  }

  /** Modification time of the file as last read or written. */
  diskMtime(): number {
    return this.mtime;
  }

  /**
   * Takes the file's new contents after something else changed it, such as
   * an agent. Returns false when the document has to be opened again instead.
   */
  abstract reload(file: FileText): boolean;

  /** Languages with code in this document, for the kernel bar. */
  abstract languages(): string[];
  abstract mount(parent: HTMLElement): void;
  abstract focus(): void;
  /** Saves; without `force`, refuses (CONFLICT_ERROR) if the file changed on disk. */
  abstract save(force?: boolean): Promise<void>;
  abstract runAll(): Promise<void>;
  /** Scripts: runs the selection or the statement at the cursor. */
  runCurrent?(): boolean;
  /** Where the user is in the document, for agents. */
  abstract place(): Place;
  /** Its code cells, for the Agents pane's @ menu: the cell id and how to name it. */
  codeCells(): { id: string; name: string }[] {
    return [];
  }
  /**
   * An agent's code, as the user would add it: a new cell after `after`
   * (else after the last cell), or for a script, code at its end. The cursor
   * moves to it, so the agent's next cell goes below this one.
   */
  abstract addCell(code: string, language?: string, after?: string): AgentCell;
  /** Replaces a cell's code, for an agent. */
  abstract editCell(cellId: string, code: string): AgentCell;
  /** Runs what `addCell` or `editCell` made, as its Run button would. */
  abstract runAgentCell(cell: AgentCell, code: string): Promise<void>;
  abstract dispose(): void;
  /** Shows or hides every output in the document. */
  abstract setAllOutputs(visible: boolean): void;

  /** Keeps `cursor` up to date from an editor of this document. */
  protected trackCursor(): Extension {
    return EditorView.updateListener.of((update) => {
      if (!update.selectionSet && !update.docChanged && !update.focusChanged) return;
      if (!update.view.hasFocus && !update.selectionSet) return;
      const head = update.state.selection.main.head;
      const line = update.state.doc.lineAt(head);
      this.setCursor(`Ln ${line.number}, Col ${head - line.from + 1}`);
      this.setMoved((n) => n + 1);
    });
  }

  /** Whether the output of `cellId` starts out shown. */
  protected outputShown(cellId: string): boolean {
    return this.shownOutputs().includes(cellId);
  }

  /** Records that the output of `cellId` was shown or hidden. */
  protected outputToggled(cellId: string, visible: boolean) {
    const rest = this.shownOutputs().filter((id) => id !== cellId);
    this.setShownOutputs(visible ? [...rest, cellId] : rest);
  }

  /**
   * How a cell's outputs render. With `ran`, which says what code of which
   * cell they came from, errors get Fix and Explain buttons for the agent.
   */
  protected renderContext(onResize: () => void, ran?: () => { cell: string; code?: string; language?: string }): RenderContext {
    return {
      blobUrl: (hash, mime) => this.ws.backend.blobUrl(hash, mime, this.path),
      blobText: async (hash) =>
        (await this.ws.call<{ data: string }>("get_blob", { hash, near: this.path })).data,
      onResize,
      handError: ran && ((action, error) => {
        const { cell, code = "", language } = ran();
        this.ws.askAgent?.({ action, error, code, language, cell: { path: this.path, id: cell } });
      }),
    };
  }

  /**
   * Runs `code` in this document's session for `language`, tied to a cell.
   * Code is sent one run at a time, so it reaches the kernel in the order it
   * was run, even when several runs wait for a kernel to start.
   */
  protected execute(language: string, code: string, cellId: string): Promise<string> {
    const sent = this.sending.then(async () => {
      const session = await this.kernels.ensure(language);
      return this.ws.execute(session, code, { file: this.path, cell_id: cellId });
    });
    this.sending = sent.catch(() => {});
    return sent;
  }

  /** Points each language at the session that last ran this file's cells, if it's still running. */
  protected adoptSessions(cells: { language: string; session: string; started_ms: number }[]) {
    const live = new Set(this.ws.liveSessions().map((s) => s.id));
    for (const cell of [...cells].sort((a, b) => a.started_ms - b.started_ms)) {
      if (live.has(cell.session)) this.kernels.assign(cell.language, cell.session);
    }
  }
}
