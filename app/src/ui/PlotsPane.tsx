import { createEffect, createMemo, createSignal, For, type JSX, on, onCleanup, Show, untrack } from "solid-js";
import { Portal } from "solid-js/web";
import type { FileStat, PlotRedraw, SessionInfo, SessionPlots } from "../backend/types";
import { ago } from "../docs/cellOutput";
import { documentPath } from "../model/agents";
import {
  afterClear,
  clampSize,
  EXTENSIONS,
  fromJournal,
  fromRun,
  imageCount,
  mergePlots,
  PLOT_SIZES,
  type PlotFormat,
  type PlotItem,
  plotFormats,
  plotName,
  plotSource,
  plotSrc,
  REDRAW_FORMATS,
  redrawTarget,
} from "../model/plots";
import { fromNotebook } from "../model/runs";
import type { Workspace } from "../model/workspace";
import { ask, askText, notify } from "./dialogs";
import { Icon } from "./icons";
import { copyPng, DPI, fit, naturalSize, plotFile, plotPng } from "./plotImage";

/** Plots read from each session's journal when the pane opens. */
const LOAD = 100;
/** Where plots are saved unless the user saved somewhere else last. */
const FOLDER = "figures";

type Size = { width: number; height: number };

/**
 * The Plots pane: every plot the project's R and Python sessions drew, from
 * the Console, scripts and agents, and from notebooks when the user asks for
 * them, newest last. A new plot comes to the front as it is drawn; the arrows
 * and thumbnails go back through the rest. The magnifier opens a plot large,
 * to copy it or save it as a file.
 */
