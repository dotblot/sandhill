import { describe, expect, it } from "vitest";
import { errorMessage, errorPlace, errorText } from "./errorHelp";

describe("errorHelp", () => {
  it("hands the agent the code, the error and the cell", () => {
    const error = {
      output_type: "error",
      ename: "KeyError",
      evalue: "'leiden'",
      traceback: ["\x1b[31mKeyError\x1b[0m Traceback", "----> 1 adata.obs[\x1b[32m'leiden'\x1b[0m]", "KeyError: 'leiden'"],
    };
    const { text, refs } = errorMessage(
      { action: "fix", error, code: "adata.obs['leiden']\n", language: "python", cell: { path: "/p/qc/de.ipynb", id: "c3", name: "cell 3" } },
      "/p",
    );
    expect(text).toBe(
      [
        "Fix this error from cell 3 of qc/de.ipynb: change the code so it works, then run it again to check.",
        "",
        "The code:",
        "```python",
        "adata.obs['leiden']",
        "```",
        "",
        "The error:",
        "```",
        "KeyError Traceback",
        "----> 1 adata.obs['leiden']",
        "KeyError: 'leiden'",
        "```",
      ].join("\n"),
    );
    expect(refs).toEqual([{ kind: "chunk", name: "de.ipynb › cell 3", path: "qc/de.ipynb", cell: "c3" }]);

    const explain = errorMessage({
      action: "explain",
      error: { output_type: "error", ename: "simpleError", evalue: "object 'x' not found" },
      code: "print(```x```)",
      language: "r",
    });
    expect(explain.text).toContain("Explain this error from the R Console");
    expect(explain.text).toContain("````r\nprint(```x```)\n````");
    expect(explain.refs).toEqual([]);
  });

  it("says where the error came from", () => {
    const error = { output_type: "error" };
    expect(errorPlace({ action: "fix", error, code: "", cell: { path: "/p/a.qmd", id: "umap" } }, "/p")).toBe("a cell of a.qmd");
    expect(errorPlace({ action: "fix", error, code: "", language: "python", file: "/p/qc.py" }, "/p")).toBe(
      "qc.py, run in the Python Console",
    );
    expect(errorPlace({ action: "fix", error, code: "", language: "julia" })).toBe("the Console");
  });

  it("keeps the end of a long traceback", () => {
    const traceback = Array.from({ length: 200 }, (_, i) => `frame ${i}`);
    const text = errorText({ output_type: "error", traceback });
    expect(text.split("\n")).toHaveLength(81);
    expect(text.startsWith("…\nframe 120")).toBe(true);
  });
});
