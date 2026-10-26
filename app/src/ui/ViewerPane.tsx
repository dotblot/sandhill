import { createEffect, createMemo, createResource, createSignal, type JSX, Match, on, onCleanup, Show, Switch, untrack } from "solid-js";
import { isBlobRef, type PageUrl, type PreviewInfo, type SessionInfo, type SessionPlots } from "../backend/types";
import { ago } from "../docs/cellOutput";
import { atPort, fileItem, fromJournal, fromRun, localPort, MAX_PAGES, type PageItem, pageCount, plotlyPage, previewItem, urlItem } from "../model/pages";
import { afterClear, mergePlots, plotSource } from "../model/plots";
import type { Workspace } from "../model/workspace";
import { ask, notify } from "./dialogs";
import { Icon } from "./icons";

/** Pages read from each session's journal when the pane opens. */
const LOAD = 20;
/** HTML files opened from the tree that the pane remembers. */
const FILES = 20;

/**
 * A page to bring to the front: an HTML file to open, an app's address (a
 * link in the Terminal), a Quarto document to preview, or a page shown, by
 * key.
 */
export type ViewerRequest = { path: string } | { url: string } | { preview: string } | { key: string };

/**
 * Pages run in a frame with scripts but never with the app's origin: an
 * output's HTML in a frame of no origin at all, and a served file or an app
 * in one of its own address (`127.0.0.1` and a port), which can't reach the
 * app either.
 */
const OUTPUT_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups allow-popups-to-escape-sandbox";
const SERVED_SANDBOX = `${OUTPUT_SANDBOX} allow-same-origin allow-downloads`;

/**
 * The Viewer: web pages from the project's R and Python sessions
 * (htmlwidgets such as DT, plotly and leaflet, shown from the Console and
 * scripts), apps, and HTML reports opened from the file tree (MultiQC,
 * FastQC, a rendered Quarto document), newest last. A new page comes to
 * the front as it is shown; the arrows go back through the rest.
 */
