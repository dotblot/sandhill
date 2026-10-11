// The output shown under one Quarto chunk or notebook cell: a status line
// and the outputs, from a saved copy or a live run.

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
  private area: OutputArea;
  private unwatch?: () => void;
  private timer?: ReturnType<typeof setInterval>;
  info?: OutputInfo;
  runId?: string;

  constructor(
    ctx: RenderContext,
    private onChange: (info: OutputInfo) => void = () => {},
  ) {
    this.element = document.createElement("div");
    this.element.className = "cell-output empty";
    this.statusLine = document.createElement("div");
    this.statusLine.className = "cell-status";
    this.area = new OutputArea(ctx);
    this.element.append(this.statusLine, this.area.element);
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
    this.area.update([]);
    this.statusLine.textContent = "";
    this.element.classList.add("empty");
  }

  setStale(stale: boolean) {
    this.element.classList.toggle("stale", stale);
    this.statusLine.title = stale ? "The code changed since these outputs were produced" : "";
  }

  dispose() {
    this.detach();
  }

  private render(info: OutputInfo) {
    this.info = info;
    this.area.update(info.outputs);
    this.element.classList.toggle("empty", false);
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
    this.statusLine.textContent = statusText(info, Date.now());
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

function ago(ms: number): string {
  const m = Math.floor(ms / 60000);
  if (m < 1) return "just now";
  if (m < 60) return `${m} min ago`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.floor(h / 24)} days ago`;
}
