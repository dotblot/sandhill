import { describe, expect, it } from "vitest";
import type { DirEntry } from "../backend/types";
import { listProjectFiles, matchScore } from "./projectFiles";

const file = (name: string): DirEntry => ({ name, kind: "file", size: 1 }) as DirEntry;
const dir = (name: string): DirEntry => ({ name, kind: "dir", size: 0 }) as DirEntry;

describe("listProjectFiles", () => {
  it("lists documents Sandhill opens, skipping hidden and library folders", async () => {
    const tree: Record<string, DirEntry[]> = {
      "/p": [file("analysis.qmd"), file("data.csv"), dir("scripts"), dir(".git"), dir("renv"), file(".Rhistory")],
      "/p/scripts": [file("01_qc.R"), file("utils.py"), dir("deep")],
      "/p/scripts/deep": [file("nb.ipynb")],
      "/p/.git": [file("x.py")],
      "/p/renv": [file("y.R")],
    };
    const files = await listProjectFiles(async (path) => tree[path] ?? [], "/p/");
    expect(files).toEqual(["/p/analysis.qmd", "/p/scripts/01_qc.R", "/p/scripts/utils.py", "/p/scripts/deep/nb.ipynb"]);
  });

  it("keeps going when a folder can't be read", async () => {
    const files = await listProjectFiles(async (path) => {
      if (path === "/p") return [dir("locked"), file("a.qmd")];
      throw new Error("permission denied");
    }, "/p");
    expect(files).toEqual(["/p/a.qmd"]);
  });
});

describe("matchScore", () => {
  it("matches letters in order, and only then", () => {
    expect(matchScore("analysis.qmd", "anq")).toBeDefined();
    expect(matchScore("analysis.qmd", "qa")).toBeUndefined();
    expect(matchScore("Analysis.qmd", "ANA")).toBeDefined();
  });

  it("prefers runs of letters and starts of words", () => {
    const rank = (names: string[], q: string) =>
      names.filter((n) => matchScore(n, q) !== undefined).sort((a, b) => matchScore(b, q)! - matchScore(a, q)!);
    expect(rank(["scripts/qc_helpers.R", "01_qc.R", "quick.R"], "qc")).toEqual(["01_qc.R", "scripts/qc_helpers.R", "quick.R"]);
    expect(rank(["return values", "run file"], "run")).toEqual(["run file", "return values"]);
  });
});
