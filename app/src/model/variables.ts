// What the Variables pane shows of a session's variables: grouped as data,
// values and functions, as in RStudio, filtered by name, and what deleting
// some of them frees.

import type { Variable } from "../backend/types";
import { formatBytes } from "./machine";

export interface VariableGroup {
  title: "Data" | "Values" | "Functions";
  variables: Variable[];
}

const GROUPS: { title: VariableGroup["title"]; kind: string }[] = [
  { title: "Data", kind: "data" },
  { title: "Values", kind: "value" },
  { title: "Functions", kind: "function" },
];

/** The variables whose names hold `filter` (any case), by name in their groups; empty groups are left out. */
export function groupVariables(variables: Variable[], filter = ""): VariableGroup[] {
  const wanted = filter.trim().toLowerCase();
  const shown = (wanted ? variables.filter((v) => v.name.toLowerCase().includes(wanted)) : [...variables]).sort((a, b) =>
    a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true }),
  );
  return GROUPS.map(({ title, kind }) => ({
    title,
    variables: shown.filter((v) => (kind === "data" ? !["value", "function"].includes(v.kind) : v.kind === kind)),
  })).filter((g) => g.variables.length);
}

/** The memory the named variables hold, as far as their sizes are known. */
export function sizeOf(variables: Variable[], names: Iterable<string>): number {
  const picked = new Set(names);
  return variables.filter((v) => picked.has(v.name)).reduce((sum, v) => sum + (v.size ?? 0), 0);
}

/** "seu, markers · 412 MB": what a deletion took away. */
export function removedText(names: string[], bytes: number): string {
  const shown = names.length > 3 ? `${names.slice(0, 3).join(", ")} and ${names.length - 3} more` : names.join(", ");
  return bytes ? `Removed ${shown} · ${formatBytes(bytes)} freed` : `Removed ${shown}`;
}

/** A path as a key: a variable's name, then its parts. */
export function pathKey(path: string[]): string {
  return path.join("\u0000");
}
