import { describe, expect, it } from "vitest";
import type { GitStatus } from "../backend/types";
import {
  avatar,
  branchName,
  commitMessage,
  diffLines,
  fileMark,
  hunkLabel,
  fileSize,
  layoutGraph,
  lineCounts,
  splitMessage,
  splitPath,
  stagedFiles,
  timeAgo,
  unstagedFiles,
  WORKTREE,
} from "./git";

const c = (sha: string, ...parents: string[]) => ({ sha, parents });

describe("layoutGraph", () => {
  it("keeps a line of commits in one lane", () => {
    const { rows, lanes } = layoutGraph([c("c", "b"), c("b", "a"), c("a")]);
    expect(lanes).toBe(1);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0]);
    expect(rows[0].edges).toEqual([{ from: 0, to: 0, color: 0 }]);
    expect(rows[2].edges).toEqual([]);
  });

  it("draws the mockup's branches: a fork, a merge and a side branch", () => {
    // The mockup's history, newest first, with the working tree on top.
    const { rows, lanes } = layoutGraph([
      c(WORKTREE, "label"),
      c("label", "qc"),
      c("qc", "filter"),
      c("harmony", "filter"),
      c("filter", "pin"),
      c("pin", "merge"),
      c("merge", "first", "deseq"),
      c("deseq", "sheet"),
      c("sheet", "first"),
      c("first"),
    ]);
    expect(lanes).toBe(2);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0, 1, 0, 0, 0, 1, 1, 0]);
    // exp/harmony forks from "filter": its lane curves back into lane 0.
    expect(rows[3].edges).toEqual([
      { from: 0, to: 0, color: 0 },
      { from: 1, to: 0, color: 1 },
    ]);
    // The merge starts lane 1 for its second parent.
    expect(rows[6].edges).toEqual([
      { from: 0, to: 0, color: 0 },
      { from: 0, to: 1, color: 1 },
    ]);
    // ...which joins lane 0 again at the first commit.
    expect(rows[8].edges).toEqual([
      { from: 0, to: 0, color: 0 },
      { from: 1, to: 0, color: 1 },
    ]);
  });

  it("reuses a lane once its branch ends", () => {
    const { rows, lanes } = layoutGraph([c("x", "a"), c("y", "b"), c("a", "r"), c("b", "r"), c("z", "r"), c("r")]);
    expect(rows.map((r) => r.lane)).toEqual([0, 1, 0, 1, 2, 0]);
    expect(lanes).toBe(3);
  });

  it("keeps lines going past the last loaded commit", () => {
    const { rows } = layoutGraph([c("b", "a")]);
    expect(rows[0].edges).toEqual([{ from: 0, to: 0, color: 0 }]);
  });
});

describe("avatar", () => {
  it("shows a person's initials in a colour of their own", () => {
    const huan = avatar({ author: "Huan Yang", email: "huan@example.org" });
    expect(huan.initials).toBe("HY");
    expect(huan.who).toBe("Huan Yang");
    expect(huan.agent).toBeUndefined();
    expect(avatar({ author: "Huan Yang", email: "huan@example.org" }).colors).toEqual(huan.colors);
    expect(avatar({ author: "dotblot", email: "x@y" }).initials).toBe("D");
  });

  it("shows Claude Code and Codex for the commits they helped write", () => {
    const claude = avatar({ author: "Huan Yang", email: "h@x", coauthors: ["Claude <noreply@anthropic.com>"] });
    expect(claude.initials).toBe("CC");
    expect(claude.agent).toBe("claude");
    expect(claude.who).toBe("Huan Yang with Claude Code");
    const codex = avatar({ author: "Codex", email: "codex@openai.com" });
    expect(codex.initials).toBe("CX");
    expect(codex.who).toBe("Codex");
  });
});

