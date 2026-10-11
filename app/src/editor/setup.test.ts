import { describe, expect, it } from "vitest";
import { highlightedLines, languageFor } from "./setup";

const text = (lines: HTMLElement[]) => lines.map((l) => l.textContent);

describe("highlightedLines", () => {
  it("keeps every line, empty ones too, as the editor would", () => {
    const lines = highlightedLines("import scanpy as sc\n\nsc.pl.umap(adata)\n", languageFor("python"));
    expect(text(lines)).toEqual(["import scanpy as sc", "", "sc.pl.umap(adata)", ""]);
    // An empty line still takes a line's height.
    expect(lines[1].innerHTML).toBe("<br>");
  });

  it("marks Python tokens with the editor's classes", () => {
    const [line] = highlightedLines("import os  # note", languageFor("python"));
    expect(line.querySelector(".tok-keyword")?.textContent).toBe("import");
    expect(line.querySelector(".tok-comment")?.textContent).toBe("# note");
  });

  it("highlights R outside an editor", () => {
    const [line] = highlightedLines('x <- "a"', languageFor("r"));
    expect(line.textContent).toBe('x <- "a"');
    expect(line.querySelector(".tok-string")?.textContent).toBe('"a"');
  });

  it("shows plain text without a language", () => {
    expect(text(highlightedLines("a\nb", undefined))).toEqual(["a", "b"]);
  });
});
