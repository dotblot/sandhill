import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, Plot, SessionInfo } from "../backend/types";
import { fromJournal, PAGE_MIME } from "../model/pages";
import { Workspace } from "../model/workspace";
import { Dialogs } from "./dialogs";
import { type ViewerRequest, ViewerPane, wholePage } from "./ViewerPane";

const PROJECT = "/home/huan/pbmc";
const SESSION: SessionInfo = {
  id: "s1",
  kernel: "ir",
  language: "R",
  cwd: PROJECT,
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: PROJECT,
};

const JOURNAL: Plot[] = [
  {
    run: "r1",
    seq: 3,
    ts_ms: 1000,
    code: "DT::datatable(iris)",
    data: {
      [PAGE_MIME]: { path: "/tmp/Rtmp1/viewhtml1/index.html", title: "datatables" },
      "text/html": "<!DOCTYPE html><html><head></head><body><div class='datatables html-widget'></div><script>draw()</script></body></html>",
    },
    metadata: { "text/html": { isolated: true } },
  },
  {
    run: "r2",
    seq: 7,
    ts_ms: 2000,
    cell: { file: `${PROJECT}/qc.R`, cell_id: "line-3" },
    code: "leaflet()",
    data: { [PAGE_MIME]: { path: "/tmp/Rtmp1/viewhtml2/index.html", title: "leaflet" }, "text/html": { sandhill_blob: "ab", size: 3_000_000 } },
    metadata: { "text/html": { isolated: true } },
  },
];

function fakeWorkspace(forwarded?: (port: number) => number) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const opened: string[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: forwarded ? "lab" : "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "session_pages":
          return { session: "s1", plots: JOURNAL, seq: 9 } as T;
        case "get_blob":
          return { hash: params.hash, data: "<p>leaflet map</p><script>map()</script>" } as T;
        case "serve_page":
          return { port: 4100, path: `/sig${params.path}` } as T;
        case "serve_html":
          return { port: 4100, path: "/sig/pages/1/index.html" } as T;
        default:
          return null as T;
      }
    },
    onEvent(listener) {
      emit = listener;
    },
    onTerminal() {},
    onRequest() {},
    openLink: (url) => void opened.push(url),
    onLink() {},
    blobUrl: (hash: string) => `blob:${hash}`,
    reconnect: async () => {},
    disconnect: async () => {},
  };
  if (forwarded) backend.forwardPort = async (port) => forwarded(port);
  const ws = new Workspace(backend, PROJECT);
  return { ws, calls, opened, emit: (event: DaemonEvent) => emit(event) };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const short = (path: string) => path.slice(PROJECT.length + 1);

function mount(ws: Workspace) {
  const root = document.body.appendChild(document.createElement("div"));
  const [request, setRequest] = createSignal<ViewerRequest & { n: number }>();
  const shown: number[] = [];
  const dispose = render(
    () => (
      <>
        <ViewerPane ws={ws} project={PROJECT} sessions={[SESSION]} shortName={short} tabs={null} buttons={null} request={request()} onNew={() => shown.push(1)} />
        <Dialogs />
      </>
    ),
    root,
  );
  let n = 0;
  const ask = (r: ViewerRequest) => setRequest({ ...r, n: ++n });
  return { root, dispose, ask, shown };
}

const button = (root: ParentNode, title: string) =>
  [...root.querySelectorAll("button")].find((b) => b.title.startsWith(title)) as HTMLButtonElement;
const frame = (root: ParentNode) => root.querySelector(".viewer-frame") as HTMLIFrameElement;
const address = (root: ParentNode) => root.querySelector(".viewer-where")?.textContent;

afterEach(() => {
  document.body.replaceChildren();
  localStorage.clear();
});