export function ViewerPane(props: {
  ws: Workspace;
  project: string;
  /** The project's live sessions; their pages show. */
  sessions: SessionInfo[];
  shortName: (path: string) => string;
  classList?: Record<string, boolean>;
  style?: JSX.CSSProperties;
  /** The Plots and Viewer tabs. */
  tabs: JSX.Element;
  /** The minimize and maximize buttons. */
  buttons: JSX.Element;
  /** A page to show; `n` changes each time. */
  request?: ViewerRequest & { n: number };
  /** A page was just shown. */
  onNew?: () => void;
}) {
  const ws = props.ws;
  const filesKey = `sandhill.viewerFiles:${props.project}`;
  const clearKey = `sandhill.viewerCleared:${props.project}`;
  const [pages, setPages] = createSignal<PageItem[]>(storedFiles(filesKey));
  /** The page in front, by key; none for the newest. */
  const [picked, setPicked] = createSignal<string>();
  const [cleared, setCleared] = createSignal<number | undefined>(Number(stored(clearKey)) || undefined);
  /** Bumped by Reload. */
  const [reload, setReload] = createSignal(0);
  /** Bumped as the run of the page in front goes, for Stop. */
  const [runTick, setRunTick] = createSignal(0);
  /** The terminals running documents' previews, by page key, for Stop. */
  const [previews, setPreviews] = createSignal<Record<string, string>>({});
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), 30_000);
  onCleanup(() => clearInterval(clock));

  const ids = createMemo(
    () => props.sessions.map((s) => s.id),
    [],
    { equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]) },
  );
  const shown = createMemo(() => {
    const live = new Set(ids());
    return afterClear(
      pages().filter((p) => !p.session || live.has(p.session)),
      cleared(),
    );
  });
  const index = () => {
    const list = shown();
    const key = picked();
    const at = key ? list.findIndex((p) => p.key === key) : -1;
    return at >= 0 ? at : list.length - 1;
  };
  const item = () => shown()[index()];
  const pick = (at: number) => {
    const list = shown();
    const i = Math.max(0, Math.min(list.length - 1, at));
    setPicked(i === list.length - 1 ? undefined : list[i]?.key);
  };
  const step = (by: number) => pick(index() + by);
  /** The terminal of the preview in front, which Stop closes. */
  const previewTerminal = () => {
    const p = item();
    return p?.preview ? previews()[p.key] : undefined;
  };
  /** The run of the app in front while it still runs, which Stop ends. */
  const appRun = () => {
    runTick();
    const p = item();
    const run = p?.url && p.run ? ws.runs.get(p.run) : undefined;
    return run && run.phase !== "done" ? run : undefined;
  };

  /** Adds pages; with `jump`, a new one comes to the front. */
  const add = (items: PageItem[], jump: boolean) => {
    const before = pages();
    const next = mergePlots(before, items, MAX_PAGES);
    if (next === before) return;
    setPages(next);
    if (!jump) return;
    const known = new Set(before.map((p) => p.key));
    if (!items.some((p) => !known.has(p.key))) return;
    setPicked(undefined);
    props.onNew?.();
  };

  /** Reads a session's latest pages from the daemon's journal, and follows the session. */
  async function load(session: string, limit: number) {
    const read = await ws.call<SessionPlots>("session_pages", { session, limit });
    add(
      read.plots.map((p) => fromJournal(session, p)).filter((p): p is PageItem => !!p),
      false,
    );
    await ws.subscribe(session, read.seq);
  }

  // Each live session's pages, read once per connection.
  const loaded = new Set<string>();
  let connection: number | undefined;
  createEffect(() => {
    const since = ws.connectedSince();
    const list = ids();
    untrack(() => {
      if (since !== connection) {
        connection = since;
        if (since !== undefined) loaded.clear();
      }
      for (const id of list) {
        if (loaded.has(id)) continue;
        loaded.add(id);
        load(id, LOAD).catch((err) => {
          loaded.delete(id);
          console.warn("could not read the pages of", id, err);
        });
      }
    });
  });

  // Pages as runs show them. Once a run that showed some is done, they are
  // read again from the journal, for the time each was shown.
  const showing = new Set<string>();
  onCleanup(
    ws.runs.watchAll((run) => {
      if (run.id === untrack(item)?.run) setRunTick((n) => n + 1);
      if (run.quiet || !untrack(ids).includes(run.session)) return;
      if (run.phase !== "done") {
        const found = fromRun(run);
        if (!found.length) return;
        showing.add(run.id);
        add(found, true);
        return;
      }
      add(fromRun(run), false);
      if (showing.delete(run.id)) void load(run.session, Math.max(1, pageCount(run))).catch(() => {});
    }),
  );

  // A file to open, or a page to bring to the front.
  createEffect(
    on(
      () => props.request,
      (request) => {
        if (!request) return;
        if ("path" in request) {
          const file = fileItem(request.path, Date.now());
          add([file], false);
          setPicked(undefined);
          saveFiles(filesKey, pages());
        } else if ("url" in request) {
          add([urlItem(request.url, Date.now())], false);
          setPicked(undefined);
        } else if ("preview" in request) {
          add([previewItem(request.preview, Date.now())], false);
          setPicked(undefined);
          // Asks again: a preview that stopped starts again.
          setReload((n) => n + 1);
        } else if (pages().some((p) => p.key === request.key)) {
          setPicked(request.key);
          // The newest page is the one in front when none is picked.
          if (shown().at(-1)?.key === request.key) setPicked(undefined);
        }
        props.onNew?.();
      },
    ),
  );

  async function clear() {
    const list = shown();
    if (!list.length) return;
    const n = list.length;
    const choice = await ask(`Clear ${n === 1 ? "the page" : `all ${n} pages`} from the Viewer?`, ["Clear", "Cancel"], "Files you opened stay where they are.");
    if (choice !== "Clear") return;
    const last = Math.max(...list.map((p) => p.tsMs));
    setCleared(last);
    store(clearKey, String(last));
    setPages((all) => all.filter((p) => p.run));
    saveFiles(filesKey, []);
    setPicked(undefined);
  }

  async function openInBrowser() {
    const p = item();
    if (!p) return;
    try {
      ws.backend.openLink(await pageAddress(ws, p, props.project));
    } catch (err) {
      notify(`Could not open the page: ${message(err)}`);
    }
  }

  async function stop() {
    const run = appRun();
    const terminal = previewTerminal();
    const key = item()?.key;
    try {
      if (run) await ws.interrupt(run.session);
      else if (terminal) {
        await ws.call("close_terminal", { terminal });
        setPreviews((all) => Object.fromEntries(Object.entries(all).filter(([k]) => k !== key)));
      }
    } catch (err) {
      notify(`Could not stop ${run ? "the app" : "the preview"}: ${message(err)}`);
    }
  }
  const previewed = (key: string, info: PreviewInfo) => setPreviews((all) => ({ ...all, [key]: info.terminal.id }));

  const source = (p: PageItem) => (p.run ? plotSource(p, props.shortName) : props.shortName(p.preview ?? p.path ?? p.title));
  const label = (p: PageItem) =>
    p.run ? `${source(p)} · ${p.url ?? p.title}` : p.preview ? `Quarto Preview · ${source(p)}` : (p.url ?? source(p));
  const where = (p: PageItem) => p.url ?? p.preview ?? p.path ?? p.title;

  const onKey = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "ArrowRight") step(1);
    else return;
    e.preventDefault();
  };

  return (
    <div class="pane plots-pane viewer-pane" classList={props.classList} style={props.style}>
      <div class="tabs" role="tablist">
        {props.tabs}
        <span class="spacer" />
        <button class="tab-btn" title="Back" disabled={index() <= 0} onClick={() => step(-1)}>
          <Icon name="chevronLeft" small />
        </button>
        <button class="tab-btn" title="Forward" disabled={index() >= shown().length - 1} onClick={() => step(1)}>
          <Icon name="chevronRight" small />
        </button>
        <button class="tab-btn" title="Reload the page" disabled={!item()} onClick={() => setReload((n) => n + 1)}>
          <Icon name="refresh" small />
        </button>
        <button class="tab-btn" title="Open in browser" disabled={!item()} onClick={() => void openInBrowser()}>
          <Icon name="open" small />
        </button>
        <Show when={appRun() || previewTerminal()}>
          <button class="tab-btn" title={appRun() ? "Stop the app" : "Stop the preview"} onClick={() => void stop()}>
            <Icon name="stop" small />
          </button>
        </Show>
        <button class="tab-btn" title="Clear the Viewer" disabled={!shown().length} onClick={() => void clear()}>
          <Icon name="trash" small />
        </button>
        {props.buttons}
      </div>
      <div class="viewer" tabIndex={0} onKeyDown={onKey}>
        <Show
          when={item()}
          fallback={
            <p class="vempty">
              Web pages show here: htmlwidgets such as DT, plotly and leaflet from the Console and scripts, Shiny and other apps, and HTML
              reports you open from the file tree. Notebooks' pages stay under their cells.
            </p>
          }
        >
          {(p) => (
            <>
              <div class="viewer-address">
                <span class="viewer-where" title={where(p())}>
                  {label(p())}
                </span>
                <span class="plot-when">
                  {index() + 1} of {shown().length} · {ago(Math.max(0, now() - p().tsMs))}
                </span>
              </div>
              <PageFrame ws={ws} item={p()} reload={reload()} project={props.project} onPreview={previewed} />
            </>
          )}
        </Show>
      </div>
    </div>
  );
}

