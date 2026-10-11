import { afterEach, describe, expect, it } from "vitest";
import { previousFolder, takeLegacyState } from "./legacy";

describe("tabs from before projects", () => {
  afterEach(() => localStorage.clear());

  it("come back once, for that folder only", () => {
    localStorage.setItem("sandhill.root", "/w/a");
    localStorage.setItem("sandhill.tabs", JSON.stringify(["/w/a/x.qmd", 3]));
    expect(previousFolder()).toBe("/w/a");
    expect(takeLegacyState("/w/b")).toEqual({});
    expect(takeLegacyState("/w/a")).toEqual({ tabs: ["/w/a/x.qmd"] });
    expect(takeLegacyState("/w/a")).toEqual({});
    expect(previousFolder()).toBeUndefined();
  });
});
