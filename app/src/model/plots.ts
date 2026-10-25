// The Plots pane's history: every image the project's sessions showed, read
// from the daemon's journal (`session_plots`) and added to as runs show more.

import { type CellRef, isBlobRef, type Output, type Plot } from "../backend/types";
import type { RunState } from "./runs";

/** The image types the pane shows, best first: SVG can be saved at any size. */
export const PLOT_MIMES = ["image/svg+xml", "image/png", "image/jpeg", "image/gif"];

/** How many plots the pane keeps, all sessions together. */
export const MAX_PLOTS = 300;

export interface PlotItem {
  /** The run, then the image: the same plot read twice has the same key. */
  key: string;
  run: string;
  session: string;
  /** When it was shown, by the daemon's clock. */
  tsMs: number;
  /** True when the time is the journal's; plots seen as a run goes have its start. */
  exact: boolean;
  cell?: CellRef;
  by?: string;
  /** Its images by MIME type: blob references, or inline values. */
  images: Record<string, unknown>;
  /** The type shown. */
  mime: string;
  /** The size the kernel asked for, in CSS pixels, if it did. */
  width?: number;
  height?: number;
}

export type PlotFormat = "png" | "svg" | "pdf" | "jpeg" | "gif";

export const EXTENSIONS: Record<PlotFormat, string> = { png: "png", svg: "svg", pdf: "pdf", jpeg: "jpg", gif: "gif" };

/** What a plot its session can draw again is saved as, at any size. */
export const REDRAW_FORMATS: PlotFormat[] = ["png", "svg", "pdf"];

/** Sizes to save an SVG plot at, in pixels at 300 dpi. */
export const PLOT_SIZES = [
  { label: "4 × 3 in", width: 1200, height: 900 },
  { label: "6 × 4 in", width: 1800, height: 1200 },
  { label: "Slide 16:9", width: 1920, height: 1080 },
];

export const MIN_SIZE = 100;
export const MAX_SIZE = 6000;

export function plotMime(data: Record<string, unknown>): string | undefined {
  return PLOT_MIMES.find((m) => m in data);
}

interface Source {
  run: string;
  session: string;
  cell?: CellRef;
  by?: string;
}

/** A plot from an output's MIME bundle; nothing if it holds no image. */
export function plotItem(
  source: Source,
  tsMs: number,
  exact: boolean,
  data: Record<string, unknown>,
  metadata?: Record<string, any> | null,
): PlotItem | undefined {
  const mime = plotMime(data);
  if (!mime) return undefined;
  const images: Record<string, unknown> = {};
  for (const m of PLOT_MIMES) if (m in data) images[m] = data[m];
  const meta = metadata?.[mime] ?? {};
  return {
    key: `${source.run}:${imageId(data[mime])}`,
    run: source.run,
    session: source.session,
    tsMs,
    exact,
    cell: source.cell,
    by: source.by,
    images,
    mime,
    width: typeof meta.width === "number" ? meta.width : undefined,
    height: typeof meta.height === "number" ? meta.height : undefined,
  };
}

/** A plot `session_plots` returned. */
export function fromJournal(session: string, plot: Plot): PlotItem | undefined {
  return plotItem({ run: plot.run, session, cell: plot.cell, by: plot.by }, plot.ts_ms, true, plot.data ?? {}, plot.metadata);
}

/** The plots a run has shown so far. */
export function fromRun(run: RunState): PlotItem[] {
  if (run.quiet) return [];
  const source = { run: run.id, session: run.session, cell: run.cell, by: run.by };
  return run.outputs
    .filter(isDisplay)
    .map((o) => plotItem(source, run.startedMs, false, o.data ?? {}, o.metadata))
    .filter((p): p is PlotItem => !!p);
}

/** How many images a run has shown. */
export function imageCount(run: RunState): number {
  return run.outputs.filter((o) => isDisplay(o) && plotMime(o.data ?? {})).length;
}

function isDisplay(output: Output): boolean {
  return output.output_type === "display_data" || output.output_type === "execute_result";
}

/**
 * Adds plots to the history, or updates ones it has, oldest first. A plot
 * keeps the journal's time once it has it.
 */