describe("ViewerPane", () => {
  it("shows the newest page, in a frame that can't reach the app, and goes back through the rest", async () => {
    const { ws, calls } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    await settle();
    expect(calls.find((c) => c.method === "session_pages")?.params).toEqual({ session: "s1", limit: 20 });
    expect(address(root)).toBe("qc.R · leaflet");
    expect(root.querySelector(".plot-when")?.textContent).toMatch(/^2 of 2 · /);
    // A page kept as a blob is read for its frame.
    expect(calls.find((c) => c.method === "get_blob")?.params).toEqual({ hash: "ab" });
    expect(frame(root).getAttribute("srcdoc")).toContain("leaflet map");
    expect(frame(root).getAttribute("sandbox")).toContain("allow-scripts");
    expect(frame(root).getAttribute("sandbox")).not.toContain("allow-same-origin");
    expect(button(root, "Forward").disabled).toBe(true);
    button(root, "Back").click();
    await settle();
    expect(address(root)).toBe("Console · datatables");
    expect(frame(root).getAttribute("srcdoc")).toContain("html-widget");
    dispose();
  });

  it("brings a page to the front as a run shows it", async () => {
    const { ws, emit } = fakeWorkspace();
    const { root, dispose, shown } = mount(ws);
    await settle();
    emit({ seq: 10, session: "s1", run: "r3", msg_type: "sandhill.run_started", ts_ms: 3000, content: { code: "plotly::plot_ly()" } });
    emit({
      seq: 11,
      session: "s1",
      run: "r3",
      msg_type: "display_data",
      ts_ms: 3100,
      content: { data: { [PAGE_MIME]: { path: "/tmp/Rtmp1/viewhtml3/index.html", title: "plotly" }, "text/html": "<script>plot()</script>" }, metadata: {} },
    });
    await settle();
    expect(address(root)).toBe("Console · plotly");
    expect(shown.length).toBe(1);
    // A table stays in the Console.
    emit({ seq: 12, session: "s1", run: "r3", msg_type: "display_data", ts_ms: 3200, content: { data: { "text/html": "<table></table>" }, metadata: {} } });
    await settle();
    expect(root.querySelector(".plot-when")?.textContent).toMatch(/^3 of 3 · /);
    dispose();
  });

  it("opens an HTML file through the daemon's page server, at a port of this Mac's", async () => {
    const { ws, calls, opened } = fakeWorkspace((port) => port + 50000);
    const { root, dispose, ask, shown } = mount(ws);
    await settle();
    ask({ path: `${PROJECT}/qc/multiqc_report.html` });
    await settle();
    await settle();
    expect(shown.length).toBe(1);
    expect(address(root)).toBe("qc/multiqc_report.html");
    expect(calls.find((c) => c.method === "serve_page")?.params).toEqual({ path: `${PROJECT}/qc/multiqc_report.html` });
    expect(frame(root).getAttribute("src")).toBe(`http://127.0.0.1:54100/sig${PROJECT}/qc/multiqc_report.html`);
    // Its own address, so its scripts may keep what they keep.
    expect(frame(root).getAttribute("sandbox")).toContain("allow-same-origin");
    // It is remembered.
    expect(JSON.parse(localStorage.getItem(`sandhill.viewerFiles:${PROJECT}`)!)).toEqual([{ path: `${PROJECT}/qc/multiqc_report.html`, tsMs: expect.any(Number) }]);

    button(root, "Open in browser").click();
    await settle();
    await settle();
    expect(opened).toEqual([`http://127.0.0.1:54100/sig${PROJECT}/qc/multiqc_report.html`]);

    // An output the Console points to comes to the front, and opens in the
    // browser as its HTML, served.
    ask({ key: fromJournal("s1", JOURNAL[0])!.key });
    await settle();
    expect(address(root)).toBe("Console · datatables");
    expect(shown.length).toBe(2);
    button(root, "Open in browser").click();
    await settle();
    await settle();
    expect(calls.find((c) => c.method === "serve_html")?.params.html).toContain("html-widget");
    expect(opened[1]).toBe("http://127.0.0.1:54100/sig/pages/1/index.html");
    dispose();
  });

  it("opens an app from a Terminal link, and stops one a run started", async () => {
    const { ws, calls, emit } = fakeWorkspace((port) => port + 50000);
    const { root, dispose, ask } = mount(ws);
    await settle();
    ask({ url: "http://localhost:8501/" });
    await settle();
    await settle();
    expect(address(root)).toBe("http://localhost:8501/");
    expect(frame(root).getAttribute("src")).toBe("http://127.0.0.1:58501/");
    // The Terminal stops what it started.
    expect(button(root, "Stop")).toBeUndefined();

    // By the daemon's clock, after the link was opened.
    const t = Date.now() + 1000;
    emit({ seq: 10, session: "s1", run: "r4", msg_type: "sandhill.run_started", ts_ms: t, content: { code: "shiny::runApp()" } });
    emit({
      seq: 11,
      session: "s1",
      run: "r4",
      msg_type: "display_data",
      ts_ms: t + 100,
      content: { data: { [PAGE_MIME]: { url: "http://127.0.0.1:7123", title: "http://127.0.0.1:7123" } }, metadata: {} },
    });
    await settle();
    await settle();
    expect(address(root)).toBe("Console · http://127.0.0.1:7123");
    expect(frame(root).getAttribute("src")).toBe("http://127.0.0.1:57123/");
    button(root, "Stop").click();
    await settle();
    expect(calls.filter((c) => c.method === "interrupt").map((c) => c.params)).toEqual([{ session: "s1" }]);
    emit({ seq: 12, session: "s1", run: "r4", msg_type: "sandhill.run_finished", ts_ms: t + 9000, content: { status: "aborted" } });
    await settle();
    expect(button(root, "Stop")).toBeUndefined();
    dispose();
  });

  it("clears the pages, and the files opened", async () => {
    const { ws } = fakeWorkspace();
    const { root, dispose, ask } = mount(ws);
    await settle();
    ask({ path: `${PROJECT}/report.html` });
    await settle();
    button(root, "Clear").click();
    await settle();
    [...document.querySelectorAll("button")].find((b) => b.textContent === "Clear")!.click();
    await settle();
    expect(root.querySelector(".vempty")?.textContent).toMatch(/^Web pages show here/);
    expect(localStorage.getItem(`sandhill.viewerFiles:${PROJECT}`)).toBe("[]");
    dispose();
  });

  it("gives pages a doctype and the app's font, before their own styles", () => {
    expect(wholePage("<p>py</p>")).toMatch(/^<!doctype html><meta charset="utf-8"><style>body\{font:/);
    expect(wholePage("<!DOCTYPE html><html><head><style>x{}</style></head>")).toMatch(/^<!DOCTYPE html><html><head><style>body\{font:[^<]*<\/style><style>x\{\}/);
    expect(wholePage("<html><body>b</body></html>")).toBe("<html><body>b</body></html>");
  });
});
