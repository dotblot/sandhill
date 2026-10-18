import { describe, expect, it } from "vitest";
import type { Plot } from "../backend/types";
import {
  afterClear,
  clampSize,
  fromJournal,
  fromRun,
  imageCount,
  MAX_PLOTS,
  mergePlots,
  type PlotItem,
  plotFormats,
  plotName,
  plotSource,
  plotSrc,
} from "./plots";
import type { RunState } from "./runs";

const HASH = "a".repeat(64);
const blob = (hash = HASH) => ({ sandhill_blob: hash, size: 10 });

const journal = (over: Partial<Plot> = {}): Plot => ({
  run: "r1",
  seq: 5,
  ts_ms: 1000,
  code: "plot(x)",
  data: { "image/png": blob(), "text/plain": "<Figure>" },
  metadata: { "image/png": { width: 400, height: 300 } },
  ...over,
});

const run = (over: Partial<RunState> = {}): RunState => ({
  id: "r2",
  session: "s1",
  code: "plt.show()",
  phase: "running",
  startedMs: 2000,
  outputs: [],
  ...over,
});

const item = (key: string, tsMs: number, over: Partial<PlotItem> = {}): PlotItem => ({
  key,
  run: key.split(":")[0],
  session: "s1",
  tsMs,
  exact: true,
  images: { "image/png": blob() },
  mime: "image/png",
  ...over,
});

describe("plots", () => {
  it("reads the journal's plots, with the size the kernel asked for", () => {
    const p = fromJournal("s1", journal())!;
    expect(p).toMatchObject({ key: `r1:${HASH}`, run: "r1", session: "s1", tsMs: 1000, exact: true, mime: "image/png", width: 400, height: 300 });
    expect(Object.keys(p.images)).toEqual(["image/png"]);
    expect(fromJournal("s1", journal({ data: { "text/plain": "hi" } }))).toBeUndefined();
  });

  it("prefers SVG, which saves at any size", () => {
    const p = fromJournal("s1", journal({ data: { "image/png": blob(), "image/svg+xml": "<svg/>" } }))!;
    expect(p.mime).toBe("image/svg+xml");
    expect(plotFormats(p)).toEqual(["png", "svg"]);
    expect(plotFormats(fromJournal("s1", journal())!)).toEqual(["png"]);
  });

  it("takes the images a run shows as it goes, but not a quiet run's", () => {
    const outputs = [
      { output_type: "stream", name: "stdout", text: "hi" },
      { output_type: "display_data", data: { "image/png": blob("b".repeat(64)) }, metadata: {} },
      { output_type: "display_data", data: { "image/svg+xml": "<svg/>" }, metadata: {} },
    ];
    const plots = fromRun(run({ outputs }));
    expect(plots.map((p) => p.mime)).toEqual(["image/png", "image/svg+xml"]);
    expect(plots[0]).toMatchObject({ tsMs: 2000, exact: false, key: `r2:${"b".repeat(64)}` });
    expect(plots[1].key).toMatch(/^r2:[0-9a-f]+-6$/);
    expect(imageCount(run({ outputs }))).toBe(2);
    expect(fromRun(run({ outputs, quiet: true }))).toEqual([]);
  });

  it("merges by key, in time order, keeping the journal's time", () => {
    const live = item("r2:x", 2000, { exact: false });
    let list = mergePlots([], [item("r1:a", 1000), live]);
    expect(list.map((p) => p.key)).toEqual(["r1:a", "r2:x"]);
    // The journal's copy replaces the live one; a later live copy doesn't undo that.
    list = mergePlots(list, [item("r2:x", 2500)]);
    expect(list[1]).toMatchObject({ tsMs: 2500, exact: true });
    const same = mergePlots(list, [live]);
    expect(same).toBe(list);
    // Another session's plot lands by time.
    list = mergePlots(list, [item("q1:b", 1500, { session: "s2" })]);
    expect(list.map((p) => p.key)).toEqual(["r1:a", "q1:b", "r2:x"]);
  });

  it("keeps the newest plots when there are too many", () => {
    const many = Array.from({ length: MAX_PLOTS + 5 }, (_, i) => item(`r${i}:a`, i));
    const list = mergePlots([], many);
    expect(list.length).toBe(MAX_PLOTS);
    expect(list[0].key).toBe("r5:a");
  });

  it("says where a plot came from and names it for saving", () => {
    const short = (p: string) => p.replace("/home/huan/pbmc/", "");
    const qmd = item("r1:a", 1, { cell: { file: "/home/huan/pbmc/analysis.qmd", cell_id: "umap" } });
    const unlabeled = item("r2:a", 2, { cell: { file: "/home/huan/pbmc/analysis.qmd", cell_id: "chunk-3" } });
    const script = item("r3:a", 3, { cell: { file: "/home/huan/pbmc/qc.py", cell_id: "console-1" } });
    const agent = item("r4:a", 4, { by: "Claude" });
    const prompt = item("r5:a", 5);
    expect([qmd, unlabeled, script, agent, prompt].map((p) => plotSource(p, short))).toEqual([
      "analysis.qmd · umap",
      "analysis.qmd",
      "qc.py",
      "Claude",
      "Console",
    ]);
    const all = [qmd, unlabeled, script, agent, prompt];
    expect(all.map((p) => plotName(p, all))).toEqual(["umap", "analysis", "qc", "plot", "plot"]);
    // A run that drew two plots numbers them.
    const two = [item("r6:a", 6), item("r6:b", 6)];
    expect(two.map((p) => plotName(p, two))).toEqual(["plot-1", "plot-2"]);
    expect(plotName(item("r7:a", 7, { cell: { file: "/p/my plot (v2).qmd", cell_id: "chunk-1" } }), [])).toBe("my-plot-v2");
  });

  it("makes URLs for blobs and inline images", () => {
    const url = (hash: string, mime: string) => `blob/${hash}?${mime}`;
    expect(plotSrc(item("r1:a", 1), url)).toBe(`blob/${HASH}?image/png`);
    const svg = item("r1:b", 1, { mime: "image/svg+xml", images: { "image/svg+xml": "<svg a='1'/>" } });
    expect(plotSrc(svg, url)).toBe("data:image/svg+xml;charset=utf-8,%3Csvg%20a%3D'1'%2F%3E");
    const png = item("r1:c", 1, { images: { "image/png": "iVBO\nRw0=" } });
    expect(plotSrc(png, url)).toBe("data:image/png;base64,iVBORw0=");
  });

  it("hides what was cleared, and keeps sizes in range", () => {
    const list = [item("r1:a", 1000), item("r2:a", 2000)];
    expect(afterClear(list, undefined)).toBe(list);
    expect(afterClear(list, 1000).map((p) => p.key)).toEqual(["r2:a"]);
    expect([clampSize(50, 1200), clampSize(9000, 1200), clampSize(Number.NaN, 1200), clampSize(1500.4, 1200)]).toEqual([100, 6000, 1200, 1500]);
  });
});
