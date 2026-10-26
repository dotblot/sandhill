import { describe, expect, it } from "vitest";
import cases from "../../../tests/fixtures/pages.json";
import type { Plot } from "../backend/types";
import { atPort, browserOpener, fileItem, forViewer, fromJournal, fromRun, localPort, PAGE_MIME, PLOTLY_MIME, plotlyPage, titleOf, urlItem } from "./pages";
import type { RunState } from "./runs";

describe("pages", () => {
  it("sends the pages the daemon does to the Viewer", () => {
    expect(cases.length).toBeGreaterThan(0);
    for (const c of cases) {
      expect(forViewer(c.file ?? undefined, c.data as Record<string, unknown>, c.metadata), c.name).toBe(c.viewer);
    }
  });

  it("names pages, and tells them apart by run and content", () => {
    const widget: Plot = {
      run: "r1",
      seq: 4,
      ts_ms: 1000,
      code: "DT::datatable(iris)",
      data: { "text/html": '<div class="datatables html-widget"></div><script>x</script>' },
      metadata: { "text/html": { isolated: true } },
    };
    const page = fromJournal("s1", widget)!;
    expect(page).toMatchObject({ run: "r1", session: "s1", tsMs: 1000, exact: true, title: "datatables" });
    expect(page.html).toContain("html-widget");
    expect(fromJournal("s1", { ...widget, data: { ...widget.data, "text/html": "<script>y</script>" } })!.key).not.toBe(page.key);

    const named = fromJournal("s1", {
      ...widget,
      data: { [PAGE_MIME]: { path: "/tmp/Rtmp/viewhtml1/index.html", title: "leaflet" }, "text/html": { sandhill_blob: "ab", size: 9 } },
    })!;
    expect(named).toMatchObject({ title: "leaflet", path: "/tmp/Rtmp/viewhtml1/index.html", html: { sandhill_blob: "ab" } });

    const app = fromJournal("s1", { ...widget, data: { [PAGE_MIME]: { url: "http://127.0.0.1:7123", title: "http://127.0.0.1:7123" } } })!;
    expect(app).toMatchObject({ key: "r1:http://127.0.0.1:7123", url: "http://127.0.0.1:7123", html: undefined });

    const figure = fromJournal("s1", { ...widget, data: { [PLOTLY_MIME]: { data: [], layout: {} }, "text/plain": "Figure" } })!;
    expect(figure).toMatchObject({ title: "plotly", html: undefined, plotly: { data: [] } });
  });

  it("takes a run's pages, but not a notebook's or a quiet run's", () => {
    const run: RunState = {
      id: "r2",
      session: "s1",
      code: "fig.show()",
      phase: "running",
      startedMs: 500,
      cell: { file: "/p/umap.py", cell_id: "line-1" },
      outputs: [
        { output_type: "stream", name: "stdout", text: "hi\n" },
        { output_type: "display_data", data: { "text/html": "<table></table>" }, metadata: {} },
        { output_type: "display_data", data: { "text/html": "<script>draw()</script>" }, metadata: {} },
      ],
    };
    expect(fromRun(run).map((p) => [p.title, p.exact, p.tsMs])).toEqual([["Page", false, 500]]);
    expect(fromRun({ ...run, quiet: true })).toEqual([]);
    expect(fromRun({ ...run, cell: { file: "/p/analysis.ipynb", cell_id: "c1" } })).toEqual([]);
  });

  it("opens HTML files", () => {
    expect(fileItem("/p/qc/multiqc_report.html", 9)).toEqual({
      key: "file:/p/qc/multiqc_report.html",
      tsMs: 9,
      exact: true,
      title: "multiqc_report.html",
      path: "/p/qc/multiqc_report.html",
    });
  });

  it("opens an app's address from a link", () => {
    expect(urlItem("http://localhost:8501/", 9)).toEqual({ key: "url:http://localhost:8501/", tsMs: 9, exact: true, title: "http://localhost:8501/", url: "http://localhost:8501/" });
  });

  it("opens each web page R asks for in the browser once, as it asks", () => {
    const opened: string[] = [];
    const watch = browserOpener((url) => opened.push(url));
    const page = (url: string, browser = true) => ({
      output_type: "display_data" as const,
      data: { [PAGE_MIME]: { url, title: url, ...(browser ? { browser } : {}) }, "text/plain": `Opening ${url} in the browser` },
      metadata: {},
    });
    const run: RunState = { id: "r1", session: "s1", code: "browseURL(u)", phase: "running", startedMs: 0, outputs: [] };
    watch(run);
    watch({ ...run, outputs: [page("https://bioconductor.org/")] });
    watch({ ...run, outputs: [page("https://bioconductor.org/"), page("http://127.0.0.1:7123", false)] });
    watch({ ...run, phase: "done", outputs: [page("https://bioconductor.org/"), page("https://cran.r-project.org/")] });
    expect(opened).toEqual(["https://bioconductor.org/", "https://cran.r-project.org/"]);
    // A run that had opened its page before the window opened.
    watch({ ...run, id: "r2", outputs: [page("https://example.org/")] });
    watch({ ...run, id: "r2", phase: "done", outputs: [page("https://example.org/")] });
    expect(opened).toHaveLength(2);
  });

  it("reaches apps on the daemon's machine at a port here", () => {
    expect(localPort("http://127.0.0.1:7123/")).toBe(7123);
    expect(localPort("http://localhost:8501/?x=1")).toBe(8501);
    expect(localPort("http://0.0.0.0:8050")).toBe(8050);
    expect(localPort("https://example.org/")).toBeUndefined();
    expect(localPort("not a url")).toBeUndefined();
    expect(atPort("http://localhost:8501/app?x=1", 50123)).toBe("http://127.0.0.1:50123/app?x=1");
  });

  it("names a page by its title, its widget, or its figure", () => {
    expect(titleOf("<head><title> QC report </title>")).toBe("QC report");
    expect(titleOf('<div id="htmlwidget-1" class="plotly html-widget html-fill-item">')).toBe("plotly");
    expect(titleOf('<div class="plotly-graph-div">')).toBe("plotly");
    expect(titleOf("<p>hi</p>")).toBeUndefined();
  });

  it("draws a plotly figure with plotly.js, which its data can't end", () => {
    const page = plotlyPage({ data: [{ type: "bar", name: "</script><b>" }], layout: { height: 300 } }, false);
    expect(page).toContain('<script src="https://cdn.plot.ly/plotly-');
    expect(page).toContain("height:300px");
    expect(page).not.toContain("</script><b>");
    expect(page).toContain("<\\/script><b>");
    // Filling the Viewer, the figure's own height gives way.
    const filled = plotlyPage({ data: [], layout: { height: 300 } }, true);
    expect(filled).toContain("height:100vh");
    expect(filled).not.toContain('"height":300');
  });
});
