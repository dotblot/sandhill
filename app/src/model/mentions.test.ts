import { describe, expect, it } from "vitest";
import { attachmentName, cellDocuments, cellMentions, documentCells, filterCells, filterMentions, type Mention, typedMention } from "./mentions";

describe("mentions", () => {
  it("finds the @ being typed before the caret, not one inside a word", () => {
    expect(typedMention("look at @an", 11)).toEqual({ at: 8, query: "an", sign: "@" });
    expect(typedMention("@", 1)).toEqual({ at: 0, query: "", sign: "@" });
    expect(typedMention("fix #3", 6)).toEqual({ at: 4, query: "3", sign: "#" });
    expect(typedMention("issue#3", 7)).toBeUndefined();
    expect(typedMention("mail me@x", 9)).toBeUndefined();
    expect(typedMention("@seu and more", 13)).toBeUndefined();
  });

  it("groups matches in menu order", () => {
    const all: Mention[] = [
      { ref: { kind: "variable", name: "markers" } },
      { ref: { kind: "file", name: "R/markers.R" } },
      { ref: { kind: "plot", name: "umap · analysis.qmd" } },
    ];
    expect(filterMentions(all, "MARK").map(([label, list]) => [label, list.map((m) => m.ref.name)])).toEqual([
      ["Files", ["R/markers.R"]],
      ["Variables", ["markers"]],
    ]);
  });

  it("lists the cells of the notebooks named with @ for #, by number", () => {
    const refs = [
      { kind: "file" as const, name: "de.ipynb", path: "de.ipynb" },
      { kind: "file" as const, name: "R/markers.R", path: "R/markers.R" },
      { kind: "file" as const, name: "report.qmd", path: "docs/report.qmd" },
    ];
    expect(cellDocuments(refs)).toEqual(["de.ipynb", "docs/report.qmd"]);
    const nb = {
      cells: [
        { cell_type: "markdown", id: "t", source: "# QC" },
        { cell_type: "code", id: "load", source: ["\n", "adata = sc.read('x')\n"] },
        { cell_type: "code", id: "umap", source: "sc.pl.umap(adata)" },
      ],
    };
    const cells = documentCells("de.ipynb", JSON.stringify(nb));
    // Numbered by place among all cells, as the notebook shows them.
    expect(cells.map((c) => c.name)).toEqual(["cell 2", "cell 3"]);
    expect(documentCells("docs/report.qmd", "```{r setup}\nlibrary(x)\n```\n\n```{r}\nplot(1)\n```\n").map((c) => c.name)).toEqual(["setup", "chunk 2"]);
    const all = cellMentions("de.ipynb", "de.ipynb", cells);
    expect(all[0]).toMatchObject({ ref: { kind: "chunk", name: "de.ipynb › cell 2", path: "de.ipynb", cell: "load" }, label: "cell 2", detail: "adata = sc.read('x')" });
    expect(filterCells(all, "3").map(([doc, list]) => [doc, list.map((m) => m.label)])).toEqual([["de.ipynb", ["cell 3"]]]);
    expect(filterCells(all, "umap")[0][1].map((m) => m.ref.cell)).toEqual(["umap"]);
  });

  it("names pasted images by the time, as macOS names screenshots", () => {
    const at = new Date(2026, 9, 25, 9, 5, 3);
    expect(attachmentName("image.png", at)).toBe("Pasted image 2026-10-25 at 09.05.03.png");
    expect(attachmentName("gel/blot.tif", at)).toBe("gel-blot.tif");
  });
});