export function mergePlots(list: PlotItem[], add: PlotItem[]): PlotItem[] {
  if (!add.length) return list;
  const index = new Map(list.map((p, i) => [p.key, i]));
  const next = [...list];
  let changed = false;
  for (const item of add) {
    const at = index.get(item.key);
    if (at === undefined) {
      index.set(item.key, next.length);
      next.push(item);
      changed = true;
    } else if (!next[at].exact || item.exact) {
      if (sameItem(next[at], item)) continue;
      next[at] = item;
      changed = true;
    }
  }
  if (!changed) return list;
  // Stable: plots of one run keep the order they came in.
  next.sort((a, b) => a.tsMs - b.tsMs);
  return next.length > MAX_PLOTS ? next.slice(next.length - MAX_PLOTS) : next;
}

function sameItem(a: PlotItem, b: PlotItem): boolean {
  return a.tsMs === b.tsMs && a.exact === b.exact && a.by === b.by && a.cell?.cell_id === b.cell?.cell_id;
}

/** Where a plot came from: "Console", an agent, "qc.py", or "analysis.qmd · umap". */
export function plotSource(item: PlotItem, shortName: (path: string) => string): string {
  if (item.cell) {
    const file = shortName(item.cell.file);
    const label = chunkLabel(item.cell);
    return label ? `${file} · ${label}` : file;
  }
  return item.by ?? "Console";
}

/** A Quarto or R Markdown chunk's own label, not one made from its position. */
function chunkLabel(cell: CellRef): string | undefined {
  return /\.(qmd|rmd)$/i.test(cell.file) && !/^chunk-\d+$/.test(cell.cell_id) ? cell.cell_id : undefined;
}

/**
 * A file name for saving a plot, without its extension: its chunk's label,
 * else its file's name, else "plot"; numbered when its run drew several.
 */
export function plotName(item: PlotItem, all: PlotItem[]): string {
  const base = item.cell ? (chunkLabel(item.cell) ?? stem(item.cell.file)) : "plot";
  const siblings = all.filter((p) => p.run === item.run);
  const name = siblings.length > 1 ? `${base}-${siblings.findIndex((p) => p.key === item.key) + 1}` : base;
  return name.replace(/[^\w.-]+/g, "-").replace(/^[-.]+|-+$/g, "") || "plot";
}

function stem(path: string): string {
  return (path.split("/").pop() ?? path).replace(/\.[^.]+$/, "");
}

/** What a plot can be saved as: an SVG plot as PNG too, at any size. */
export function plotFormats(item: PlotItem): PlotFormat[] {
  switch (item.mime) {
    case "image/svg+xml":
      return ["png", "svg"];
    case "image/jpeg":
      return ["jpeg"];
    case "image/gif":
      return ["gif"];
    default:
      return ["png"];
  }
}

/**
 * Which plot this is to the daemon, to draw it again (`redraw_plot`): the
 * blob of one of its images, and else its place among its run's plots.
 */
export function redrawTarget(item: PlotItem, all: PlotItem[]): { image?: string; index: number } {
  const blob = Object.values(item.images).find(isBlobRef);
  const index = all.filter((p) => p.run === item.run).findIndex((p) => p.key === item.key);
  return { image: blob?.sandhill_blob, index: Math.max(0, index) };
}

/** Keeps a typed width or height in range; NaN gives `fallback`. */
export function clampSize(value: number, fallback: number): number {
  if (!Number.isFinite(value) || value <= 0) return fallback;
  return Math.round(Math.max(MIN_SIZE, Math.min(MAX_SIZE, value)));
}

/** A URL for one of a plot's images: the daemon's blob, or the inline value. */
export function plotSrc(item: PlotItem, blobUrl: (hash: string, mime: string) => string, mime = item.mime): string {
  const value = item.images[mime];
  if (isBlobRef(value)) return blobUrl(value.sandhill_blob, mime);
  const text = typeof value === "string" ? value : "";
  return mime === "image/svg+xml"
    ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(text)}`
    : `data:${mime};base64,${text.replace(/\s/g, "")}`;
}

/** Plots shown after a clear: `cleared` is the time of the newest plot cleared. */
export function afterClear(list: PlotItem[], cleared: number | undefined): PlotItem[] {
  return cleared === undefined ? list : list.filter((p) => p.tsMs > cleared);
}

/** A short id for an image: its blob's hash, or a hash of the inline value. */
function imageId(value: unknown): string {
  if (isBlobRef(value)) return value.sandhill_blob;
  const text = typeof value === "string" ? value : JSON.stringify(value);
  // FNV-1a, enough to tell a run's images apart.
  let hash = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    hash ^= text.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return `${(hash >>> 0).toString(16)}-${text.length}`;
}
