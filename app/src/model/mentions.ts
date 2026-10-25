// The Agents pane's @ menu (draft 33): the project's files, the open
// document's chunks, the sessions' variables and the recent plots, grouped
// and filtered by what follows the @. A pick becomes a chip in the message
// box and goes with the message as a ref (see `MessageRef`).

import type { MessageRef } from "../backend/types";

export interface Mention {
  ref: MessageRef;
  /** Makes the ref when it is picked: a plot is saved to a file first. */
  make?: () => Promise<MessageRef>;
}

export const MENTION_GROUPS: [MessageRef["kind"], string][] = [
  ["file", "Files"],
  ["chunk", "Chunks"],
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

/** The "@" being typed before the caret, and what follows it, if the menu should be open. */
export function typedMention(text: string, caret: number): { at: number; query: string } | undefined {
  const match = /(^|\s)@([^@\s]*)$/.exec(text.slice(0, caret));
  if (!match) return undefined;
  return { at: caret - match[2].length - 1, query: match[2] };
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
