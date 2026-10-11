import { describe, expect, it } from "vitest";
import { statementAt, statements } from "./statements";

const lines = (text: string, language: string) => statements(text, language).map((s) => [s.from, s.to]);

describe("statements in R", () => {
  it("keeps calls, pipes and functions together", () => {
    const script = [
      "library(Seurat)", // 1
      "", // 2
      "# Load the counts", // 3
      "pbmc <- Read10X(", // 4
      "  data.dir = 'data/'", // 5
      ")", // 6
      "pbmc |>", // 7
      "  NormalizeData() %>%", // 8
      "  FindVariableFeatures()", // 9
      "p <- ggplot(df) +", // 10
      "  geom_point()", // 11
      "f <- function(x) {", // 12
      "  x + 1 # }", // 13
      "}", // 14
      'msg <- "a # not a comment (', // 15
      'still the string"', // 16
      "x <- 1; y <- 2", // 17
    ].join("\n");
    expect(lines(script, "r")).toEqual([
      [1, 1],
      [4, 6],
      [7, 9],
      [10, 11],
      [12, 14],
      [15, 16],
      [17, 17],
    ]);
  });

  it("finds the statement around the cursor, or none on blank and comment lines", () => {
    const script = "x <- c(1,\n  2,\n  3)\n\n# done\nprint(x)";
    expect(statementAt(script, "r", 2)).toEqual({ from: 1, to: 3 });
    expect(statementAt(script, "r", 4)).toBeUndefined();
    expect(statementAt(script, "r", 5)).toBeUndefined();
    expect(statementAt(script, "r", 6)).toEqual({ from: 6, to: 6 });
  });
});

describe("statements in Python", () => {
  it("keeps blocks, decorators and bracketed lines together", () => {
    const script = [
      "import scanpy as sc", // 1
      "adata = sc.read_h5ad(", // 2
      '    "pbmc.h5ad")', // 3
      "for gene in genes:", // 4
      "    print(gene)", // 5
      "", // 6
      "    print('still in the loop')", // 7
      "else:", // 8
      "    pass", // 9
      "@cache", // 10
      "def f(x):", // 11
      '    """Doc', // 12
      "    string: with a colon", // 13
      '    """', // 14
      "    return x", // 15
      "", // 16
      "total = 1 + \\", // 17
      "    2", // 18
      "sc.pl.umap(adata)  # plot: colour", // 19
    ].join("\n");
    expect(lines(script, "python")).toEqual([
      [1, 1],
      [2, 3],
      [4, 9],
      [10, 15],
      [17, 18],
      [19, 19],
    ]);
  });
});
