// Keeps an agent's cell in view while its output arrives, so the user sees
// what the agent made without scrolling to it. Following stops when the run
// ends, or as soon as the user scrolls, clicks or types in the document.

import type { CellOutput } from "./cellOutput";

/** A vertical extent on screen. */
export interface Span {
  top: number;
  bottom: number;
}

/** Plots still loading after the run ends keep being followed this long. */
const AFTER_DONE_MS = 1500;
/** Room left between what is shown and the edge. */
const MARGIN = 8;
const USER_EVENTS = ["wheel", "touchstart", "mousedown", "keydown"] as const;

/**
 * How far to scroll `view` down (or up, when negative) to show `cell`, the
 * cell with its output; when that is taller than the view, to show as much
 * of `output` as fits, from its start.
 */
export function revealDelta(view: Span, cell: Span, output: Span, margin = MARGIN): number {
  const room = view.bottom - view.top - 2 * margin;
  const target = cell.bottom - cell.top <= room ? cell : output;
  const top = view.top + margin;
  const bottom = view.bottom - margin;
  if (target.bottom > bottom) return Math.min(target.bottom - bottom, target.top - top);
  if (target.top < top) return target.top - top;
  return 0;
}

/**
 * Follows `output` as it grows in `scroller`, with the cell starting at
 * `cellTop()` (on screen). Returns what stops it.
 */
export function followOutput(output: CellOutput, scroller: HTMLElement, cellTop: () => number | undefined): () => void {
  let stopped = false;
  let doneAt: number | undefined;
  const reveal = () => {
    if (stopped || !output.element.isConnected) return;
    const out = output.element.getBoundingClientRect();
    const view = scroller.getBoundingClientRect();
    const delta = revealDelta(view, { top: cellTop() ?? out.top, bottom: out.bottom }, out);
    if (delta) scroller.scrollTop += delta;
  };
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(reveal);
  observer?.observe(output.element);
  const check = setInterval(() => {
    if (output.info?.phase !== "done") return;
    doneAt ??= Date.now();
    if (Date.now() - doneAt >= AFTER_DONE_MS) stop();
  }, 250);
  const stop = () => {
    if (stopped) return;
    stopped = true;
    observer?.disconnect();
    clearInterval(check);
    for (const type of USER_EVENTS) scroller.removeEventListener(type, stop, true);
  };
  for (const type of USER_EVENTS) scroller.addEventListener(type, stop, { capture: true, passive: true });
  queueMicrotask(reveal);
  return stop;
}
