import { describe, expect, it } from "vitest";
import type { Output } from "../backend/types";
import { CellOutput, outputSummary } from "./cellOutput";

const ctx = {
  blobUrl: (hash: string) => `blob://${hash}`,
  blobText: async () => "",
  onResize: () => {},
};

const plot: Output = {
  output_type: "display_data",
  data: { "image/png": { sandhill_blob: "a".repeat(64), size: 10 }, "text/plain": "<Figure>" },
  metadata: {},
};
const table: Output = {
  output_type: "execute_result",
  data: { "text/html": "<table><tr><td>1</td></tr></table>", "text/plain": "df" },
  metadata: {},
};
const text: Output = { output_type: "stream", name: "stdout", text: "hello\n" };

describe("CellOutput", () => {
  it("keeps a hidden plot out of the page until it is shown", () => {
    const toggles: boolean[] = [];
    const output = new CellOutput(ctx, () => {}, false, (v) => toggles.push(v));
    output.showSaved({ phase: "done", status: "ok", code: "plot()", outputs: [text, plot] });
    const status = output.element.querySelector(".cell-status") as HTMLElement;

    expect(output.element.querySelector("img")).toBeNull();
    expect(status.textContent).toContain("text, plot");

    status.click();
    expect(output.isVisible).toBe(true);
    expect(output.element.querySelector("img")).not.toBeNull();
    expect(status.textContent).not.toContain("plot");

    status.click();
    expect(output.element.querySelector("img")).toBeNull();
    expect(toggles).toEqual([true, false]);
  });

  it("shows outputs that arrive while it is shown", () => {
    const output = new CellOutput(ctx, () => {}, true);
    output.showSaved({ phase: "running", code: "x", outputs: [text] });
    expect(output.element.querySelector(".output-area")!.textContent).toContain("hello");
  });
});

describe("outputSummary", () => {
  it("names what a hidden output holds", () => {
    expect(outputSummary([plot])).toBe("plot");
    expect(outputSummary([plot, plot, table])).toBe("2 plots, table");
    expect(outputSummary([text, text])).toBe("text");
    expect(outputSummary([{ output_type: "error", ename: "E", evalue: "", traceback: [] }])).toBe("error");
  });
});
