import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it } from "vitest";
import type { Backend } from "../backend";
import type { DaemonEvent, Plot, SessionInfo } from "../backend/types";
import { Workspace } from "../model/workspace";
import { Dialogs } from "./dialogs";
import { PlotsPane } from "./PlotsPane";

const PROJECT = "/home/huan/pbmc";
const SESSION: SessionInfo = {
  id: "s1",
  kernel: "python3",
  language: "python",
  cwd: PROJECT,
  pid: 1,
  status: "running",
  created_ms: 0,
  last_seq: 0,
  project: PROJECT,
};
const HASH = (c: string) => c.repeat(64);
const blob = (c: string) => ({ sandhill_blob: HASH(c), size: 10 });

const JOURNAL: Plot[] = [
  { run: "r1", seq: 3, ts_ms: 1000, code: "plot(x)", data: { "image/png": blob("a") }, metadata: {} },
  {
    run: "r2",
    seq: 7,
    ts_ms: 2000,
    cell: { file: `${PROJECT}/analysis.qmd`, cell_id: "umap" },
    code: "sc.pl.umap(adata)",
    data: { "image/svg+xml": "<svg xmlns='http://www.w3.org/2000/svg' width='40' height='30'/>" },
    metadata: {},
  },
];

function fakeWorkspace() {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  let emit: (event: DaemonEvent) => void = () => {};
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "session_plots": {
          const limit = (params.limit as number) ?? 100;
          return { session: "s1", plots: JOURNAL.slice(-limit), seq: 9 } as T;
        }
        case "stat_files":
          // An SVG of that name is there already.
          return (params.paths as string[]).map((path) => (path.endsWith(".svg") ? { path, size: 9, mtime_ms: 5 } : { path, size: 0 })) as T;
        case "get_blob":
          return { hash: params.hash, data: "iVBORw0KGgo=" } as T;
        case "write_file":
          return { path: params.path, mtime_ms: 1 } as T;
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
    blobUrl: (hash: string) => `blob:${hash.slice(0, 1)}`,
    reconnect: async () => {},
    disconnect: async () => {},
  };
  const ws = new Workspace(backend, PROJECT);
  return { ws, calls, emit: (event: DaemonEvent) => emit(event) };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const short = (path: string) => path.slice(PROJECT.length + 1);

function mount(ws: Workspace, notebooks = true) {
  const root = document.body.appendChild(document.createElement("div"));
  const [withNotebooks, setNotebooks] = createSignal(notebooks);
  const dispose = render(
    () => (
      <>
        <PlotsPane ws={ws} project={PROJECT} sessions={[SESSION]} notebooks={withNotebooks()} shortName={short} hostLabel="This Mac" buttons={null} />
        <Dialogs />
      </>
    ),
    root,
  );
  return { root, dispose, setNotebooks };
}

const button = (root: ParentNode, title: string) =>
  [...root.querySelectorAll("button")].find((b) => b.title.startsWith(title)) as HTMLButtonElement;

afterEach(() => {
  document.body.replaceChildren();
  localStorage.clear();
});

