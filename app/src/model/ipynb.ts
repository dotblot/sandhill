// Reads and writes Jupyter notebooks (nbformat 4). Unknown fields are kept
// as they were, and the output matches what Jupyter itself writes (sorted
// keys, one-space indent, multi-line strings as lists of lines) so saving
// a notebook here doesn't produce noisy diffs.

import type { Output } from "../backend/types";

export type CellType = "code" | "markdown" | "raw";

export interface NbCell {
  id: string;
  cell_type: CellType;
  source: string;
  metadata: Record<string, unknown>;
  /** Code cells only. */
  outputs: Output[];
  execution_count: number | null;
  /** Fields this editor doesn't model (attachments, ...), written back unchanged. */
  extra: Record<string, unknown>;
}

export interface Notebook {
  cells: NbCell[];
  metadata: Record<string, any>;
  nbformat: number;
  nbformat_minor: number;
  extra: Record<string, unknown>;
}

const CELL_KEYS = new Set(["id", "cell_type", "source", "metadata", "outputs", "execution_count"]);
const NOTEBOOK_KEYS = new Set(["cells", "metadata", "nbformat", "nbformat_minor"]);

export function parseNotebook(text: string): Notebook {
  const raw = JSON.parse(text);
  if (!raw || typeof raw !== "object" || !Array.isArray(raw.cells)) {
    throw new Error("not a Jupyter notebook");
  }
  if (raw.nbformat !== 4) {
    throw new Error(`nbformat ${raw.nbformat} notebooks are not supported; convert to version 4`);
  }
  const seen = new Set<string>();
  const cells = raw.cells.map((c: any): NbCell => {
    let id = typeof c.id === "string" && c.id && !seen.has(c.id) ? c.id : newCellId();
    while (seen.has(id)) id = newCellId();
    seen.add(id);
    const cell_type: CellType = c.cell_type === "markdown" || c.cell_type === "raw" ? c.cell_type : "code";
    return {
      id,
      cell_type,
      source: joinLines(c.source),
      metadata: c.metadata ?? {},
      outputs: cell_type === "code" ? (c.outputs ?? []).map(normalizeOutput) : [],
      execution_count: cell_type === "code" ? (c.execution_count ?? null) : null,
      extra: pick(c, (k) => !CELL_KEYS.has(k)),
    };
  });
  return {
    cells,
    metadata: raw.metadata ?? {},
    nbformat: raw.nbformat,
    nbformat_minor: raw.nbformat_minor ?? 0,
    extra: pick(raw, (k) => !NOTEBOOK_KEYS.has(k)),
  };
}

export function serializeNotebook(nb: Notebook): string {
  const cells = nb.cells.map((c) => {
    const out: Record<string, unknown> = {
      ...c.extra,
      id: c.id,
      cell_type: c.cell_type,
      metadata: c.metadata,
      source: splitLines(c.source),
    };
    if (c.cell_type === "code") {
      out.execution_count = c.execution_count;
      out.outputs = c.outputs.map(splitOutput);
    }
    if (c.extra.attachments && typeof c.extra.attachments === "object") {
      out.attachments = Object.fromEntries(
        Object.entries(c.extra.attachments as Record<string, any>).map(([k, v]) => [k, splitBundle(v)]),
      );
    }
    return out;
  });
  const doc = {
    ...nb.extra,
    cells,
    metadata: nb.metadata,
    nbformat: 4,
    // Cell ids need nbformat 4.5.
    nbformat_minor: Math.max(nb.nbformat_minor, 5),
  };
  return JSON.stringify(sortKeys(doc), null, 1) + "\n";
}

export function newNotebook(kernel?: { name: string; display_name: string; language: string }): Notebook {
  return {
    cells: [newCell("code")],
    metadata: kernel
      ? {
          kernelspec: { name: kernel.name, display_name: kernel.display_name, language: kernel.language },
          language_info: { name: kernel.language },
        }
      : {},
    nbformat: 4,
    nbformat_minor: 5,
    extra: {},
  };
}

export function newCell(cell_type: CellType, source = ""): NbCell {
  return { id: newCellId(), cell_type, source, metadata: {}, outputs: [], execution_count: null, extra: {} };
}

/** The notebook's language, from its kernelspec or language_info. */
export function notebookLanguage(nb: Notebook): string | undefined {
  const lang = nb.metadata?.kernelspec?.language ?? nb.metadata?.language_info?.name;
  return typeof lang === "string" ? lang.toLowerCase() : undefined;
}

export function newCellId(): string {
  const bytes = new Uint8Array(4);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function normalizeOutput(o: any): Output {
  const out = { ...o };
  if (out.output_type === "stream") out.text = joinLines(out.text);
  if (out.data && typeof out.data === "object") {
    out.data = Object.fromEntries(
      Object.entries(out.data).map(([mime, v]) => [mime, Array.isArray(v) ? v.join("") : v]),
    );
  }
  return out;
}

function splitOutput(o: Output): Output {
  const out: Output = { ...o };
  if (out.output_type === "stream" && typeof out.text === "string") out.text = splitLines(out.text);
  if (out.output_type === "display_data" || out.output_type === "execute_result") {
    out.data = splitBundle(out.data ?? {});
    out.metadata = out.metadata ?? {};
  }
  return out;
}

/** nbformat splits text/* , application/javascript and SVG values into lines. */
function splitBundle(data: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(data).map(([mime, v]) => {
      const split =
        typeof v === "string" &&
        (mime.startsWith("text/") || mime === "application/javascript" || mime === "image/svg+xml");
      return [mime, split ? splitLines(v as string) : v];
    }),
  );
}

function joinLines(v: unknown): string {
  if (Array.isArray(v)) return v.join("");
  return typeof v === "string" ? v : "";
}

/** Python's str.splitlines(keepends=True), which nbformat uses. */
export function splitLines(s: string): string[] {
  const out: string[] = [];
  const re = /\r\n|[\n\r\x0b\x0c\x1c\x1d\x1e\x85\u2028\u2029]/g;
  let start = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) {
    out.push(s.slice(start, m.index + m[0].length));
    start = m.index + m[0].length;
  }
  if (start < s.length) out.push(s.slice(start));
  return out;
}

function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys);
  if (v && typeof v === "object") {
    return Object.fromEntries(
      Object.keys(v as object)
        .sort()
        .map((k) => [k, sortKeys((v as Record<string, unknown>)[k])]),
    );
  }
  return v;
}

function pick(o: Record<string, unknown>, keep: (k: string) => boolean): Record<string, unknown> {
  return Object.fromEntries(Object.entries(o).filter(([k]) => keep(k)));
}
