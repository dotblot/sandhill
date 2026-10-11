// Finding things for the ⌘K palette: the project's documents, and a loose
// match of what was typed against names.

import type { DirEntry } from "../backend/types";
import { docKind } from "../docs/doc";

/** Folders with nothing to open that can hold many files. */
const SKIP = new Set(["node_modules", "renv", "__pycache__", "site-packages", "venv", "env", "_freeze", "_site"]);
const MAX_DIRS = 400;
const MAX_FILES = 3000;

/**
 * Documents Sandhill opens (.qmd, .Rmd, .ipynb, .R, .py) under `root`,
 * breadth first, skipping hidden folders and library folders, and stopping
 * after a few hundred folders so a huge data folder can't stall it.
 */
export async function listProjectFiles(
  list: (path: string) => Promise<DirEntry[]>,
  root: string,
): Promise<string[]> {
  const files: string[] = [];
  let level = [root.replace(/\/$/, "")];
  let visited = 0;
  while (level.length && visited < MAX_DIRS && files.length < MAX_FILES) {
    const batch = level.slice(0, MAX_DIRS - visited);
    visited += batch.length;
    const listed = await Promise.all(batch.map((dir) => list(dir).then((entries) => ({ dir, entries }), () => ({ dir, entries: [] }))));
    const next: string[] = [];
    for (const { dir, entries } of listed) {
      for (const entry of entries) {
        if (entry.name.startsWith(".")) continue;
        const path = `${dir}/${entry.name}`;
        if (entry.kind === "dir" && !SKIP.has(entry.name)) next.push(path);
        else if (entry.kind === "file" && docKind(entry.name)) files.push(path);
      }
    }
    level = next;
  }
  return files.slice(0, MAX_FILES);
}

/**
 * How well `query` matches `text`: its letters must appear in order. Runs
 * of consecutive letters, and letters at the start of words, score higher.
 * Undefined when it doesn't match.
 */
export function matchScore(text: string, query: string): number | undefined {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const t = text.toLowerCase();
  let score = 0;
  let from = 0;
  let previous = -2;
  for (const ch of q) {
    const at = t.indexOf(ch, from);
    if (at < 0) return undefined;
    score += at === previous + 1 ? 3 : 1;
    if (at === 0 || /[\s/_.\-]/.test(t[at - 1])) score += 2;
    previous = at;
    from = at + 1;
  }
  // Shorter names win among equal matches.
  return score - t.length / 100;
}
