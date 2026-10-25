// The title bar's edit buttons (undo, redo, cut, copy and paste) act on the
// editor that was last in focus, and dim when they can't act on it.

import { redo, redoDepth, undo, undoDepth } from "@codemirror/commands";
import { EditorView } from "@codemirror/view";
import { createSignal } from "solid-js";

const [tick, setTick] = createSignal(0);
let last: EditorView | undefined;

/** Follows the editor last in focus and its changes; every editor has it (see `baseExtensions`). */
export const editTracking = EditorView.updateListener.of((update) => {
  if (update.focusChanged && update.view.hasFocus) last = update.view;
  if (update.focusChanged || (update.view === last && (update.docChanged || update.selectionSet))) setTick((n) => n + 1);
});

/**
 * Keeps the cursor clear of the glass bars over the top of the editor pane:
 * the document scrolls under them, so the space they cover counts as hidden.
 */
export const underBars = EditorView.scrollMargins.of((view) => {
  const host = view.dom.closest<HTMLElement>(".doc-host");
  const top = host ? Number.parseFloat(getComputedStyle(host).paddingTop) : 0;
  return top ? { top } : null;
});

/** The editor last in focus, if it is still inside `root`. Reactive. */
export function lastEditor(root: HTMLElement | undefined): EditorView | undefined {
  tick();
  return last && last.dom.isConnected && root?.contains(last.dom) ? last : undefined;
}

export type EditAction = "undo" | "redo" | "cut" | "copy" | "paste";

/** Which edit buttons can act on `view`. */
export function canEdit(view: EditorView | undefined): Record<EditAction, boolean> {
  const state = view?.state;
  const selected = !!state && state.selection.ranges.some((r) => !r.empty);
  const writable = !!state && !state.readOnly;
  return {
    undo: writable && undoDepth(state) > 0,
    redo: writable && redoDepth(state) > 0,
    cut: writable && selected,
    copy: selected,
    paste: writable,
  };
}

/** Does an edit button's work, then puts the cursor back in the editor. */
export async function runEdit(view: EditorView, action: EditAction): Promise<void> {
  const { state } = view;
  const text = () =>
    state.selection.ranges
      .filter((r) => !r.empty)
      .map((r) => state.sliceDoc(r.from, r.to))
      .join(state.lineBreak);
  try {
    if (action === "undo") undo(view);
    else if (action === "redo") redo(view);
    else if (action === "copy") await navigator.clipboard.writeText(text());
    else if (action === "cut") {
      await navigator.clipboard.writeText(text());
      view.dispatch(view.state.replaceSelection(""), { userEvent: "delete.cut", scrollIntoView: true });
    } else {
      const pasted = await navigator.clipboard.readText();
      view.dispatch(view.state.replaceSelection(pasted), { userEvent: "input.paste", scrollIntoView: true });
    }
  } finally {
    view.focus();
  }
}
