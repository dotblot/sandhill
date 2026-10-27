// The output shown under one Quarto chunk or notebook cell: a status line
// and the outputs, from a saved copy or a live run.
//
// Outputs can be hidden, leaving the status line with a summary ("plot,
// table"). A hidden output is not rendered at all, so its plots are never
// loaded or decoded: that is what keeps a notebook full of plots small. A
// notebook's outputs also wait until their cell comes near view, so showing
// every output of a long notebook draws only the few on screen.

import type { Output } from "../backend/types";
import type { RunPhase, RunState, RunStore } from "../model/runs";
import { OutputArea, type RenderContext } from "../render/outputs";

export interface OutputInfo {
  phase: RunPhase;
  status?: string;
  executionCount?: number;
  startedMs?: number;
  finishedMs?: number;
  code: string;
  outputs: Output[];
}

export class CellOutput {
  readonly element: HTMLElement;
  private statusLine: HTMLElement;
  private statusText: HTMLElement;
  private area: OutputArea;
  private unwatch?: () => void;
  private timer?: ReturnType<typeof setInterval>;
  /** Whether the outputs may be drawn now: false while far from view, see `setNearView`. */
  private near: boolean;
  /** Outputs to draw once near view. */
  private waiting = false;
  info?: OutputInfo;
  runId?: string;

  constructor(
    private ctx: RenderContext,
    private onChange: (info: OutputInfo) => void = () => {},
    private visible = true,
    /** Called whenever the output is shown or hidden. */
    private onToggle: (visible: boolean) => void = () => {},
    /** Wait for `setNearView(true)` before drawing anything. */
    deferred = false,
  ) {
    this.near = !deferred;
    this.element = document.createElement("div");
    this.element.className = "cell-output empty";
    this.element.classList.toggle("collapsed", !visible);
    this.statusLine = document.createElement("div");
    this.statusLine.className = "cell-status";
    this.statusLine.addEventListener("click", () => {
      if (this.info?.outputs.length) this.setVisible(!this.visible);
    });
    const caret = document.createElement("span");
    caret.className = "cell-caret";
    this.statusText = document.createElement("span");
    this.statusLine.append(caret, this.statusText);
    this.area = new OutputArea(ctx);
    this.element.append(this.statusLine, this.area.element);
  }

  get isVisible(): boolean {
    return this.visible;
  }

  /** Shows or hides the outputs; hidden ones are taken out of the page. */
  setVisible(visible: boolean) {
    if (visible === this.visible) return;
    this.visible = visible;
    this.element.classList.toggle("collapsed", !visible);
    this.draw();
    this.renderStatus();
    this.onToggle(visible);
    this.ctx.onResize();
  }

  /** Outputs read from a file (a notebook or a .qmd sidecar). */
  showSaved(info: OutputInfo) {
    this.detach();
    this.runId = undefined;
    this.render(info);
  }

  /** Follows a run in the store until another run or saved copy replaces it. */
  bindRun(store: RunStore, runId: string) {
    if (this.runId === runId) return;
    this.detach();
    this.runId = runId;
    const show = (run: RunState) => this.render(fromRun(run));
    const run = store.get(runId);
    if (run) show(run);
    this.unwatch = store.watch(runId, show);
  }

  clear() {
    this.detach();
    this.runId = undefined;
    this.info = undefined;
    this.waiting = false;
    this.area.update([]);
    this.statusText.textContent = "";
    this.element.classList.add("empty");
  }

  setStale(stale: boolean) {
    this.element.classList.toggle("stale", stale);
    this.statusText.title = stale ? "The code changed since these outputs were produced" : "";
  }

  dispose() {
    this.detach();
  }

  /** Called as the output moves far from view and back. */
  setNearView(near: boolean) {
    this.near = near;
    if (near && this.waiting) this.draw();
    if (near) this.area.resume();
    else this.area.suspend();
  }

