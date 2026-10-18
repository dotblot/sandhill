import { describe, expect, it } from "vitest";
import type { Variable } from "../backend/types";
import { groupVariables, removedText, sizeOf } from "./variables";

const v = (name: string, kind: string, size?: number): Variable => ({ name, kind, class: "x", size, children: false, summary: "" });

describe("variables", () => {
  const list = [v("seu", "data", 412 * 1024 * 1024), v("res", "value", 56), v("plot_qc", "function"), v("markers", "data", 1024 * 1024), v("odd", "other")];

  it("groups data, values and functions, and filters by name", () => {
    expect(groupVariables(list).map((g) => [g.title, g.variables.map((x) => x.name)])).toEqual([
      ["Data", ["markers", "odd", "seu"]],
      ["Values", ["res"]],
      ["Functions", ["plot_qc"]],
    ]);
    expect(groupVariables(list, " MARK").map((g) => [g.title, g.variables.map((x) => x.name)])).toEqual([["Data", ["markers"]]]);
  });

  it("says what deleting frees", () => {
    const bytes = sizeOf(list, ["seu", "markers", "plot_qc"]);
    expect(bytes).toBe(413 * 1024 * 1024);
    expect(removedText(["seu", "markers"], bytes)).toBe("Removed seu, markers · 413 MB freed");
    expect(removedText(["a", "b", "c", "d", "e"], 0)).toBe("Removed a, b, c and 2 more");
  });
});
