import { describe, expect, it } from "vitest";
import { attachmentName, filterMentions, type Mention, typedMention } from "./mentions";

describe("mentions", () => {
  it("finds the @ being typed before the caret, not one inside a word", () => {
    expect(typedMention("look at @an", 11)).toEqual({ at: 8, query: "an" });
    expect(typedMention("@", 1)).toEqual({ at: 0, query: "" });
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

  it("names pasted images by the time, as macOS names screenshots", () => {
    const at = new Date(2026, 9, 25, 9, 5, 3);
    expect(attachmentName("image.png", at)).toBe("Pasted image 2026-10-25 at 09.05.03.png");
    expect(attachmentName("gel/blot.tif", at)).toBe("gel-blot.tif");
  });
});
