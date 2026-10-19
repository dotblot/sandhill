import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, SessionInfo, SessionRun } from "../backend/types";
import type { HelpRequest } from "../model/help";
import { Workspace } from "../model/workspace";
import { SessionConsole } from "./sessionConsole";

// jsdom does no layout; CodeMirror measures text ranges when it scrolls.
const noRects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
Range.prototype.getClientRects ??= noRects;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

const SESSION: SessionInfo = {
  id: "s1",
  kernel: "ark",
  language: "R",
  cwd: "/home/huan/pbmc",
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 5,
  project: "/home/huan/pbmc",
  pending_runs: 0,
  language_version: "4.4.1",
};

const done = (run: string, code: string, started_ms: number, extra: Partial<SessionRun> = {}): SessionRun => ({
  run,
  code,
  started_ms,
  finished_ms: started_ms + 1,
  status: "ok",
  outputs: [{ output_type: "stream", name: "stdout", text: `${code} ran\n` }],
  ...extra,
});

/** A daemon with one R session whose journal holds `runs`. */
function fakeWorkspace(runs: SessionRun[]) {
  const executed: Record<string, unknown>[] = [];
  const interrupted: string[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      switch (method) {
        case "session_runs":
          return { session: "s1", runs, seq: 5 } as T;
        case "list_sessions":
          return [SESSION] as T;
        case "execute":
          executed.push(params);
          return { run: `new${executed.length}`, msg_id: "m" } as T;
        case "interrupt":
          interrupted.push(params.session as string);
          return null as T;
        default:
          return null as T;
      }
    },
    onEvent(listener) {
      emit = listener;
    },
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  const ws = new Workspace(backend, "/home/huan/pbmc");
  return { ws, executed, interrupted, emit: (event: DaemonEvent) => emit(event) };
}

const opened: { console: SessionConsole; host: HTMLElement }[] = [];

async function openConsole(ws: Workspace) {
  await ws.refreshSessions();
  const files: string[] = [];
  const helped: HelpRequest[] = [];
  const console = new SessionConsole(ws, "s1", "r", {
    openFile: (path) => files.push(path),
    shortName: (path) => path.replace("/home/huan/pbmc/", ""),
    onError: (message) => {
      throw new Error(message);
    },
    openHelp: (request) => helped.push(request),
  });
  const host = document.body.appendChild(document.createElement("div"));
  host.append(console.element);
  await console.ready;
  opened.push({ console, host });
  return { console, files, helped };
}

const echoes = (console: SessionConsole) =>
  [...console.element.querySelectorAll(".console-code")].map((e) => e.textContent);

afterEach(() => {
  for (const { console, host } of opened.splice(0)) {
    console.dispose();
    host.remove();
  }
  localStorage.clear();
});

describe("SessionConsole", () => {
  it("shows every run of its session and where it came from", async () => {
    const { ws } = fakeWorkspace([
      done("a", "x <- 1", 10),
      done("b", "head(x)", 20, { cell: { file: "/home/huan/pbmc/qc.R", cell_id: "console-0" } }),
      done("c", "summary(x)", 30, { by: "Claude" }),
      done("d", "#| label: umap\nplot(x)\nabline(h = 0)\nlegend()", 40, { cell: { file: "/home/huan/pbmc/analysis.qmd", cell_id: "umap" } }),
    ]);
    const { console, files } = await openConsole(ws);
    expect(echoes(console)).toEqual(["x <- 1", "head(x)", "summary(x)", "plot(x)abline(h = 0)… 1 more line"]);
    const entries = console.element.querySelectorAll(".console-entry");
    expect(entries[0].querySelector(".console-from")).toBeNull();
    expect(entries[1].querySelector(".console-from")?.textContent).toBe("qc.R");
    expect(entries[2].classList.contains("from-agent")).toBe(true);
    expect(entries[2].querySelector(".console-by")?.textContent).toBe("Claude");
    // A notebook cell's output stays under the cell.
    expect(entries[3].classList.contains("from-cell")).toBe(true);
    (entries[3].querySelector(".console-file") as HTMLButtonElement).click();
    expect(files).toEqual(["/home/huan/pbmc/analysis.qmd"]);
    expect(entries[0].textContent).toContain("x <- 1 ran");
    expect(console.element.querySelector(".console-head")?.textContent).toBe("R 4.4.1 · ark · /home/huan/pbmc");
  });

  it("adds new runs as they start, but not quiet ones or other sessions'", async () => {
    const { ws, emit } = fakeWorkspace([done("a", "x <- 1", 10)]);
    const { console } = await openConsole(ws);
    const started = (run: string, session: string, content: Record<string, unknown>, seq: number) =>
      emit({ seq, session, run, msg_type: "sandhill.run_started", ts_ms: 100 + seq, content });
    started("q", "s1", { code: "ls()", quiet: true }, 6);
    started("o", "s2", { code: "other" }, 7);
    started("n", "s1", { code: "y <- 2", by: "Codex" }, 8);
    expect(echoes(console)).toEqual(["x <- 1", "y <- 2"]);
    expect(console.element.querySelector(".from-agent .console-by")?.textContent).toBe("Codex");
  });

  it("runs what is typed when it is complete, and recalls earlier code", async () => {
    const { ws, executed } = fakeWorkspace([done("a", "x <- 1", 10), done("b", "y <- 2", 20)]);
    const { console } = await openConsole(ws);
    const press = (key: string, shiftKey = false) => {
      const content = console.element.querySelector(".cm-content") as HTMLElement;
      content.dispatchEvent(new KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true }));
    };
    console.setInput("f(a,");
    press("Enter");
    expect(executed).toEqual([]);
    expect(console.input).toBe("f(a,\n  ");
    console.setInput("mean(x)");
    press("Enter");
    await Promise.resolve();
    expect(executed).toEqual([{ session: "s1", code: "mean(x)" }]);
    expect(console.input).toBe("");
    press("ArrowUp");
    expect(console.input).toBe("mean(x)");
    press("ArrowUp");
    expect(console.input).toBe("y <- 2");
    press("ArrowDown");
    press("ArrowDown");
    expect(console.input).toBe("");
  });

  it("opens help in the Help pane rather than running it", async () => {
    const { ws, executed } = fakeWorkspace([]);
    const { console, helped } = await openConsole(ws);
    const content = console.element.querySelector(".cm-content") as HTMLElement;
    console.setInput("?stats::filter");
    content.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
    await Promise.resolve();
    expect(executed).toEqual([]);
    expect(helped).toEqual([{ topic: "filter", package: "stats" }]);
    expect(console.input).toBe("");
    content.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true, cancelable: true }));
    expect(console.input).toBe("?stats::filter");
  });

  it("stays cleared when it opens again", async () => {
    const { ws } = fakeWorkspace([done("a", "x <- 1", 10)]);
    const first = await openConsole(ws);
    expect(echoes(first.console)).toHaveLength(1);
    first.console.clear();
    expect(echoes(first.console)).toHaveLength(0);
    const again = await openConsole(ws);
    expect(echoes(again.console)).toHaveLength(0);
  });

  it("interrupts with Ctrl-C when nothing is selected", async () => {
    const { ws, interrupted } = fakeWorkspace([]);
    const { console } = await openConsole(ws);
    const content = console.element.querySelector(".cm-content") as HTMLElement;
    content.dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true }));
    await Promise.resolve();
    expect(interrupted).toEqual(["s1"]);
  });
});
