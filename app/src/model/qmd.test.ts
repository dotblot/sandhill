import { describe, expect, it } from "vitest";
import { frontMatterKernel, parseChunks, sameCode } from "./qmd";

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