/** One page, in a frame that fills the pane. */
function PageFrame(props: {
  ws: Workspace;
  item: PageItem;
  reload: number;
  project: string;
  /** A document's preview answered. */
  onPreview: (key: string, info: PreviewInfo) => void;
}) {
  // Loaded again for another page or a Reload, not when the journal's copy
  // of the page replaces the one seen as its run went.
  const [frame] = createResource(
    () => `${props.item.key}\n${props.reload}`,
    () => {
      const item = untrack(() => props.item);
      return frameSource(props.ws, item, props.project, (info) => props.onPreview(item.key, info));
    },
  );
  return (
    <Switch>
      <Match when={frame.loading && props.item.preview}>
        <p class="vempty">Rendering {props.item.title} with Quarto…</p>
      </Match>
      <Match when={frame.error && props.item.preview}>
        <div class="vempty viewer-failed">
          <p>Could not preview {props.item.title}.</p>
          <pre>{message(frame.error)}</pre>
        </div>
      </Match>
      <Match when={frame.error}>
        <p class="vempty">Could not show the page: {message(frame.error)}</p>
      </Match>
      <Match when={frame()}>
        {(f) => (
          <Show
            when={f().srcdoc !== undefined}
            fallback={<iframe class="viewer-frame" title={props.item.title} sandbox={SERVED_SANDBOX} src={f().src} />}
          >
            <iframe class="viewer-frame" title={props.item.title} sandbox={OUTPUT_SANDBOX} srcdoc={f().srcdoc} />
          </Show>
        )}
      </Match>
    </Switch>
  );
}

