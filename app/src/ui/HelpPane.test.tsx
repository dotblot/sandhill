import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, HelpPage, HelpTopics, SessionInfo } from "../backend/types";
import type { HelpRequest } from "../model/help";
import { Workspace } from "../model/workspace";
import { HelpPane } from "./HelpPane";

const R_SESSION: SessionInfo = {
  id: "s1",
  kernel: "ir",
  language: "R",
  cwd: "/home/huan/pbmc",
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: "/home/huan/pbmc",
  pending_runs: 0,
  language_version: "4.4.1",
};
const PY_SESSION: SessionInfo = { ...R_SESSION, id: "s2", kernel: "python3", language: "python", language_version: "3.12.4" };

const FILTERS: HelpTopics = {
  session: "s1",
  topics: Array.from({ length: 8 }, (_, i) => ({ topic: i ? `filter${i}` : "filter", package: i % 2 ? "dplyr" : "stats", title: `Title ${i}` })),
  more: 12,
};

const GRID: HelpTopics = {
  session: "s1",
  topics: [{ topic: "grid.newpage", package: "grid", title: "Move to a New Page" }],
  more: 0,
  vignettes: [
    { name: "grid", package: "grid", title: "Introduction to grid", file: "/lib/grid/doc/grid.html", code: "/lib/grid/doc/grid.R" },
    { name: "frame", package: "grid", title: "Frames and packing grobs", file: "/lib/grid/doc/frame.pdf" },
  ],
};

const PAGES: Record<string, HelpPage> = {
  "stats/filter": {
    session: "s1",
    topic: "filter",
    package: "stats",
    title: "Linear Filtering on a Time Series",
    version: "4.4.1",
    html: `<h2 id='filter'>Linear Filtering</h2><h3>Description</h3><p>See <code><a href="../../stats/help/convolve">convolve</a></code> and <a href="https://doi.org/10.1/x">the paper</a>.</p><script>window.bad = 1</script><p><a href='../Example/filter'>Run examples</a></p><img src="figures/x.png">`,
    alternatives: [{ topic: "filter", package: "dplyr", title: "Keep rows" }],
  },
  "stats/convolve": { session: "s1", topic: "convolve", package: "stats", title: "Convolution", version: "4.4.1", html: "<h3>Description</h3><p>Uses the FFT.</p>" },
  "Seurat/Seurat-package": { session: "s1", topic: "Seurat-package", package: "Seurat", title: "Tools for Single Cell Genomics", version: "5.1.0", html: "<p>Seurat.</p>" },
  "/pandas.DataFrame": {
    session: "s2",
    topic: "pandas.DataFrame",
    package: "pandas",
    title: "Two-dimensional, size-mutable, potentially heterogeneous tabular data.",
    version: "3.0.1",
    kind: "class",
    signature: "(data=None, index=None, columns=None, dtype=None, copy=None)",
    text: "Two-dimensional data.\n\nParameters\n----------\ndata : ndarray, Iterable, dict, or DataFrame\n    Dict can contain Series. See :func:`concat`.\n\nExamples\n--------\n>>> pd.DataFrame({'a': [1]})",
    members: [
      { name: "merge", kind: "function", title: "Merge DataFrame objects." },
      { name: "shape", kind: "property", title: "Return a tuple." },
    ],
  },
  "/pandas.DataFrame.merge": { session: "s2", topic: "pandas.DataFrame.merge", package: "pandas", title: "Merge.", version: "3.0.1", kind: "function", signature: "(self, right)", text: "Merge." },
  "/pandas.concat": { session: "s2", topic: "pandas.concat", package: "pandas", title: "Concatenate.", version: "3.0.1", kind: "function", signature: "(objs)", text: "Concatenate." },
};

function fakeWorkspace(options: { busyOnce?: boolean } = {}) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  let busy = !!options.busyOnce;
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "list_sessions":
          return [R_SESSION, PY_SESSION] as T;
        case "help_topics":
          if (busy) {
            busy = false;
            throw new Error("daemon error: the session is busy running code; try again once it is idle");
          }
          return (params.package === "grid" ? GRID : FILTERS) as T;
        case "read_file":
          return { path: params.path, text: String(params.path).endsWith(".R") ? "library(grid)\ngrid.newpage()\n" : "<html><body><h1>Introduction</h1></body></html>", mtime_ms: 1 } as T;
        case "read_bytes":
          return { path: params.path, offset: params.offset, data: btoa("%PDF-1.4"), size: 8 } as T;
        case "help_page": {
          // Python looks a dotted name up whatever the package.
          const page = PAGES[`${params.package ?? ""}/${params.topic}`] ?? PAGES[`/${params.topic}`];
          if (!page) throw new Error(`daemon error: could not read the help: Error: no help for ${params.topic}`);
          return page as T;
        }
        case "help_source":
          return { session: "s2", topic: params.topic, package: "pandas", text: "class DataFrame:\n    pass\n", file: "/env/pandas/core/frame.py", line: 400 } as T;
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
  return { ws, calls, emit: (event: DaemonEvent) => emit(event) };
}