export function PlotsPane(props: {
  ws: Workspace;
  project: string;
  /** The project's live sessions; their plots show. */
  sessions: SessionInfo[];
  /** Whether notebook and Quarto cells' plots show too; they are under their cells either way. */
  notebooks: boolean;
  shortName: (path: string) => string;
  /** The machine files are saved on, like "gpu01". */
  hostLabel: string;
  classList?: Record<string, boolean>;
  style?: JSX.CSSProperties;
  /** The minimize and maximize buttons. */
  buttons: JSX.Element;
}) {
  const ws = props.ws;
  const [plots, setPlots] = createSignal<PlotItem[]>([]);
  /** The plot in front, by key; none for the newest. */
  const [picked, setPicked] = createSignal<string>();
  const clearKey = `sandhill.plotsCleared:${props.project}`;
  const [cleared, setCleared] = createSignal<number | undefined>(Number(stored(clearKey)) || undefined);
  const [zoomed, setZoomed] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), 30_000);
  onCleanup(() => clearInterval(clock));

  const ids = createMemo(
    () => props.sessions.map((s) => s.id),
    [],
    { equals: (a, b) => a.length === b.length && a.every((id, i) => id === b[i]) },
  );
  const wanted = (p: PlotItem) => props.notebooks || !fromNotebook(p.cell);
  const shown = createMemo(() => {
    const live = new Set(ids());
    return afterClear(
      plots().filter((p) => live.has(p.session) && wanted(p)),
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
  const source = (p: PlotItem) => plotSource(p, props.shortName);
  const when = (p: PlotItem) => ago(Math.max(0, now() - p.tsMs));
  const blobUrl = (p: PlotItem) => (hash: string, mime: string) => ws.backend.blobUrl(hash, mime, p.cell?.file);

  /** Adds plots; with `jump`, a new one comes to the front. */
  const add = (items: PlotItem[], jump: boolean) => {
    const before = plots();
    const next = mergePlots(before, items);
    if (next === before) return;
    setPlots(next);
    if (!jump) return;
    const known = new Set(before.map((p) => p.key));
    if (items.some((p) => !known.has(p.key) && wanted(p))) setPicked(undefined);
  };

  /** Reads a session's latest plots from the daemon's journal, and follows the session. */
  async function load(session: string, limit: number) {
    const read = await ws.call<SessionPlots>("session_plots", { session, limit });
    add(
      read.plots.map((p) => fromJournal(session, p)).filter((p): p is PlotItem => !!p),
      false,
    );
    await ws.subscribe(session, read.seq);
  }

  // Each live session's plots, read once per connection.
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
          console.warn("could not read the plots of", id, err);
        });
      }
    });
  });

  // Plots as runs draw them. Once a run that drew some is done, they are read
  // again from the journal, for the time each was drawn.
  const drawing = new Set<string>();
  onCleanup(
    ws.runs.watchAll((run) => {
      if (run.quiet || !untrack(ids).includes(run.session)) return;
      if (run.phase !== "done") {
        const found = fromRun(run);
        if (!found.length) return;
        drawing.add(run.id);
        add(found, true);
        return;
      }
      add(fromRun(run), false);
      if (drawing.delete(run.id)) void load(run.session, Math.max(1, imageCount(run))).catch(() => {});
    }),
  );

  async function clear() {
    const list = shown();
    if (!list.length) return;
    const n = list.length;
    const choice = await ask(
      `Clear ${n === 1 ? "the plot" : `all ${n} plots`} from this pane?`,
      ["Clear", "Cancel"],
      "They stay in the Console and in your documents' outputs.",
    );
    if (choice !== "Clear") return;
    const last = Math.max(...list.map((p) => p.tsMs));
    setCleared(last);
    store(clearKey, String(last));
    setPicked(undefined);
    setZoomed(false);
  }

  let mainImg: HTMLImageElement | undefined;
  /** Copies the plot in front as a PNG; an SVG one at twice its size, so it stays sharp. */
  async function copy() {
    const p = item();
    if (!p) return;
    const natural = (mainImg && naturalSize(mainImg)) ?? { width: 800, height: 600 };
    const scale = p.mime === "image/svg+xml" ? 2 : 1;
    try {
      await copyPng(plotPng(ws, p, { width: natural.width * scale, height: natural.height * scale }));
      notify("Copied the plot.", "info");
    } catch (err) {
      notify(`Could not copy the plot: ${message(err)}`);
    }
  }

  // The thumbnail of the plot in front stays in view.
  let strip: HTMLDivElement | undefined;
  createEffect(
    on(index, (at) => {
      const thumb = strip?.children[at] as HTMLElement | undefined;
      thumb?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    }),
  );

  // The main plot fits the room it has, never larger than drawn unless it is an SVG.
  const [room, setRoom] = createSignal<Size>({ width: 0, height: 0 });
  const [natural, setNatural] = createSignal<Size>();
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(([e]) => setRoom({ width: e.contentRect.width, height: e.contentRect.height }));
  onCleanup(() => observer?.disconnect());
  const mainSize = () => {
    const p = item();
    const n = natural();
    if (!p || !n || !room().width) return undefined;
    // A kernel's high-resolution PNG asks to be shown at half its pixels.
    const drawn = p.width && p.height ? { width: p.width, height: p.height } : n;
    return fit(drawn, room(), p.mime === "image/svg+xml");
  };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "ArrowLeft") step(-1);
    else if (e.key === "ArrowRight") step(1);
    else if (e.key === "Enter" && item()) setZoomed(true);
    else return;
    e.preventDefault();
  };

  return (
    <div class="pane plots-pane" classList={props.classList} style={props.style}>
      <div class="tabs" role="tablist">
        <div class="cap tabcap">
          <div class="tab active" role="tab" aria-selected="true">
            <span class="tab-name">Plots</span>
          </div>
        </div>
        <span class="spacer" />
        <button class="tab-btn" title="Previous plot (←)" disabled={index() <= 0} onClick={() => step(-1)}>
          <Icon name="chevronLeft" small />
        </button>
        <button class="tab-btn" title="Next plot (→)" disabled={index() >= shown().length - 1} onClick={() => step(1)}>
          <Icon name="chevronRight" small />
        </button>
        <button class="tab-btn" title="Zoom, copy or save" disabled={!item()} onClick={() => setZoomed(true)}>
          <Icon name="zoom" small />
        </button>
        <button class="tab-btn" title="Copy as PNG" disabled={!item()} onClick={() => void copy()}>
          <Icon name="copy" small />
        </button>
        <button class="tab-btn" title="Save as PNG or SVG…" disabled={!item()} onClick={() => setZoomed(true)}>
          <Icon name="save" small />
        </button>
        <button class="tab-btn" title="Clear the plots" disabled={!shown().length} onClick={() => void clear()}>
          <Icon name="trash" small />
        </button>
        {props.buttons}
      </div>
      <div class="plots" tabIndex={0} onKeyDown={onKey}>
        <Show
          when={item()}
          fallback={
            <p class="vempty">
              {props.notebooks
                ? "Plots that R and Python draw show here: from the Console, scripts, notebooks and agents."
                : "Plots that R and Python draw show here: from the Console, scripts and agents. Notebooks' plots stay under their cells."}
            </p>
          }
        >
          {(p) => (
            <>
              <div
                class="plot-stage"
                ref={(el) => observer?.observe(el)}
                onDblClick={() => setZoomed(true)}
                title="Double-click to zoom"
              >
                <img
                  ref={mainImg}
                  class="plot-img"
                  src={plotSrc(p(), blobUrl(p()))}
                  alt={`Plot from ${source(p())}`}
                  style={mainSize() ? { width: `${mainSize()!.width}px`, height: `${mainSize()!.height}px` } : { visibility: "hidden" }}
                  onLoad={(e) => setNatural(naturalSize(e.currentTarget))}
                  onError={() => setNatural(undefined)}
                />
              </div>
              <div class="plot-meta">
                <span class="plot-from" title={p().cell?.file ?? source(p())}>
                  {source(p())}
                </span>
                <span class="plot-when">
                  {index() + 1} of {shown().length} · {when(p())}
                </span>
              </div>
            </>
          )}
        </Show>
        <Show when={shown().length > 1}>
          <div class="plot-thumbs" ref={strip}>
            <For each={shown()}>
              {(t, i) => (
                <button aria-pressed={i() === index()} title={`${source(t)} · ${when(t)}`} onClick={() => pick(i())}>
                  <img src={plotSrc(t, blobUrl(t))} alt="" loading="lazy" decoding="async" />
                </button>
              )}
            </For>
          </div>
        </Show>
      </div>
      <Show when={zoomed() && item()}>
        {(p) => (
          <PlotZoom
            ws={ws}
            item={p()}
            name={plotName(p(), shown())}
            target={redrawTarget(p(), shown())}
            source={source(p())}
            position={`${index() + 1} of ${shown().length} · ${when(p())}`}
            project={props.project}
            hostLabel={props.hostLabel}
            shortName={props.shortName}
            canBack={index() > 0}
            canOn={index() < shown().length - 1}
            onStep={step}
            onClose={() => setZoomed(false)}
            src={(mime) => plotSrc(p(), blobUrl(p()), mime)}
          />
        )}
      </Show>
    </div>
  );
}