describe("PlotsPane", () => {
  it("shows the newest plot, and goes back through the rest", async () => {
    const { ws, calls } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    expect(calls.find((c) => c.method === "session_plots")?.params).toEqual({ session: "s1", limit: 100 });
    expect(calls.find((c) => c.method === "subscribe")?.params).toEqual({ session: "s1", since_seq: 9 });
    expect(root.querySelector(".plot-from")?.textContent).toBe("analysis.qmd · umap");
    expect(root.querySelector(".plot-when")?.textContent).toMatch(/^2 of 2 · /);
    expect(root.querySelectorAll(".plot-thumbs button").length).toBe(2);
    expect(button(root, "Next plot").disabled).toBe(true);
    button(root, "Previous plot").click();
    expect(root.querySelector(".plot-from")?.textContent).toBe("Console");
    expect((root.querySelector(".plot-img") as HTMLImageElement).src).toBe(`blob:a`);
    dispose();
  });

  it("brings a plot to the front as a run draws it, then reads its time from the journal", async () => {
    const { ws, calls, emit } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    button(root, "Previous plot").click();
    emit({ seq: 10, session: "s1", run: "r3", msg_type: "sandhill.run_started", ts_ms: 3000, content: { code: "plt.plot(y)", by: "Claude" } });
    emit({ seq: 11, session: "s1", run: "r3", msg_type: "display_data", ts_ms: 3100, content: { data: { "image/png": blob("c") }, metadata: {} } });
    expect(root.querySelector(".plot-from")?.textContent).toBe("Claude");
    expect(root.querySelector(".plot-when")?.textContent).toMatch(/^3 of 3/);
    // A quiet run's images aren't plots.
    emit({ seq: 12, session: "s1", run: "q1", msg_type: "sandhill.run_started", ts_ms: 3200, content: { code: "x", quiet: true } });
    emit({ seq: 13, session: "s1", run: "q1", msg_type: "display_data", ts_ms: 3300, content: { data: { "image/png": blob("d") } } });
    expect(root.querySelectorAll(".plot-thumbs button").length).toBe(3);
    const before = calls.filter((c) => c.method === "session_plots").length;
    emit({ seq: 14, session: "s1", run: "r3", msg_type: "sandhill.run_finished", ts_ms: 3400, content: { status: "ok" } });
    await settle();
    const reads = calls.filter((c) => c.method === "session_plots");
    expect(reads.length).toBe(before + 1);
    expect(reads[reads.length - 1].params).toEqual({ session: "s1", limit: 1 });
    dispose();
  });

  it("leaves notebooks' plots out unless asked", async () => {
    const { ws, emit } = fakeWorkspace();
    const { root, dispose, setNotebooks } = mount(ws, false);
    await settle();
    const count = () => root.querySelector(".plot-when")?.textContent?.split(" · ")[0];
    expect(count()).toBe("1 of 1");
    expect(root.querySelector(".plot-from")?.textContent).toBe("Console");
    // A notebook cell drawing doesn't show, or move the pane.
    emit({
      seq: 10,
      session: "s1",
      run: "r3",
      msg_type: "sandhill.run_started",
      ts_ms: 3000,
      content: { code: "sc.pl.pca(adata)", cell: { file: `${PROJECT}/qc.ipynb`, cell_id: "c1" } },
    });
    emit({ seq: 11, session: "s1", run: "r3", msg_type: "display_data", ts_ms: 3100, content: { data: { "image/png": blob("c") }, metadata: {} } });
    expect(count()).toBe("1 of 1");
    // A script's plot does.
    emit({
      seq: 12,
      session: "s1",
      run: "r4",
      msg_type: "sandhill.run_started",
      ts_ms: 3200,
      content: { code: "plot(y)", cell: { file: `${PROJECT}/qc.R`, cell_id: "line" } },
    });
    emit({ seq: 13, session: "s1", run: "r4", msg_type: "display_data", ts_ms: 3300, content: { data: { "image/png": blob("d") }, metadata: {} } });
    expect(count()).toBe("2 of 2");
    expect(root.querySelector(".plot-from")?.textContent).toBe("qc.R");
    setNotebooks(true);
    expect(count()).toBe("4 of 4");
    dispose();
  });

  it("clears the plots, and keeps them cleared next time", async () => {
    const { ws } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    button(root, "Clear the plots").click();
    await settle();
    expect(document.querySelector(".modal-message")?.textContent).toBe("Clear all 2 plots from this pane?");
    ([...document.querySelectorAll(".modal button")].find((b) => b.textContent === "Clear") as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".plot-img")).toBeNull();
    expect(root.querySelector(".vempty")).not.toBeNull();
    dispose();
    const again = mount(fakeWorkspace().ws);
    await settle();
    expect(again.root.querySelector(".plot-img")).toBeNull();
    again.dispose();
  });

  it("saves an SVG plot as SVG under figures, named for its chunk", async () => {
    const { ws, calls } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    button(root, "Zoom").click();
    const dialog = document.querySelector(".pzoom") as HTMLElement;
    expect(dialog).not.toBeNull();
    dialog.querySelector(".pz-plot img")!.dispatchEvent(new Event("load"));
    // An SVG plot can be saved at a size, as PNG, or as SVG.
    expect((dialog.querySelector("[aria-label='Width in pixels']") as HTMLInputElement).value).toBe("1200");
    expect([...dialog.querySelectorAll(".pz-seg button")].map((b) => b.textContent)).toEqual(["PNG", "SVG"]);
    ([...dialog.querySelectorAll(".pz-seg button")][1] as HTMLButtonElement).click();
    expect(dialog.querySelector("[aria-label='Width in pixels']")).toBeNull();
    (dialog.querySelector(".pz-btn.pri") as HTMLButtonElement).click();
    await settle();
    const field = document.querySelector(".modal-input") as HTMLInputElement;
    expect(field.value).toBe("figures/umap.svg");
    (document.querySelector("form.modal") as HTMLFormElement).requestSubmit();
    await settle();
    await settle();
    // The file is there already: replace it.
    ([...document.querySelectorAll(".modal button")].find((b) => b.textContent === "Replace") as HTMLButtonElement).click();
    await settle();
    await settle();
    const write = calls.find((c) => c.method === "write_file");
    expect(write?.params).toEqual({ path: `${PROJECT}/figures/umap.svg`, text: JOURNAL[1].data["image/svg+xml"], base64: false });
    expect(dialog.querySelector(".pz-note")?.textContent).toBe("Saved figures/umap.svg.");
    dispose();
  });

  it("saves a PNG plot's own bytes, where it was saved last", async () => {
    localStorage.setItem(`sandhill.plotsFolder:${PROJECT}`, "results");
    const { ws, calls } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    button(root, "Previous plot").click();
    button(root, "Save as PNG").click();
    const dialog = document.querySelector(".pzoom") as HTMLElement;
    dialog.querySelector(".pz-plot img")!.dispatchEvent(new Event("load"));
    expect(dialog.querySelector(".pz-seg")).toBeNull();
    (dialog.querySelector(".pz-btn.pri") as HTMLButtonElement).click();
    await settle();
    expect((document.querySelector(".modal-input") as HTMLInputElement).value).toBe("results/plot.png");
    (document.querySelector("form.modal") as HTMLFormElement).requestSubmit();
    await settle();
    await settle();
    await settle();
    expect(calls.find((c) => c.method === "get_blob")?.params).toEqual({ hash: HASH("a"), near: undefined });
    expect(calls.find((c) => c.method === "write_file")?.params).toEqual({ path: `${PROJECT}/results/plot.png`, text: "iVBORw0KGgo=", base64: true });
    expect(dialog.querySelector(".pz-note")?.textContent).toBe("Saved results/plot.png, 800 × 600 px.");
    // Escape closes the dialog.
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(document.querySelector(".pzoom")).toBeNull();
    dispose();
  });
});
