// The Agents pane's @ menu (draft 33): the project's files, the sessions'
// variables and the recent plots, grouped and filtered by what follows the
// @. After a notebook or Quarto file is added with @, typing # lists its
// cells by number (Huan's call, 2026-11-01). A pick becomes a chip in the
// message box and goes with the message as a ref (see `MessageRef`).

import type { MessageRef } from "../backend/types";
import { parseChunks } from "./qmd";

export interface Mention {
  ref: MessageRef;
  /** Makes the ref when it is picked: a plot is saved to a file first. */
  make?: () => Promise<MessageRef>;
  /** What the menu shows, when not the ref's name: "cell 3". */
  label?: string;
  /** Grey text after it: a cell's first line of code. */
  detail?: string;
}

/** A code cell of a document, as the # menu names it. */
export interface DocCell {
  id: string;
  /** "cell 3" in a notebook (its place among all cells), a chunk's label or "chunk 2" in Quarto. */
  name: string;
  code?: string;
}

export const MENTION_GROUPS: [MessageRef["kind"], string][] = [
  ["file", "Files"],
  ["chunk", "Cells"],
  ["variable", "Variables"],
  ["plot", "Plots"],
];

/** At most this many of each group are shown; typing narrows them. */
export const MENTIONS_SHOWN = 30;

/** Where pasted, dragged and attached files and saved plots go, in the project. */
export const ATTACHMENTS = ".sandhill/attachments";

/** The mentions matching `query` (any part of the name, any case), grouped in menu order. */
export function filterMentions(all: Mention[], query: string): [string, Mention[]][] {
  const q = query.toLowerCase();
  const groups: [string, Mention[]][] = [];
  for (const [kind, label] of MENTION_GROUPS) {
    const hits = all.filter((m) => m.ref.kind === kind && (!q || m.ref.name.toLowerCase().includes(q)));
    if (hits.length) groups.push([label, hits.slice(0, MENTIONS_SHOWN)]);
  }
  return groups;
}

/** The "@" (or "#", for cells) being typed before the caret, and what follows it, if the menu should be open. */
export function typedMention(text: string, caret: number): { at: number; query: string; sign: "@" | "#" } | undefined {
  const match = /(^|\s)([@#])([^@#\s]*)$/.exec(text.slice(0, caret));
  if (!match) return undefined;
  return { at: caret - match[3].length - 1, query: match[3], sign: match[2] as "@" | "#" };
}

/** The documents a message names with @ whose cells # can pick: notebooks, Quarto and R Markdown files. */
export function cellDocuments(refs: MessageRef[]): string[] {
  const paths = refs.filter((r) => r.kind === "file" && r.path && /\.(ipynb|qmd|rmd)$/i.test(r.path)).map((r) => r.path!);
  return [...new Set(paths)];
}

/** A document's code cells, read from its text, named as its open editor names them. */
export function documentCells(path: string, text: string): DocCell[] {
  if (/\.ipynb$/i.test(path)) {
    const nb = JSON.parse(text) as { cells?: { cell_type?: string; id?: string; source?: string | string[] }[] };
    return (nb.cells ?? []).flatMap((cell, i) =>
      cell.cell_type === "code" && cell.id
        ? [{ id: cell.id, name: `cell ${i + 1}`, code: Array.isArray(cell.source) ? cell.source.join("") : (cell.source ?? "") }]
        : [],
    );
  }
  return parseChunks(text).map((c) => ({ id: c.cellId, name: c.label ?? `chunk ${c.index + 1}`, code: c.code }));
}

/** A document's cells as # menu items: "cell 3", with its first line of code. */
export function cellMentions(path: string, name: string, cells: DocCell[]): Mention[] {
  return cells.map((cell) => ({
    ref: { kind: "chunk", name: `${name} › ${cell.name}`, path, cell: cell.id },
    label: cell.name,
    detail: cell.code?.split("\n").find((line) => line.trim())?.trim(),
  }));
}

/** The # menu: the cells matching `query` (a number finds that cell, words its name or code), under their document's name. */
export function filterCells(all: Mention[], query: string): [string, Mention[]][] {
  const q = query.toLowerCase();
  const groups = new Map<string, Mention[]>();
  for (const m of all) {
    const name = m.label ?? m.ref.name;
    const number = /(\d+)$/.exec(name)?.[1];
    const hit = !q || (/^\d+$/.test(q) && number?.startsWith(q)) || `${name} ${m.detail ?? ""}`.toLowerCase().includes(q);
    if (!hit) continue;
    const doc = (m.ref.path ?? "").split("/").pop() ?? "";
    groups.set(doc, [...(groups.get(doc) ?? []), m]);
  }
  return [...groups].map(([doc, list]) => [doc, list.slice(0, MENTIONS_SHOWN)]);
}

/** Whether a ref is already a chip: the same kind and name. */
export function sameRef(a: MessageRef, b: MessageRef): boolean {
  return a.kind === b.kind && a.name === b.name;
}

/** A name for a file saved in the attachments folder: pasted images get the time, as macOS names screenshots. */
export function attachmentName(name: string, now: Date): string {
  const clean = name.replace(/[/\\:]/g, "-").trim();
  if (clean && clean !== "image.png") return clean;
  const pad = (n: number) => String(n).padStart(2, "0");
  const day = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  return `Pasted image ${day} at ${pad(now.getHours())}.${pad(now.getMinutes())}.${pad(now.getSeconds())}.png`;
}
