import { describe, expect, it } from "vitest";
import {
  chunkCodeChange,
  chunkInsertion,
  frontMatterKernel,
  newChunkLanguage,
  parseChunks,
  proseLines,
  sameCode,
} from "./qmd";

const doc = `---
title: "Demo"
jupyter: python3
---

Some text.

\`\`\`{python}
#| label: load
import pandas as pd
\`\`\`

\`\`\`python
not executed
\`\`\`

\`\`\`{r setup, echo=FALSE}
library(Seurat)
\`\`\`

\`\`\`\`markdown
\`\`\`{r}
inside a markdown example
\`\`\`
\`\`\`\`

\`\`\`{r, label="qc plot"}
plot(1)
\`\`\`

\`\`\`{python}
x = 1
\`\`\`
`;

describe("parseChunks", () => {
  const chunks = parseChunks(doc);

  it("finds only executable chunks", () => {
    expect(chunks.map((c) => c.language)).toEqual(["python", "r", "r", "python"]);
  });

  it("uses labels as ids and positions otherwise", () => {
    expect(chunks.map((c) => c.cellId)).toEqual(["load", "setup", "qc plot", "chunk-4"]);
  });

  it("extracts the code between the fences", () => {
    expect(chunks[0].code).toBe("#| label: load\nimport pandas as pd");
    expect(doc.slice(chunks[1].codeFrom, chunks[1].codeTo)).toBe("library(Seurat)");
    expect(doc.slice(chunks[1].from, chunks[1].to)).toBe(
      "```{r setup, echo=FALSE}\nlibrary(Seurat)\n```",
    );
  });

  it("reports fence line numbers", () => {
    const lines = doc.split("\n");
    expect(lines[chunks[0].openLine - 1]).toBe("```{python}");
    expect(lines[chunks[0].closeLine - 1]).toBe("```");
  });

  it("treats an unclosed chunk as running to the end", () => {
    const open = parseChunks("```{r}\nx <- 1\ny <- 2");
    expect(open).toHaveLength(1);
    expect(open[0].closed).toBe(false);
    expect(open[0].code).toBe("x <- 1\ny <- 2");
  });

  it("handles empty chunks and CRLF line endings", () => {
    const empty = parseChunks("```{r}\n```\n");
    expect(empty[0].code).toBe("");
    const crlf = parseChunks("```{r}\r\nx\r\n```\r\n");
    expect(crlf[0].code).toBe("x");
  });
});

describe("front matter", () => {
  it("reads the jupyter kernel", () => {
    expect(frontMatterKernel(doc)).toBe("python3");
    expect(frontMatterKernel("no front matter")).toBeUndefined();
  });
});

describe("sameCode", () => {
  it("ignores trailing whitespace and line endings", () => {
    expect(sameCode("x <- 1  \r\ny", "x <- 1\ny\n")).toBe(true);
    expect(sameCode("x <- 1", "x <- 2")).toBe(false);
  });
});

describe("proseLines", () => {
  it("leaves out the front matter and every fenced block", () => {
    const lines = doc.split("\n");
    const prose = proseLines(doc).map((n) => lines[n - 1]);
    expect(prose.filter((l) => l.trim())).toEqual(["Some text."]);
    expect(proseLines(doc)).toContain(5);
  });

  it("treats a document without front matter as prose from the top", () => {
    expect(proseLines("# Title\r\n\r\n```{r}\nx\n```\nend")).toEqual([1, 2, 6]);
  });
});

describe("agents' chunks", () => {
  const apply = (text: string, c: { from: number; to: number; insert: string }) =>
    text.slice(0, c.from) + c.insert + text.slice(c.to);

  it("go after the chunk named, or at the end", () => {
    const text = "# QC\n\n```{r setup}\nlibrary(x)\n```\n\nText.\n";
    const chunks = parseChunks(text);
    const change = chunkInsertion(text, "dim(x)\n", "r", chunks[0]);
    const after = apply(text, change);
    expect(after).toBe("# QC\n\n```{r setup}\nlibrary(x)\n```\n\n```{r}\ndim(x)\n```\n\nText.\n");
    expect(parseChunks(after).find((c) => c.from === change.start)?.cellId).toBe("chunk-2");
    expect(apply("Prose", chunkInsertion("Prose", "1", "python"))).toBe("Prose\n\n```{python}\n1\n```\n");
    expect(apply("", chunkInsertion("", "1", "python"))).toBe("```{python}\n1\n```\n");
  });

  it("take the language of their neighbours or the front matter", () => {
    const text = "---\njupyter: python3\n---\n\nNo code.\n";
    expect(newChunkLanguage(text, [], undefined, undefined, "/a.qmd")).toBe("python");
    expect(newChunkLanguage("x", [], undefined, undefined, "/a.Rmd")).toBe("r");
    expect(newChunkLanguage("x", [], undefined, undefined, "/a.qmd")).toBeUndefined();
    const chunks = parseChunks("```{r}\n1\n```\n");
    expect(newChunkLanguage("", chunks, undefined, undefined, "/a.qmd")).toBe("r");
  });

  it("replace a chunk's code in place", () => {
    const text = "```{r load}\nx <- read()\n```\n\n```{r}\n```\n";
    const chunks = parseChunks(text);
    expect(apply(text, chunkCodeChange(text, chunks[0], "x <- read('a')\n"))).toBe(
      "```{r load}\nx <- read('a')\n```\n\n```{r}\n```\n",
    );
    expect(apply(text, chunkCodeChange(text, chunks[1], "head(x)"))).toBe(
      "```{r load}\nx <- read()\n```\n\n```{r}\nhead(x)\n```\n",
    );
  });
});
