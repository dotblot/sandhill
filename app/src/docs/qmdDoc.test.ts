import { describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { CellOutputs } from "../backend/types";
import { Workspace } from "../model/workspace";
import { QmdDoc } from "./qmdDoc";

// jsdom does no layout; CodeMirror measures text ranges when it scrolls.
const noRects = () => Object.assign([], { item: () => null }) as unknown as DOMRectList;
Range.prototype.getClientRects ??= noRects;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

const QMD = "---\ntitle: t\n---\n\n```{r}\nx <- 1\n```\n\n```{r}\nmean(x)\n```\n";

/** A daemon with one Quarto document, whose second chunk has run. */
function fakeWorkspace() {
  const executed: { code: string; cell_id: string }[] = [];
  const ran: CellOutputs = {
    cell_id: "chunk-2",
    run: "old",
    session: "gone",
    language: "R",
    code: "mean(x)",
    started_ms: 1,
    finished_ms: 2,
    status: "ok",
    outputs: [{ output_type: "stream", name: "stdout", text: "[1] 1\n" }],
  };
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      switch (method) {
        case "read_file":
          if (params.path !== "/p/a.qmd") throw new Error("no such file");
          return { path: "/p/a.qmd", text: QMD, mtime_ms: 1 } as T;
        case "file_outputs":
          return { file: "/p/a.qmd", cells: [ran], seq: 0 } as T;
        case "list_kernelspecs":
          return [{ name: "ark", display_name: "R (Ark)", language: "R" }] as T;
        case "list_sessions":
          return [] as T;
        case "start_session":
          return { id: "s1", kernel: "ark", language: "R", cwd: "/p", status: "running", created_ms: 0, last_seq: 0 } as T;
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
  };
  return { ws: new Workspace(backend, "/p"), executed };
}

/** The editor's lines and output blocks, top to bottom. */
function shown(host: HTMLElement): string[] {
  const content = host.querySelector(".cm-content")!;
  return [...content.children].map((el) => (el.classList.contains("cm-output-block") ? `out: ${el.textContent}` : (el.textContent ?? "")));
}

const settle = () => new Promise((r) => setTimeout(r, 60));

describe("QmdDoc", () => {
  it("keeps outputs with their chunks when an agent adds a chunk above them", async () => {
    const { ws, executed } = fakeWorkspace();
    await ws.refreshKernelspecs();
    const doc = await QmdDoc.open(ws, "/p/a.qmd");
    const host = document.body.appendChild(document.createElement("div"));
    doc.mount(host);
    await settle();
    // The saved run of mean(x) is from long ago.
    const saved = (l: string) => l.startsWith("out:") && l.includes("days ago");
    expect(shown(host).some(saved)).toBe(true);

    // The new chunk takes the id chunk-2; mean(x) becomes chunk-3.
    const cell = doc.addCell("plot(x)", "r", "chunk-1");
    expect(cell.cellId).toBe("chunk-2");
    await doc.runAgentCell(cell);
    await settle();
    expect(executed).toEqual([{ code: "plot(x)", cell_id: "chunk-2" }]);

    const lines = shown(host);
    const plot = lines.findIndex((l) => l.startsWith("plot(x)"));
    const mean = lines.findIndex((l) => l.startsWith("mean(x)"));
    const meanOut = lines.findIndex(saved);
    // The run shows under the new chunk, and mean(x) keeps its output.
    expect(lines.slice(plot, mean).some((l) => l.startsWith("out:"))).toBe(true);
    expect(meanOut).toBeGreaterThan(mean);
    doc.dispose();
    host.remove();
  });
});
