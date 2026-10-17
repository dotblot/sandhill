import { describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { CellOutputs, SessionInfo } from "../backend/types";
import { Workspace } from "../model/workspace";
import { ScriptDoc } from "./scriptDoc";

// jsdom does no layout; CodeMirror measures text ranges when it scrolls.
const noRects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
Range.prototype.getClientRects ??= noRects;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

const SCRIPT = "x <- c(1,\n  2)\n\n# a comment\nmean(x)\n";

/** A daemon with one script, where starting a kernel takes a moment. */
function fakeWorkspace(journal: CellOutputs[] = []) {
  const executed: { code: string; cell_id: string }[] = [];
  const sessions: SessionInfo[] = [];
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      switch (method) {
        case "read_file":
          return { path: "/p/a.R", text: SCRIPT, mtime_ms: 1 } as T;
        case "file_outputs":
          return { file: "/p/a.R", cells: journal, seq: 0 } as T;
        case "list_kernelspecs":
          return [{ name: "ark", display_name: "R (Ark)", language: "R" }] as T;
        case "list_sessions":
          return sessions as T;
        case "start_session": {
          await new Promise((r) => setTimeout(r, 20));
          const info = {
            id: "s1",
            kernel: "ark",
            language: "R",
            cwd: params.cwd,
            status: "running",
            created_ms: 0,
            last_seq: 0,
            project: params.project,
          } as SessionInfo;
          sessions.push(info);
          return info as T;
        }
        case "execute": {
          const cell = params.cell as { cell_id: string };
          executed.push({ code: params.code as string, cell_id: cell.cell_id });
          return { run: `r${executed.length}` } as T;
        }
        default:
          return null as T;
      }
    },
    onEvent() {},
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  const ws = new Workspace(backend, "/p");
  return { ws, executed };
}

async function openScript(ws: Workspace) {
  await ws.refreshKernelspecs();
  const doc = await ScriptDoc.open(ws, "/p/a.R");
  const host = document.body.appendChild(document.createElement("div"));
  doc.mount(host);
  // The window shows the console in its own pane.
  host.append(doc.consoleElement);
  return { doc, host };
}

const settle = () => new Promise((r) => setTimeout(r, 60));

describe("ScriptDoc", () => {
  it("runs statement by statement, in order, while the kernel starts", async () => {
    const { ws, executed } = fakeWorkspace();
    const { doc, host } = await openScript(ws);
    // The cursor starts on line 1: the call over lines 1-2 runs, then the
    // cursor skips the blank and comment lines to mean(x).
    expect(doc.runCurrent()).toBe(true);
    expect(doc.runCurrent()).toBe(true);
    await settle();
    expect(executed).toEqual([
      { code: "x <- c(1,\n  2)", cell_id: "console-0" },
      { code: "mean(x)", cell_id: "console-1" },
    ]);
    expect(doc.consoleElement.querySelectorAll(".console-entry")).toHaveLength(2);
    expect(host.querySelector(".cm-editor")).toBeTruthy();
    await doc.runAll();
    expect(executed[2]).toEqual({ code: SCRIPT, cell_id: "console-2" });
    doc.dispose();
    host.remove();
  });

  it("brings back the console from the journal, oldest first, and carries on after it", async () => {
    const run = (cell_id: string, code: string, started_ms: number): CellOutputs => ({
      cell_id,
      run: `old-${cell_id}`,
      session: "gone",
      language: "R",
      code,
      started_ms,
      finished_ms: started_ms + 1,
      status: "ok",
      outputs: [{ output_type: "stream", name: "stdout", text: `${code}\n` }],
    });
    const { ws, executed } = fakeWorkspace([run("console-7", "second", 20), run("console-6", "first", 10)]);
    const { doc, host } = await openScript(ws);
    const echoes = [...doc.consoleElement.querySelectorAll(".console-code")].map((e) => e.textContent);
    expect(echoes).toEqual(["first", "second"]);
    doc.runCurrent();
    await settle();
    expect(executed[0].cell_id).toBe("console-8");
    doc.dispose();
    host.remove();
  });
});