const settle = async () => {
  for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};
const pause = () => new Promise((resolve) => setTimeout(resolve, 200));
const type = (input: HTMLInputElement, text: string) => {
  input.value = text;
  input.dispatchEvent(new InputEvent("input", { bubbles: true }));
};

afterEach(() => {
  document.body.replaceChildren();
});

describe("HelpPane", () => {
  it("searches as you type and opens a page, following its links", async () => {
    const { ws, calls } = fakeWorkspace();
    await ws.refreshSessions();
    const ran: string[] = [];
    const opened: string[] = [];
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(
      () => (
        <HelpPane ws={ws} session={R_SESSION} sessions={[R_SESSION]} onSession={() => {}} onRun={(_, code) => ran.push(code)} openLink={(url) => opened.push(url)} shown />
      ),
      root,
    );
    expect(root.querySelector(".hr-n")?.textContent).toMatch(/^Type a function name/);
    type(root.querySelector('input[aria-label="Function or topic"]') as HTMLInputElement, "filter");
    await pause();
    expect(calls.filter((c) => c.method === "help_topics").map((c) => c.params)).toEqual([{ session: "s1", query: "filter", package: undefined, limit: 50 }]);
    expect(root.querySelector(".hres .hr-n")?.textContent).toBe("20 topics in all installed packages");
    expect(root.querySelectorAll(".hr")).toHaveLength(6);
    expect(root.querySelector(".hr-more")?.textContent).toBe("14 more");
    (root.querySelector(".hr-more") as HTMLButtonElement).click();
    expect(root.querySelectorAll(".hr")).toHaveLength(8);
    expect(root.querySelector(".hres")?.textContent).toContain("and 12 more");

    (root.querySelector(".hr") as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("filter");
    expect(root.querySelector(".helpdoc .pkg")?.textContent).toBe("stats · 4.4.1");
    expect(root.querySelector(".hr.on .hr-f")?.textContent).toBe("filter");
    expect(root.querySelector(".hd-also")?.textContent).toBe("Also in dplyr");
    const doc = root.querySelector(".rdoc") as HTMLElement;
    expect(doc.querySelector("script, img")).toBeNull();

    // Links: the web in the browser, examples in the Console, topics here.
    (doc.querySelector('a[href^="https"]') as HTMLAnchorElement).click();
    expect(opened).toEqual(["https://doi.org/10.1/x"]);
    (doc.querySelector('a[href="../Example/filter"]') as HTMLAnchorElement).click();
    expect(ran).toEqual(['example("filter", package = "stats")']);
    (doc.querySelector('a[href="../../stats/help/convolve"]') as HTMLAnchorElement).click();
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("convolve");
    (root.querySelector('button[title="Back"]') as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("filter");
    expect((root.querySelector('button[title="Forward"]') as HTMLButtonElement).disabled).toBe(false);
    dispose();
  });

  it("shows a Python page's signature, sections and members, and its source", async () => {
    const { ws, calls } = fakeWorkspace();
    await ws.refreshSessions();
    const [ask, setAsk] = createSignal<HelpRequest & { n: number }>();
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(
      () => <HelpPane ws={ws} session={PY_SESSION} sessions={[PY_SESSION]} onSession={() => {}} request={ask()} onRun={() => {}} openLink={() => {}} shown />,
      root,
    );
    setAsk({ topic: "pandas.DataFrame", n: 1 });
    await settle();
    expect(root.querySelector(".helpdoc .pkg")?.textContent).toBe("pandas · 3.0.1 · class");
    expect(root.querySelector(".hsig")?.textContent).toBe("DataFrame(\n    data=None,\n    index=None,\n    columns=None,\n    dtype=None,\n    copy=None,\n)");
    expect([...root.querySelectorAll(".pydoc h4")].map((h) => h.textContent)).toEqual(["Parameters", "Examples", "Functions", "Properties"]);
    expect(root.querySelector(".pydoc dt")?.textContent).toBe("data : ndarray, Iterable, dict, or DataFrame");
    expect(root.querySelector(".hcode")?.textContent).toBe(">>> pd.DataFrame({'a': [1]})");

    // A reference tries the class's own name, then the package's.
    (root.querySelector(".pydoc dd a") as HTMLAnchorElement).click();
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("pandas.concat");
    expect(calls.filter((c) => c.method === "help_page").map((c) => c.params.topic)).toEqual([
      "pandas.DataFrame",
      "pandas.DataFrame.concat",
      "concat",
      "pandas.concat",
    ]);
    (root.querySelector('button[title="Back"]') as HTMLButtonElement).click();
    await settle();
    (root.querySelector(".hmem") as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("pandas.DataFrame.merge");

    (root.querySelector('button[title="Back"]') as HTMLButtonElement).click();
    await settle();
    (root.querySelector(".hd-src") as HTMLButtonElement).click();
    await settle();
    expect(calls.find((c) => c.method === "help_source")?.params).toEqual({ session: "s2", topic: "pandas.DataFrame", package: "pandas" });
    expect(root.querySelector(".hsrc")?.textContent).toBe("class DataFrame:    pass");
    expect(root.querySelector(".hd-file")?.textContent).toBe("/env/pandas/core/frame.py:400");
    dispose();
  });

  it("lists a package's topics with its overview, and waits while the session is busy", async () => {
    const { ws, calls, emit } = fakeWorkspace({ busyOnce: true });
    await ws.refreshSessions();
    const [ask, setAsk] = createSignal<HelpRequest & { n: number }>();
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(
      () => <HelpPane ws={ws} session={R_SESSION} sessions={[R_SESSION]} onSession={() => {}} request={ask()} onRun={() => {}} openLink={() => {}} shown />,
      root,
    );
    setAsk({ topic: "", package: "Seurat", n: 1 });
    await settle();
    expect((root.querySelector('input[aria-label="Package"]') as HTMLInputElement).value).toBe("Seurat");
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("Seurat-package");
    expect(root.querySelector(".vbusy")?.textContent).toBe("R is running code; help shows when it finishes.");
    emit({ seq: 1, session: "s1", run: "r1", msg_type: "sandhill.run_started", ts_ms: 1, content: { code: "x" } });
    emit({ seq: 2, session: "s1", run: "r1", msg_type: "sandhill.run_finished", ts_ms: 2, content: { status: "ok" } });
    await settle();
    expect(root.querySelector(".vbusy")).toBeNull();
    expect(calls.filter((c) => c.method === "help_topics").map((c) => c.params.package)).toEqual(["Seurat", "Seurat"]);
    expect(root.querySelectorAll(".hr")).toHaveLength(6);
    // A package with no overview page keeps what's open.
    setAsk({ topic: "", package: "harmony", n: 2 });
    await settle();
    expect(root.querySelector(".helpdoc h3")?.textContent).toBe("Seurat-package");
    expect(root.querySelector(".var-note.err")).toBeNull();
    dispose();
  });

  it("lists a package's vignettes and reads one in the pane", async () => {
    const { ws, calls } = fakeWorkspace();
    const root = document.body.appendChild(document.createElement("div"));
    const made: Blob[] = [];
    URL.createObjectURL = (blob: Blob) => (made.push(blob), "blob:frame");
    URL.revokeObjectURL = () => {};
    const dispose = render(() => <HelpPane ws={ws} session={R_SESSION} sessions={[R_SESSION]} onSession={() => {}} onRun={() => {}} openLink={() => {}} shown />, root);
    type(root.querySelector('input[aria-label="Package"]') as HTMLInputElement, "grid");
    await pause();
    const rows = [...root.querySelectorAll(".hr")].map((b) => b.textContent);
    expect(rows).toEqual(["Rgrid.newpagegrid", "Introduction to gridgrid", "Frames and packing grobsgrid"]);
    ([...root.querySelectorAll(".hr")][1] as HTMLButtonElement).click();
    await settle();
    expect(calls.find((c) => c.method === "read_file")?.params).toEqual({ path: "/lib/grid/doc/grid.html" });
    const frame = root.querySelector(".hvframe") as HTMLIFrameElement;
    expect(frame.getAttribute("srcdoc")).toContain("<h1>Introduction</h1>");
    expect(frame.getAttribute("sandbox")).toBe("allow-same-origin");
    expect(root.querySelector(".hvig .pkg")?.textContent).toBe('grid · vignette("grid")');
    expect((root.querySelector(".hmain") as HTMLElement).hidden).toBe(true);
    // Its R code, a click away.
    (root.querySelector(".hvig .hd-src") as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".hvig .hsrc")?.textContent).toBe("library(grid)grid.newpage()");
    // Back to the search, then a PDF one.
    (root.querySelector(".hvig button[title='Back to the help']") as HTMLButtonElement).click();
    expect(root.querySelector(".hvig")).toBeNull();
    ([...root.querySelectorAll(".hr")][2] as HTMLButtonElement).click();
    await settle();
    expect(calls.find((c) => c.method === "read_bytes")?.params).toEqual({ path: "/lib/grid/doc/frame.pdf", offset: 0 });
    expect(made[0].type).toBe("application/pdf");
    expect((root.querySelector(".hvframe") as HTMLIFrameElement).getAttribute("src")).toBe("blob:frame");
    expect(root.querySelector(".hvig .hd-src")).toBeNull();
    dispose();
  });
});
