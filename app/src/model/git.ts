import type { GitCommit, GitFile, GitStatus } from "../backend/types";

/** The working tree's row over the commits, as a commit whose parent is HEAD. */
export const WORKTREE = "worktree";

/** A line in the graph from one row to the next, by lane. */
export interface GraphEdge {
  from: number;
  to: number;
  /** The lane whose colour it takes. */
  color: number;
}

export interface GraphRow {
  lane: number;
  /** Lines from this row's middle to the next row's. */
  edges: GraphEdge[];
}

/**
 * Lays out the commit graph: each commit in a lane, newest first, with the
 * lines to its parents. A lane waits for one commit; a commit takes the
 * first lane waiting for it (others waiting for it end there, as branches
 * that forked from it), its first parent continues its lane, and a merge's
 * other parents join a lane already waiting for them or start a new one.
 * Lanes never move, so a branch is a straight line.
 */
export function layoutGraph(commits: Pick<GitCommit, "sha" | "parents">[]): { rows: GraphRow[]; lanes: number } {
  const lanes: (string | null)[] = [];
  const rows: { lane: number; after: (string | null)[]; through: Set<number>; started: Set<number> }[] = [];
  let width = 0;
  for (const commit of commits) {
    const waiting = lanes.flatMap((sha, i) => (sha === commit.sha ? [i] : []));
    const free = () => {
      const i = lanes.indexOf(null);
      return i < 0 ? lanes.length : i;
    };
    const lane = waiting.length ? waiting[0] : free();
    const through = new Set(lanes.flatMap((sha, i) => (sha !== null && sha !== commit.sha ? [i] : [])));
    for (const i of waiting) lanes[i] = null;
    const started = new Set<number>();
    const [first, ...others] = commit.parents;
    lanes[lane] = first ?? null;
    if (first) started.add(lane);
    for (const parent of others) {
      let at = lanes.indexOf(parent);
      if (at < 0) {
        at = free();
        lanes[at] = parent;
      }
      started.add(at);
    }
    while (lanes.length && lanes[lanes.length - 1] === null) lanes.pop();
    width = Math.max(width, lanes.length, lane + 1);
    rows.push({ lane, after: [...lanes], through, started });
  }
  return {
    lanes: width,
    rows: rows.map((row, i) => {
      const next = commits[i + 1];
      const edges: GraphEdge[] = [];
      row.after.forEach((sha, k) => {
        if (sha === null) return;
        // Into the next commit's lane if that's what this lane waits for.
        const to = next && sha === next.sha ? rows[i + 1].lane : k;
        if (row.through.has(k)) edges.push({ from: k, to, color: k });
        if (row.started.has(k)) edges.push({ from: row.lane, to, color: k });
      });
      return { lane: row.lane, edges };
    }),
  };
}

/** Lane colours, as in the mockup: blue, then violet, and on. */
export const LANE_COLORS = ["#4b8df0", "#a274e8", "#2fb39a", "#e0843c", "#d9567a", "#8a9a2a"];

export interface Avatar {
  initials: string;
  /** A gradient, top-left to bottom-right. */
  colors: [string, string];
  /** Who the tooltip names: "Huan Yang", or "Huan Yang with Claude Code". */
  who: string;
  agent?: "claude" | "codex";
}

const PEOPLE: [string, string][] = [
  ["#2aa6c9", "#2fc7a0"],
  ["#e2a33c", "#d9672f"],
  ["#5aa469", "#2e8b7a"],
  ["#c75bb0", "#7b6cf0"],
  ["#3f8cf0", "#2aa6c9"],
  ["#d9467a", "#e2a33c"],
];
const CLAUDE: [string, string] = ["#e0743c", "#d9467a"];
const CODEX: [string, string] = ["#7b6cf0", "#3f8cf0"];

/** Which agent helped write a commit, from its author and co-authors. */
export function commitAgent(commit: Pick<GitCommit, "author" | "email" | "coauthors">): "claude" | "codex" | undefined {
  const people = [`${commit.author} <${commit.email}>`, ...(commit.coauthors ?? [])];
  if (people.some((p) => /noreply@anthropic\.com|^claude\b/i.test(p))) return "claude";
  if (people.some((p) => /\bcodex\b|@openai\.com/i.test(p))) return "codex";
  return undefined;
}

/** A commit's avatar: the author's initials, or Claude Code's or Codex's. */
export function avatar(commit: Pick<GitCommit, "author" | "email" | "coauthors">): Avatar {
  const agent = commitAgent(commit);
  const name = commit.author.trim() || commit.email;
  if (agent) {
    const label = agent === "claude" ? "Claude Code" : "Codex";
    const human = !new RegExp(agent, "i").test(name);
    return {
      initials: agent === "claude" ? "CC" : "CX",
      colors: agent === "claude" ? CLAUDE : CODEX,
      who: human ? `${name} with ${label}` : label,
      agent,
    };
  }
  const words = name.split(/[\s._-]+/).filter(Boolean);
  const initials = (words.length > 1 ? words[0][0] + words[1][0] : (words[0]?.[0] ?? "?")).toUpperCase();
  let hash = 0;
  for (const c of commit.email.toLowerCase() || name) hash = (hash * 31 + c.charCodeAt(0)) | 0;
  return { initials, colors: PEOPLE[Math.abs(hash) % PEOPLE.length], who: name };
}

/** "now", "5m", "3h", "2d", then the date: how long ago, for a commit row. */
export function timeAgo(ms: number, now: number): string {
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 7 * 86400) return `${Math.floor(s / 86400)}d`;
  const date = new Date(ms);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString(undefined, sameYear ? { month: "short", day: "numeric" } : { year: "numeric", month: "short" });
}