  /** Draws the outputs, or waits until near view to; hidden ones are taken out at once. */
  private draw() {
    const outputs = this.visible ? (this.info?.outputs ?? []) : [];
    this.waiting = outputs.length > 0 && !this.near;
    if (!this.waiting) this.area.update(outputs);
  }

  private render(info: OutputInfo) {
    this.info = info;
    if (this.visible) this.draw();
    this.element.classList.toggle("empty", false);
    this.element.classList.toggle("has-outputs", info.outputs.length > 0);
    this.element.dataset.phase = info.phase;
    this.element.dataset.status = info.status ?? "";
    this.renderStatus();
    if (info.phase === "running" && !this.timer) {
      this.timer = setInterval(() => this.renderStatus(), 1000);
    } else if (info.phase !== "running" && this.timer) {
      clearInterval(this.timer);
      this.timer = undefined;
    }
    this.onChange(info);
  }

  private renderStatus() {
    const info = this.info;
    if (!info) return;
    const summary = this.visible ? "" : outputSummary(info.outputs);
    this.statusText.textContent = statusText(info, Date.now()) + (summary ? ` · ${summary}` : "");
    this.statusLine.title = info.outputs.length ? (this.visible ? "Hide output" : "Show output") : "";
  }

  private detach() {
    this.unwatch?.();
    this.unwatch = undefined;
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }
}

function fromRun(run: RunState): OutputInfo {
  return {
    phase: run.phase,
    status: run.status,
    executionCount: run.executionCount,
    startedMs: run.startedMs,
    finishedMs: run.finishedMs,
    code: run.code,
    outputs: run.outputs,
  };
}

/** Kinds of output worth counting in a summary; text is just "text". */
const COUNTED = new Set(["plot", "table", "widget", "error"]);

/** What a hidden output holds, in a few words: "plot", "2 plots, table". */
export function outputSummary(outputs: Output[]): string {
  const counts = new Map<string, number>();
  for (const output of outputs) {
    const kind = outputKind(output);
    if (kind) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  return [...counts]
    .map(([kind, n]) => (n === 1 || !COUNTED.has(kind) ? kind : `${n} ${kind}s`))
    .join(", ");
}

function outputKind(output: Output): string | undefined {
  if (output.output_type === "error") return "error";
  if (output.output_type === "stream") return "text";
  const data = (output.data ?? {}) as Record<string, unknown>;
  const mimes = Object.keys(data);
  if (!mimes.length) return undefined;
  if (mimes.some((m) => m.startsWith("image/") || m === "application/pdf")) return "plot";
  const html = data["text/html"];
  if (html !== undefined) return typeof html === "string" && !/<table/i.test(html) ? "HTML" : "table";
  if (mimes.some((m) => m.includes("widget") || m.includes("plotly") || m.includes("vega"))) return "widget";
  return "text";
}

export function statusText(info: OutputInfo, now: number): string {
  const count = info.executionCount ? `[${info.executionCount}] ` : "";
  switch (info.phase) {
    case "queued":
      return "Waiting for the kernel…";
    case "running":
      return info.startedMs ? `Running… ${duration(now - info.startedMs)}` : "Running…";
    case "done": {
      const took =
        info.startedMs && info.finishedMs ? ` in ${duration(info.finishedMs - info.startedMs)}` : "";
      const when = info.finishedMs ? ` · ${ago(now - info.finishedMs)}` : "";
      switch (info.status) {
        case "ok":
          return `${count}Done${took}${when}`;
        case "error":
          return `${count}Error${took}${when}`;
        case "aborted":
          return "Skipped after an earlier error";
        case "unknown":
          return `Finished while the daemon was restarting${when}`;
        default:
          return `${count}Finished${when}`;
      }
    }
  }
}

function duration(ms: number): string {
  const s = Math.max(0, ms) / 1000;
  if (s < 10) return `${s.toFixed(1)}s`;
  if (s < 60) return `${Math.round(s)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${Math.round(s % 60)}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** How long ago, in words: "just now", "5 min ago", "3 h ago". */
export function ago(ms: number): string {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.floor(h / 24)} days ago`;
}
