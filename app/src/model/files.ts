// Names, templates and order for the file tree's new files and folders.

import type { DirEntry } from "../backend/types";

/** What the tree's New items make. */
export type NewKind = "file" | "folder" | "qmd" | "R" | "ipynb";

export type SortBy = "name" | "size" | "modified";

export const SORTS: { by: SortBy; label: string; order: string }[] = [
  { by: "name", label: "Name", order: "A to Z" },
  { by: "size", label: "Size", order: "largest first" },
  { by: "modified", label: "Date modified", order: "newest first" },
];

/** The name a new item's field starts with. */
export const NEW_NAMES: Record<NewKind, string> = { file: "", folder: "", qmd: "untitled.qmd", R: "untitled.R", ipynb: "untitled.ipynb" };

export const NEW_LABELS: Record<NewKind, string> = {
  file: "New file",
  folder: "New folder",
  qmd: "New Quarto document",
  R: "New R script",
  ipynb: "New notebook",
};

export function baseName(path: string): string {
  return path.slice(path.replace(/\/$/, "").lastIndexOf("/") + 1);
}

export function parentOf(path: string): string {
  const i = path.replace(/\/$/, "").lastIndexOf("/");
  return i <= 0 ? "/" : path.slice(0, i);
}

export function joinPath(dir: string, name: string): string {
  return `${dir.replace(/\/$/, "")}/${name}`;
}

/** Whether `path` is `dir` or inside it. */
export function within(path: string, dir: string): boolean {
  return path === dir || path.startsWith(`${dir.replace(/\/$/, "")}/`);
}

const BAD = /[\\:*?"<>|]/;

/**
 * Why a typed name can't be used, if it can't. New items may be paths
 * (`raw/samples.csv` makes the folder too); a rename is one name.
 */
export function nameProblem(name: string, path: boolean): string | undefined {
  if (BAD.test(name)) return 'A name can\'t contain \\ : * ? " < > |';
  if (!path && name.includes("/")) return "A name can't contain /";
  const parts = name.split("/");
  if (name.startsWith("/") || parts.some((p) => p === "" || p === "." || p === "..")) return "That isn't a name inside this folder";
  return undefined;
}

/** The name before its extension, which the field selects. */
export function stemLength(name: string, dir: boolean): number {
  const dot = name.lastIndexOf(".");
  return !dir && dot > 0 ? dot : name.length;
}

function splitExt(name: string): [string, string] {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
}

/** `name`, or "untitled 2.qmd" if that's taken, then "untitled 3.qmd" and so on. */
export function freeName(name: string, taken: ReadonlySet<string>): string {
  if (!taken.has(name)) return name;
  const [stem, ext] = splitExt(name);
  for (let n = 2; ; n++) {
    const candidate = `${stem} ${n}${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** Duplicate's name: "fig3 copy.qmd" or "data copy", then "fig3 copy 2.qmd". */
export function copyName(name: string, dir: boolean, taken: ReadonlySet<string>): string {
  const [stem, ext] = dir ? [name, ""] : splitExt(name);
  return freeName(`${stem} copy${ext}`, taken);
}

/**
 * What a new file starts with: a Quarto title block and an empty R chunk,
 * a comment naming an R script, a heading for Markdown, and nothing for
 * anything else. Notebooks are made by the caller, which knows the kernels.
 */
export function template(name: string): string {
  const [stem, dotExt] = splitExt(name);
  const ext = dotExt.slice(1).toLowerCase();
  if (ext === "qmd") return `---\ntitle: "${stem.replace(/"/g, '\\"')}"\nformat: html\n---\n\n\`\`\`{r}\n\n\`\`\`\n`;
  if (ext === "rmd") return `---\ntitle: "${stem.replace(/"/g, '\\"')}"\noutput: html_document\n---\n\n\`\`\`{r}\n\n\`\`\`\n`;
  if (ext === "r") return `# ${name}\n\n`;
  if (ext === "md") return `# ${stem}\n`;
  return "";
}

/** What the file tree leaves out, as VS Code does: git's folder, and what macOS, Jupyter and RStudio keep beside the files. */
const TREE_HIDDEN = new Set([".git", ".DS_Store", ".ipynb_checkpoints", ".Rproj.user"]);

/** Whether the file tree lists a file or folder. Other names that start with a dot, like .env, .gitignore and .sandhill, show. */
export function shownInTree(name: string): boolean {
  return !TREE_HIDDEN.has(name);
}

/** Folders first, then by name (A to Z), size (largest first) or date modified (newest first). */
export function sortEntries(entries: DirEntry[], by: SortBy): DirEntry[] {
  const name = (a: DirEntry, b: DirEntry) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
  const key = (a: DirEntry, b: DirEntry) => (by === "size" ? b.size - a.size : by === "modified" ? b.mtime_ms - a.mtime_ms : 0);
  return [...entries].sort((a, b) => Number(b.kind === "dir") - Number(a.kind === "dir") || key(a, b) || name(a, b));
}

/** "4 min ago", "3 h ago", "yesterday", "12 days ago". */
export function modifiedAgo(mtimeMs: number, now = Date.now()): string {
  const minutes = Math.max(0, Math.round((now - mtimeMs) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 1440) return `${Math.round(minutes / 60)} h ago`;
  if (minutes < 2880) return "yesterday";
  return `${Math.round(minutes / 1440)} days ago`;
}