/** Files with a staged change, and files with a change not staged (new and conflicted ones too). */
export function stagedFiles(status: GitStatus): GitFile[] {
  return status.files.filter((f) => f.staged && !f.conflict);
}
export function unstagedFiles(status: GitStatus): GitFile[] {
  return status.files.filter((f) => f.unstaged);
}

/** The letter a file shows, and its colour's class: a new file is A, a conflict !. */
export function fileMark(file: GitFile, staged: boolean): { letter: string; kind: string; title: string } {
  if (!staged && file.conflict) return { letter: "!", kind: "c", title: "Conflict: changed on both sides" };
  const letter = (staged ? file.staged : file.unstaged) ?? "M";
  switch (letter) {
    case "?":
      return { letter: "A", kind: "a", title: "New, not yet in Git" };
    case "A":
      return { letter: "A", kind: "a", title: "Added" };
    case "D":
      return { letter: "D", kind: "d", title: "Deleted" };
    case "R":
      return { letter: "R", kind: "r", title: file.from ? `Renamed from ${file.from}` : "Renamed" };
    case "C":
      return { letter: "C", kind: "r", title: file.from ? `Copied from ${file.from}` : "Copied" };
    case "T":
      return { letter: "T", kind: "m", title: "Type changed" };
    default:
      return { letter: "M", kind: "m", title: "Modified" };
  }
}

/** "+12 −3", or "binary". */
export function lineCounts(lines?: { added: number; removed: number; binary?: boolean }): string {
  if (!lines) return "";
  if (lines.binary) return "binary";
  const parts = [];
  if (lines.added) parts.push(`+${lines.added}`);
  if (lines.removed) parts.push(`−${lines.removed}`);
  return parts.join(" ");
}

/** GitHub warns about files over 50 MB and refuses them over 100 MB. */
export const BIG_FILE = 50 * 1024 * 1024;

export function fileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

/** A file's folder and name, for a row: the name, then the folder fainter. */
export function splitPath(path: string): { name: string; dir: string } {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path;
  const at = trimmed.lastIndexOf("/");
  return { name: trimmed.slice(at + 1) + (path.endsWith("/") ? "/" : ""), dir: at < 0 ? "" : trimmed.slice(0, at) };
}

/** The message for git from the summary and description boxes. */
export function commitMessage(summary: string, body: string): string {
  const head = summary.trim();
  const rest = body.trim();
  return rest ? `${head}\n\n${rest}` : head;
}

/** A message split back into the two boxes, to amend it. */
export function splitMessage(message: string): { summary: string; body: string } {
  const [summary, ...rest] = message.split("\n");
  return { summary: summary.trim(), body: rest.join("\n").trim() };
}

export interface DiffLine {
  kind: "hunk" | "add" | "del" | "ctx" | "note";
  text: string;
  /** Line numbers in the old and new file. */
  old?: number;
  new?: number;
}

/** Reads a unified diff (or a merge conflict's combined one) into lines with their numbers. */
export function diffLines(text: string): DiffLine[] {
  const lines: DiffLine[] = [];
  let old = 0;
  let now = 0;
  let width = 1;
  for (const line of text.split("\n")) {
    if (line.startsWith("@@")) {
      const combined = line.startsWith("@@@");
      width = combined ? 2 : 1;
      const ranges = [...line.matchAll(/([-+])(\d+)/g)];
      old = Number(ranges.find((r) => r[1] === "-")?.[2] ?? 0);
      now = Number(ranges.find((r) => r[1] === "+")?.[2] ?? 0);
      lines.push({ kind: "hunk", text: line });
      continue;
    }
    if (line.startsWith("\\")) {
      lines.push({ kind: "note", text: line.slice(2) });
      continue;
    }
    if (!lines.length) continue;
    const mark = line.slice(0, width);
    const body = line.slice(width);
    if (mark.includes("+")) lines.push({ kind: "add", text: body, new: now++ });
    else if (mark.includes("-")) lines.push({ kind: "del", text: body, old: old++ });
    else if (line.length || lines[lines.length - 1].kind !== "hunk") lines.push({ kind: "ctx", text: body, old: old++, new: now++ });
  }
  // The text ends with a newline, which isn't a line.
  const last = lines[lines.length - 1];
  if (last?.kind === "ctx" && last.text === "" && !text.endsWith("\n\n")) lines.pop();
  return lines;
}

/** A hunk's header in words: "Lines 33–38" of the new file, and the code it's in. */
export function hunkLabel(header: string): { lines: string; context: string } {
  const match = /^@@+ .*?\+(\d+)(?:,(\d+))? @@+ ?(.*)$/.exec(header);
  if (!match) return { lines: header, context: "" };
  const start = Number(match[1]);
  const count = match[2] === undefined ? 1 : Number(match[2]);
  const lines = count === 0 ? `After line ${start}` : count === 1 ? `Line ${start}` : `Lines ${start}–${start + count - 1}`;
  return { lines, context: match[3] };
}

/** A branch name git would take: no spaces, nothing it reserves. */
export function branchName(text: string): string | undefined {
  const name = text.trim();
  if (!name || name.startsWith("-") || name.startsWith("/") || name.endsWith("/") || name.endsWith(".lock") || name.endsWith(".")) return undefined;
  if (/[\s~^:?*[\\\x00-\x1f\x7f]|\.\.|@\{|\/\//.test(name) || name === "@") return undefined;
  return name;
}
