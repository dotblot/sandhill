// The Viewer's pages: web pages R and Python showed from the Console and
// scripts (htmlwidgets such as DT, plotly and leaflet, which R's viewer
// names), read from the daemon's journal (`session_pages`) and added to as
// runs show more; apps, from anywhere; and HTML files opened from the file
// tree. Notebooks keep their other pages under their cells.

import { type CellRef, isBlobRef, type Output, type Plot } from "../backend/types";
import { imageId } from "./plots";
import { isScript, type RunState } from "./runs";

/** The output type the daemon gives a page R's viewer named: `{path}` or `{url}`, and `title`. */
export const PAGE_MIME = "application/vnd.sandhill.page+json";

/** A plotly figure, as Python's plotly shows one in Jupyter: `{data, layout, config}`. */
export const PLOTLY_MIME = "application/vnd.plotly.v1+json";

/**
 * plotly.js, which draws a figure Python's plotly sent as data; loaded from
 * plotly's own address, so a figure needs the internet as plotly's
 * "connected" pages do. 2.28 and later read the typed arrays plotly 6 and 7
 * send.
 */
const PLOTLY_JS = "https://cdn.plot.ly/plotly-2.35.2.min.js";

/** How many pages the Viewer keeps, all sessions together. */
export const MAX_PAGES = 100;

export interface PageItem {
  /** The run, then the page; a file's path for a file opened from the tree. */
  key: string;
  /** The run that showed it; none for a file opened from the tree. */
  run?: string;
  session?: string;
  /** When it was shown, by the daemon's clock (this Mac's, for a file). */
  tsMs: number;
  /** True when the time is the journal's; pages seen as a run goes have its start. */
  exact: boolean;
  cell?: CellRef;
  by?: string;
  title: string;
  /** An output's HTML: the text, or a blob reference. Shown in a frame of its own. */
  html?: unknown;
  /** Or a plotly figure: the data, or a blob reference to it. */
  plotly?: unknown;
  /** A file on the daemon's machine: the HTML file opened, or the page R saved. */
  path?: string;
  /** A web address, such as a Shiny app's. */
  url?: string;
}

/**
 * Whether an output is a web page rather than HTML for the Console (a
 * table): it names a page, runs scripts (htmlwidgets, plotly), says it
 * wants a frame of its own (IRkernel's `isolated`), or is large enough to
 * be kept as a blob. `is_page` in `crates/daemon/src/outputs.rs` says the
 * same; `tests/fixtures/pages.json` keeps them in step.
 */
export function isPage(data: Record<string, unknown>, metadata?: Record<string, any> | null): boolean {
  if (PAGE_MIME in data || PLOTLY_MIME in data) return true;
  const html = data["text/html"];
  if (html === undefined) return false;
  if (isBlobRef(html) || metadata?.["text/html"]?.isolated === true) return true;
  return /<script/i.test(htmlText(html) ?? "");
}

/**
 * Whether the Viewer shows an output of a run from `file` (none for the
 * Console): a page from the Console or a script, or an app's address from
 * anywhere, but not a web page opened in the browser.
 */
export function forViewer(file: string | undefined, data: Record<string, unknown>, metadata?: Record<string, any> | null): boolean {
  // A web page R opened in the browser.
  if (named(data)?.browser === true) return false;
  if (file && !isScript(file)) return typeof named(data)?.url === "string";
  return isPage(data, metadata);
}

interface Source {
  run: string;
  session: string;
  cell?: CellRef;
  by?: string;
}

/** A page from an output's MIME bundle; nothing if the Viewer doesn't show it. */
export function pageItem(
  source: Source,
  tsMs: number,
  exact: boolean,
  data: Record<string, unknown>,
  metadata?: Record<string, any> | null,
): PageItem | undefined {
  if (!forViewer(source.cell?.file, data, metadata)) return undefined;
  const page = named(data);
  const html = data["text/html"];
  const url = typeof page?.url === "string" ? page.url : undefined;
  const text = htmlText(html);
  // A figure as data, unless the kernel sent its page too.
  const plotly = !url && html === undefined ? data[PLOTLY_MIME] : undefined;
  const id = url ?? imageId(plotly ?? (isBlobRef(html) ? html : text));
  return {
    key: `${source.run}:${id}`,
    run: source.run,
    session: source.session,
    tsMs,
    exact,
    cell: source.cell,
    by: source.by,
    title: (typeof page?.title === "string" && page.title) || (text && titleOf(text)) || (PLOTLY_MIME in data ? "plotly" : "Page"),
    html: url || plotly !== undefined ? undefined : typeof html === "string" ? html : (text ?? html),
    plotly,
    path: typeof page?.path === "string" ? page.path : undefined,
    url,
  };
}

/** A page `session_pages` returned. */
export function fromJournal(session: string, page: Plot): PageItem | undefined {
  return pageItem({ run: page.run, session, cell: page.cell, by: page.by }, page.ts_ms, true, page.data ?? {}, page.metadata);
}