describe("files", () => {
  const status = {
    files: [
      { path: "analysis.qmd", staged: "M" },
      { path: "R/01_qc.R", unstaged: "M", lines: { added: 3, removed: 1 } },
      { path: "both.R", staged: "M", unstaged: "M" },
      { path: "CLAUDE.md", unstaged: "?" },
      { path: "conflict.R", unstaged: "U", conflict: true },
    ],
  } as GitStatus;

  it("splits staged from not staged; a file can be in both", () => {
    expect(stagedFiles(status).map((f) => f.path)).toEqual(["analysis.qmd", "both.R"]);
    expect(unstagedFiles(status).map((f) => f.path)).toEqual(["R/01_qc.R", "both.R", "CLAUDE.md", "conflict.R"]);
  });

  it("marks each file", () => {
    expect(fileMark(status.files[3], false)).toMatchObject({ letter: "A", kind: "a" });
    expect(fileMark(status.files[4], false)).toMatchObject({ letter: "!", kind: "c" });
    expect(fileMark({ path: "b", staged: "R", from: "a" }, true).title).toBe("Renamed from a");
    expect(lineCounts(status.files[1].lines)).toBe("+3 −1");
    expect(lineCounts({ added: 0, removed: 0, binary: true })).toBe("binary");
    expect(fileSize(2.1 * 1024 ** 3)).toBe("2.1 GB");
    expect(splitPath("R/01_qc.R")).toEqual({ name: "01_qc.R", dir: "R" });
    expect(splitPath("data/raw/")).toEqual({ name: "raw/", dir: "data" });
  });
});

describe("messages", () => {
  it("joins and splits the summary and description", () => {
    expect(commitMessage(" Add QC plots ", "")).toBe("Add QC plots");
    expect(commitMessage("Add QC plots", "Per sample.\n")).toBe("Add QC plots\n\nPer sample.");
    expect(splitMessage("Add QC plots\n\nPer sample.")).toEqual({ summary: "Add QC plots", body: "Per sample." });
  });

  it("says how long ago", () => {
    const now = Date.UTC(2026, 9, 18, 12);
    expect(timeAgo(now - 30_000, now)).toBe("now");
    expect(timeAgo(now - 5 * 60_000, now)).toBe("5m");
    expect(timeAgo(now - 3 * 3600_000, now)).toBe("3h");
    expect(timeAgo(now - 2 * 86400_000, now)).toBe("2d");
  });

  it("takes only branch names git would", () => {
    expect(branchName(" exp/harmony ")).toBe("exp/harmony");
    expect(branchName("my branch")).toBeUndefined();
    expect(branchName("-f")).toBeUndefined();
    expect(branchName("a..b")).toBeUndefined();
  });
});

describe("diffLines", () => {
  it("numbers the lines of each hunk", () => {
    const lines = diffLines("@@ -1,3 +1,3 @@ library(Seurat)\n x <- 1\n-y <- 2\n+y <- 3\n z <- 4\n\\ No newline at end of file\n");
    expect(lines).toEqual([
      { kind: "hunk", text: "@@ -1,3 +1,3 @@ library(Seurat)", at: 0 },
      { kind: "ctx", text: "x <- 1", at: 1, old: 1, new: 1 },
      { kind: "del", text: "y <- 2", at: 2, old: 2 },
      { kind: "add", text: "y <- 3", at: 3, new: 2 },
      { kind: "ctx", text: "z <- 4", at: 4, old: 3, new: 3 },
      { kind: "note", text: "No newline at end of file", at: 5 },
    ]);
  });

  it("says where a hunk is in words", () => {
    expect(hunkLabel("@@ -33,3 +33,6 @@ plt.close(fig)")).toEqual({ lines: "Lines 33–38", context: "plt.close(fig)" });
    expect(hunkLabel("@@ -0,0 +1 @@")).toEqual({ lines: "Line 1", context: "" });
    expect(hunkLabel("@@ -5,2 +4,0 @@")).toEqual({ lines: "After line 4", context: "" });
    expect(hunkLabel("@@@ -1,2 -1,2 +1,6 @@@")).toEqual({ lines: "Lines 1–6", context: "" });
  });

  it("reads a conflict's combined diff", () => {
    const lines = diffLines("@@@ -1,2 -1,2 +1,6 @@@\n  x <- 1\n++<<<<<<< HEAD\n+ y <- 30\n");
    expect(lines.map((l) => [l.kind, l.text])).toEqual([
      ["hunk", "@@@ -1,2 -1,2 +1,6 @@@"],
      ["ctx", "x <- 1"],
      ["add", "<<<<<<< HEAD"],
      ["add", "y <- 30"],
    ]);
  });
});