/** What a page's frame shows: an output's HTML, or an address. */
async function frameSource(
  ws: Workspace,
  item: PageItem,
  project: string,
  onPreview: (info: PreviewInfo) => void,
): Promise<{ srcdoc?: string; src?: string }> {
  const html = await outputHtml(ws, item);
  return html !== undefined ? { srcdoc: wholePage(html) } : { src: await pageAddress(ws, item, project, onPreview) };
}

/**
 * Where a page loads from on this Mac: an app's address, a document's
 * preview (started if it isn't running), or the daemon's page server; a
 * server's ports are reached through the daemon's connection.
 */
export async function pageAddress(ws: Workspace, item: PageItem, project?: string, onPreview?: (info: PreviewInfo) => void): Promise<string> {
  if (item.preview) {
    const info = await ws.call<PreviewInfo>("preview_document", { path: item.preview, project });
    onPreview?.(info);
    return atPort(info.url, await reach(ws, localPort(info.url) ?? 0));
  }
  if (item.url) {
    const port = localPort(item.url);
    return port === undefined ? item.url : atPort(item.url, await reach(ws, port));
  }
  const served = isBlobRef(item.html)
    ? await ws.call<PageUrl>("serve_html", { blob: item.html.sandhill_blob })
    : item.html !== undefined || item.plotly !== undefined
      ? await ws.call<PageUrl>("serve_html", { html: await outputHtml(ws, item) })
      : await ws.call<PageUrl>("serve_page", { path: item.path });
  return `http://127.0.0.1:${await reach(ws, served.port)}${served.path}`;
}

function reach(ws: Workspace, port: number): Promise<number> {
  return ws.backend.forwardPort ? ws.backend.forwardPort(port) : Promise.resolve(port);
}

/** An output page's HTML; none for a file or an app. */
async function outputHtml(ws: Workspace, item: PageItem): Promise<string | undefined> {
  const text = async (value: unknown) =>
    isBlobRef(value) ? (await ws.call<{ data: string }>("get_blob", { hash: value.sandhill_blob })).data : value;
  if (item.plotly !== undefined) {
    const figure = await text(item.plotly);
    return plotlyPage(typeof figure === "string" ? JSON.parse(figure) : figure, true);
  }
  return item.html === undefined ? undefined : String(await text(item.html));
}

/** The app's font, for pages that don't name one; theirs come after it. */
const BASE_STYLE = "<style>body{font:14px/1.4 -apple-system,system-ui,sans-serif}</style>";

/**
 * A whole page, with the app's font unless it has its own: a fragment
 * (Python's HTML) gets a doctype too, so it isn't drawn in quirks mode.
 */
export function wholePage(html: string): string {
  const head = /<head\b[^>]*>/i.exec(html);
  if (head) return html.slice(0, head.index + head[0].length) + BASE_STYLE + html.slice(head.index + head[0].length);
  if (/^\s*(<!doctype|<html)/i.test(html)) return html;
  return `<!doctype html><meta charset="utf-8">${BASE_STYLE}${html}`;
}

/** HTML files opened from the tree, as the pane remembers them. */
function storedFiles(key: string): PageItem[] {
  try {
    const list = JSON.parse(stored(key) ?? "[]");
    if (!Array.isArray(list)) return [];
    return list
      .filter((f) => typeof f?.path === "string" && typeof f?.tsMs === "number")
      .map((f) => fileItem(f.path, f.tsMs));
  } catch {
    return [];
  }
}

function saveFiles(key: string, pages: PageItem[]) {
  const files = pages
    .filter((p) => !p.run && p.path)
    .slice(-FILES)
    .map((p) => ({ path: p.path, tsMs: p.tsMs }));
  store(key, JSON.stringify(files));
}

function stored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Private mode; it just isn't remembered.
  }
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