/** The pages a run has shown so far. */
export function fromRun(run: RunState): PageItem[] {
  if (run.quiet) return [];
  const source = { run: run.id, session: run.session, cell: run.cell, by: run.by };
  return run.outputs
    .filter(isDisplay)
    .map((o) => pageItem(source, run.startedMs, false, o.data ?? {}, o.metadata))
    .filter((p): p is PageItem => !!p);
}

/** How many pages a run has shown. */
export function pageCount(run: RunState): number {
  return fromRun(run).length;
}

/** The web pages a run asked to open in the browser (R's `browseURL()`). */
export function browserPages(run: RunState): string[] {
  return run.outputs
    .filter(isDisplay)
    .map((o) => named(o.data ?? {}))
    .filter((p) => p?.browser === true && typeof p.url === "string")
    .map((p) => p!.url as string);
}

/**
 * Watches runs, opening each web page a run asks for once, as it asks. A
 * run first seen part way (as the window opens) has opened its pages before.
 */
export function browserOpener(open: (url: string) => void): (run: RunState) => void {
  const opened = new Map<string, Set<string>>();
  return (run) => {
    const urls = browserPages(run);
    const seen = opened.get(run.id);
    if (!seen) opened.set(run.id, new Set(urls));
    else {
      for (const url of urls) {
        if (seen.has(url)) continue;
        seen.add(url);
        open(url);
      }
    }
    if (run.phase === "done") opened.delete(run.id);
  };
}

/** An app's address opened from a link, as in the Terminal. */
export function urlItem(url: string, nowMs: number): PageItem {
  return { key: `url:${url}`, tsMs: nowMs, exact: true, title: url, url };
}

/** An HTML file opened from the file tree. */
export function fileItem(path: string, nowMs: number): PageItem {
  return { key: `file:${path}`, tsMs: nowMs, exact: true, title: path.slice(path.lastIndexOf("/") + 1), path };
}

/** Whether a file is a web page the Viewer opens. */
export function isHtmlFile(path: string): boolean {
  return /\.html?$/i.test(path);
}

/**
 * A web address on the daemon's machine (`localhost`, `127.0.0.1`), which
 * the app reaches through the daemon's connection: its port, else none.
 * `is_local` in `crates/daemon/src/viewer.rs` says the same.
 */
export function localPort(url: string): number | undefined {
  try {
    const u = new URL(url);
    if (u.protocol !== "http:" || !["localhost", "127.0.0.1", "[::1]", "0.0.0.0"].includes(u.hostname)) return undefined;
    return Number(u.port || 80);
  } catch {
    return undefined;
  }
}

/** The same address, at `port` on this Mac's 127.0.0.1. */
export function atPort(url: string, port: number): string {
  const u = new URL(url);
  u.hostname = "127.0.0.1";
  u.port = String(port);
  return u.toString();
}

/**
 * A page that draws a plotly figure: filling its frame (the Viewer's), or
 * as tall as the figure asks (a notebook's, which takes the page's height).
 */
export function plotlyPage(figure: unknown, fill: boolean): string {
  const fig = (typeof figure === "object" && figure !== null ? figure : {}) as Record<string, any>;
  const layout = { ...(fig.layout ?? {}) };
  const height = fill ? "100vh" : `${typeof layout.height === "number" ? layout.height : 450}px`;
  if (fill) delete layout.height;
  // `</script` can't end the script it is in.
  const json = (value: unknown) => JSON.stringify(value ?? null).replace(/<\//g, "<\\/");
  return (
    `<!doctype html><html><head><meta charset="utf-8"><style>html,body{margin:0}#figure{width:100%;height:${height}}</style>` +
    `<script src="${PLOTLY_JS}"></script></head><body><div id="figure"></div><script>` +
    `if (window.Plotly) Plotly.newPlot("figure", ${json(fig.data ?? [])}, ${json(layout)}, Object.assign({responsive: true}, ${json(fig.config ?? {})}));` +
    `else document.body.textContent = "Could not load plotly.js, which draws this figure, from cdn.plot.ly.";` +
    `</script></body></html>`
  );
}

/**
 * A page's `<title>`, else the kind of htmlwidget it holds (`datatables`,
 * `plotly`), else whether it is a plotly figure, as the daemon names pages
 * (`crates/daemon/src/viewer.rs`).
 */
export function titleOf(html: string): string | undefined {
  const title = /<title[^>]*>([^<]*)<\/title>/i.exec(html)?.[1].trim();
  if (title) return title;
  // `<div class="datatables html-widget ..."`
  const widget = /class\s*=\s*["'][^"']*?([\w.-]+)\s+html-widget\b/i.exec(html)?.[1];
  return widget ?? (/plotly-graph-div/i.test(html) ? "plotly" : undefined);
}

function named(data: Record<string, unknown>): Record<string, unknown> | undefined {
  const page = data[PAGE_MIME];
  return typeof page === "object" && page !== null ? (page as Record<string, unknown>) : undefined;
}

/** HTML as text, from nbformat's string or list of lines; none for a blob. */
function htmlText(html: unknown): string | undefined {
  if (typeof html === "string") return html;
  if (Array.isArray(html)) return html.filter((line) => typeof line === "string").join("");
  return undefined;
}

function isDisplay(output: Output): boolean {
  return output.output_type === "display_data" || output.output_type === "execute_result";
}
