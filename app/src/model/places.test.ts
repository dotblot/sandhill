import { describe, expect, it } from "vitest";
import { closeTab, openPlace, shortPath, shownPlace, windowTitle } from "./places";

const local = { host: "local", path: "/Users/huan/thesis" };
const gpu = { host: "gpu01", path: "/home/huan/pbmc-atlas" };

describe("window places", () => {
  it("opens a project in its machine's tab", () => {
    let w = openPlace({ tabs: [], current: null }, local);
    w = openPlace(w, gpu);
    expect(w).toEqual({ tabs: [local, gpu], current: "gpu01" });
    // Another project on this Mac switches the Local tab to it, in place.
    const other = { host: "local", path: "/Users/huan/scratch" };
    w = openPlace(w, other);
    expect(w.tabs).toEqual([other, gpu]);
    expect(shownPlace(w)).toEqual(other);
  });

  it("closes a tab and shows the one before it, or the Start screen", () => {
    const w = { tabs: [local, gpu], current: "gpu01" };
    expect(closeTab(w, "gpu01")).toEqual({ tabs: [local], current: "local" });
    expect(closeTab(w, "local")).toEqual({ tabs: [gpu], current: "gpu01" });
    expect(closeTab({ tabs: [gpu], current: "gpu01" }, "gpu01")).toEqual({ tabs: [], current: null });
  });

  it("names the window and shortens home folders", () => {
    expect(windowTitle({ tabs: [local, gpu], current: "gpu01" })).toBe("pbmc-atlas — gpu01");
    expect(windowTitle({ tabs: [local], current: "local" })).toBe("thesis");
    expect(windowTitle({ tabs: [local], current: null })).toBe("Sandhill");
    expect(shortPath("/Users/huan/thesis")).toBe("~/thesis");
    expect(shortPath("/data/huan/atac", "/data/huan")).toBe("~/atac");
    expect(shortPath("/scratch/run")).toBe("/scratch/run");
  });
});