/** A plot large, with its size and format, to copy or save as a file. */
function PlotZoom(props: {
  ws: Workspace;
  item: PlotItem;
  /** A file name for it, without the extension. */
  name: string;
  /** Which plot it is to its session, to draw it again. */
  target: { image?: string; index: number };
  source: string;
  position: string;
  project: string;
  hostLabel: string;
  shortName: (path: string) => string;
  canBack: boolean;
  canOn: boolean;
  onStep: (by: number) => void;
  onClose: () => void;
  src: (mime: string) => string;
}) {
  const folderKey = `sandhill.plotsFolder:${props.project}`;
  /** Whether its session can draw it again, at any size; asked when it opens. */
  const [redraw, setRedraw] = createSignal<PlotRedraw>();
  const redrawn = () => redraw()?.can === true;
  const formats = () => (redrawn() ? REDRAW_FORMATS : plotFormats(props.item));
  const svg = () => props.item.mime === "image/svg+xml";
  const [format, setFormat] = createSignal<PlotFormat>(formats()[0]);
  const [natural, setNatural] = createSignal<Size>();
  const [size, setSize] = createSignal<Size>({ width: 1200, height: 900 });
  const [lock, setLock] = createSignal(true);
  /** What was just done: "Saved ", a path, and its size. */
  const [note, setNote] = createSignal<{ text: string; path?: string; after?: string }>();
  const [working, setWorking] = createSignal(false);

  // Another plot starts over.
  createEffect(
    on(
      () => props.item.key,
      () => {
        setFormat(formats()[0]);
        setNatural(undefined);
        setNote(undefined);
      },
      { defer: true },
    ),
  );
  createEffect(
    on(
      () => props.item.key,
      () => {
        setRedraw(undefined);
        const item = props.item;
        const { image, index } = props.target;
        props.ws
          .call<PlotRedraw | null>("redraw_plot", { session: item.session, run: item.run, image, index })
          .then((answer) => {
            if (props.item.key !== item.key || !answer) return;
            setRedraw(answer);
            if (!formats().includes(format())) setFormat(formats()[0]);
            const drawn = natural();
            if (answer.can && drawn && !svg()) setSize(startSize(drawn));
          })
          .catch(() => {});
      },
    ),
  );

  /** A plot drawn at any size starts at 4 inches wide at 300 dpi, in its own shape. */
  const startSize = (drawn: Size): Size => ({ width: 1200, height: Math.round((1200 * drawn.height) / drawn.width) });
  const loaded = (img: HTMLImageElement) => {
    const drawn = naturalSize(img) ?? { width: props.item.width ?? 800, height: props.item.height ?? 600 };
    setNatural(drawn);
    setSize(svg() || redrawn() ? startSize(drawn) : drawn);
  };
  /** Whether the size can be picked: a plot its session draws again, or an SVG plot saved as PNG. */
  const sized = () => redrawn() || (svg() && format() === "png");
  /** The size of what is saved. */
  const output = () => (sized() ? size() : (natural() ?? size()));
  /** The size of what is copied: a plot drawn again is copied as shown. */
  const copied = () => (svg() && format() === "png" ? size() : (natural() ?? size()));

  const setSide = (side: "width" | "height", typed: string) => {
    const now = size();
    const value = clampSize(Number.parseInt(typed, 10), now[side]);
    const other = side === "width" ? "height" : "width";
    const next = { ...now, [side]: value };
    if (lock()) next[other] = clampSize(Math.round((now[other] * value) / now[side]), now[other]);
    setSize(next);
    setNote(undefined);
  };

  // The preview: the output's shape, fitted to the room in the dialog.
  const [room, setRoom] = createSignal<Size>({ width: 0, height: 0 });
  const observer = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(([e]) => setRoom({ width: e.contentRect.width, height: e.contentRect.height }));
  onCleanup(() => observer?.disconnect());
  const card = () => (natural() && room().width ? fit(output(), room(), true) : undefined);

  async function copy() {
    setWorking(true);
    try {
      await copyPng(plotPng(props.ws, props.item, copied()));
      setNote({ text: `Copied a PNG, ${sizeText(copied())}.` });
    } catch (err) {
      notify(`Could not copy the plot: ${message(err)}`);
    } finally {
      setWorking(false);
    }
  }

  async function save() {
    const folder = stored(folderKey) ?? FOLDER;
    const suggested = `${folder ? `${folder}/` : ""}${props.name}.${EXTENSIONS[format()]}`;
    const typed = (await askText("Save the plot as", suggested, "Save", `In the project's folder on ${props.hostLabel}.`))?.trim();
    if (!typed) return;
    // A typed extension picks the format, if the plot can be saved that way.
    const ext = /\.([a-z]+)$/i.exec(typed)?.[1].toLowerCase();
    const as = formats().find((f) => f === ext || EXTENSIONS[f] === ext) ?? format();
    const path = documentPath(props.project, typed);
    const shown = props.shortName(path);
    setWorking(true);
    try {
      const [stat] = await props.ws.call<FileStat[]>("stat_files", { paths: [path] });
      if (stat?.mtime_ms !== undefined) {
        const choice = await ask(`Replace ${shown}?`, ["Replace", "Cancel"], "A file with this name is already there.");
        if (choice !== "Replace") return;
      }
      if (redrawn()) {
        const { image, index } = props.target;
        const { width, height } = output();
        await props.ws.call<PlotRedraw>("redraw_plot", {
          session: props.item.session,
          run: props.item.run,
          image,
          index,
          file: path,
          format: as,
          width: width / DPI,
          height: height / DPI,
          dpi: DPI,
        });
      } else {
        const file = await plotFile(props.ws, props.item, as, output());
        await props.ws.call("write_file", { path, text: file.text, base64: file.base64 });
      }
      const slash = typed.lastIndexOf("/");
      if (!typed.startsWith("/")) store(folderKey, slash > 0 ? typed.slice(0, slash) : "");
      const after = as === "png" || !redrawn() ? `, ${sizeText(output())}.` : `, ${inches(output())} in.`;
      setNote({ text: "Saved ", path: shown, after: as === "svg" && !redrawn() ? "." : after });
    } catch (err) {
      notify(`Could not save the plot: ${message(err)}`);
    } finally {
      setWorking(false);
    }
  }

  const onKey = (e: KeyboardEvent) => {
    // Keys in the save question are the question's.
    if ((e.target as HTMLElement | null)?.closest?.(".modal-backdrop")) return;
    if (e.key === "Escape") props.onClose();
    else if ((e.target as HTMLElement | null)?.tagName === "INPUT") return;
    else if (e.key === "ArrowLeft" && props.canBack) props.onStep(-1);
    else if (e.key === "ArrowRight" && props.canOn) props.onStep(1);
    else return;
    e.preventDefault();
  };
  window.addEventListener("keydown", onKey);
  onCleanup(() => window.removeEventListener("keydown", onKey));

  const tip = (): JSX.Element => {
    const done = note();
    if (done) {
      return (
        <>
          {done.text}
          <Show when={done.path}>
            <code>{done.path}</code>
          </Show>
          {done.after}
        </>
      );
    }
    if (redrawn()) return "Drawn again by its session at this size, so lines and text stay sharp.";
    if (sized()) return "Drawn from the plot's SVG at this size, so lines and text stay sharp.";
    if (svg()) return "Saved as the SVG the kernel drew; it scales to any size.";
    const why = redraw()?.reason;
    return `Saved as the kernel drew it${why ? `, since ${why}` : ""}. To save it larger, draw it again larger in R or Python.`;
  };

  return (
    <Portal>
      <div class="pzoom" role="dialog" aria-label="Plot" onClick={(e) => e.target === e.currentTarget && props.onClose()}>
        <div class="pz-card">
          <div class="pz-h">
            <b>{props.source}</b>
            <span class="pz-sub">{props.position}</span>
            <span class="spacer" />
            <Show when={natural()}>{(n) => <span class="pz-sub">{sizeText(n())} as drawn</span>}</Show>
            <button class="ib" title="Previous plot (←)" disabled={!props.canBack} onClick={() => props.onStep(-1)}>
              <Icon name="chevronLeft" small />
            </button>
            <button class="ib" title="Next plot (→)" disabled={!props.canOn} onClick={() => props.onStep(1)}>
              <Icon name="chevronRight" small />
            </button>
            <button class="ib" title="Close (esc)" onClick={() => props.onClose()}>
              <Icon name="x" small />
            </button>
          </div>
          <div class="pz-stage" ref={(el) => observer?.observe(el)}>
            <div class="pz-plot" style={card() ? { width: `${card()!.width}px`, height: `${card()!.height}px` } : { visibility: "hidden" }}>
              <img src={props.src(props.item.mime)} alt={`Plot from ${props.source}`} onLoad={(e) => loaded(e.currentTarget)} />
            </div>
          </div>
          <div class="pz-foot">
            <Show
              when={sized()}
              fallback={
                <Show when={natural()}>
                  {(n) => (
                    <span class="pz-unit">
                      <span class="pz-l">Size</span> {format() === "svg" ? "any" : sizeText(n())}
                    </span>
                  )}
                </Show>
              }
            >
              <span class="pz-l">Size</span>
              <label class="pz-num">
                <input
                  value={size().width}
                  inputmode="numeric"
                  aria-label="Width in pixels"
                  onChange={(e) => setSide("width", e.currentTarget.value)}
                />
                <span>W</span>
              </label>
              <button
                class="ib pz-lock"
                classList={{ on: lock() }}
                aria-pressed={lock()}
                title={lock() ? "Keeping its shape" : "Keep its shape"}
                onClick={() => setLock(!lock())}
              >
                <Icon name="lock" small />
              </button>
              <label class="pz-num">
                <input
                  value={size().height}
                  inputmode="numeric"
                  aria-label="Height in pixels"
                  onChange={(e) => setSide("height", e.currentTarget.value)}
                />
                <span>H</span>
              </label>
              <span class="pz-unit">
                px · {inches(size())} in{format() === "png" ? ` at ${DPI} dpi` : ""}
              </span>
              <span class="pz-presets">
                <For each={PLOT_SIZES}>
                  {(preset) => (
                    <button
                      classList={{ on: size().width === preset.width && size().height === preset.height }}
                      onClick={() => {
                        setSize({ width: preset.width, height: preset.height });
                        setNote(undefined);
                      }}
                    >
                      {preset.label}
                    </button>
                  )}
                </For>
              </span>
            </Show>
            <span class="spacer" />
            <Show when={formats().length > 1}>
              <span class="pz-seg">
                <For each={formats()}>
                  {(f) => (
                    <button
                      aria-pressed={format() === f}
                      onClick={() => {
                        setFormat(f);
                        setNote(undefined);
                      }}
                    >
                      {f.toUpperCase()}
                    </button>
                  )}
                </For>
              </span>
            </Show>
            <button class="pz-btn" disabled={working() || !natural()} title={redrawn() ? "Copy as PNG, as shown" : "Copy as PNG"} onClick={() => void copy()}>
              <Icon name="copy" small />
              Copy
            </button>
            <button class="pz-btn pri" disabled={working() || !natural()} onClick={() => void save()}>
              <Icon name="save" small />
              Save…
            </button>
          </div>
          <div class="pz-note">{tip()}</div>
        </div>
      </div>
    </Portal>
  );
}

function sizeText(size: Size): string {
  return `${size.width} × ${size.height} px`;
}

function inches(size: Size): string {
  const f = (px: number) => String(Math.round((px / DPI) * 100) / 100);
  return `${f(size.width)} × ${f(size.height)}`;
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
