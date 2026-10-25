import { describe, expect, it } from "vitest";
import type { DirEntry } from "../backend/types";
import { copyName, freeName, modifiedAgo, nameProblem, parentOf, sortEntries, stemLength, template, within } from "./files";

describe("new names", () => {
  it("numbers names that are taken, as Finder does", () => {
    expect(freeName("untitled.qmd", new Set())).toBe("untitled.qmd");
    expect(freeName("untitled.qmd", new Set(["untitled.qmd", "untitled 2.qmd"]))).toBe("untitled 3.qmd");
    expect(copyName("fig3.qmd", false, new Set())).toBe("fig3 copy.qmd");
    expect(copyName("fig3.qmd", false, new Set(["fig3 copy.qmd"]))).toBe("fig3 copy 2.qmd");
    expect(copyName("data.v2", true, new Set())).toBe("data.v2 copy");
  });

  it("refuses names that would cause trouble", () => {
    expect(nameProblem("raw/samples.csv", true)).toBeUndefined();
    expect(nameProblem("raw/samples.csv", false)).toBe("A name can't contain /");
    expect(nameProblem("a:b", true)).toMatch(/can't contain/);
    expect(nameProblem("../x", true)).toBe("That isn't a name inside this folder");
    expect(nameProblem("raw//x", true)).toBe("That isn't a name inside this folder");
  });

  it("selects the name before its extension", () => {
    expect(stemLength("untitled.qmd", false)).toBe(8);
    expect(stemLength(".env", false)).toBe(4);
    expect(stemLength("data.v2", true)).toBe(7);
  });

  it("knows paths", () => {
    expect(parentOf("/w/p/a.R")).toBe("/w/p");
    expect(parentOf("/a")).toBe("/");
    expect(within("/w/p/data/x.csv", "/w/p/data")).toBe(true);
    expect(within("/w/p/database", "/w/p/data")).toBe(false);
  });
});

describe("templates", () => {
  it("start new documents the way the spec says", () => {
    expect(template("volcano.qmd")).toBe('---\ntitle: "volcano"\nformat: html\n---\n\n```{r}\n\n```\n');
    expect(template("theme.R")).toBe("# theme.R\n\n");
    expect(template("README.md")).toBe("# README\n");
    expect(template("samples.csv")).toBe("");
  });
});

describe("sortEntries", () => {
  const e = (name: string, kind: "file" | "dir", size: number, mtime_ms: number): DirEntry => ({ name, kind, size, mtime_ms });
  const list = [e("b.R", "file", 10, 3), e("a10.R", "file", 300, 1), e("a9.R", "file", 20, 2), e("zdata", "dir", 5, 0), e("Adir", "dir", 900, 9)];
  const names = (by: "name" | "size" | "modified") => sortEntries(list, by).map((x) => x.name);

  it("puts folders first, then the chosen order", () => {
    expect(names("name")).toEqual(["Adir", "zdata", "a9.R", "a10.R", "b.R"]);
    expect(names("size")).toEqual(["Adir", "zdata", "a10.R", "a9.R", "b.R"]);
    expect(names("modified")).toEqual(["Adir", "zdata", "b.R", "a9.R", "a10.R"]);
  });

  it("says how long ago, in words", () => {
    const now = 10 * 86_400_000;
    expect(modifiedAgo(now - 4 * 60_000, now)).toBe("4 min ago");
    expect(modifiedAgo(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(modifiedAgo(now - 30 * 3_600_000, now)).toBe("yesterday");
    expect(modifiedAgo(now - 12 * 86_400_000, now)).toBe("12 days ago");
  });
});
