import { afterEach, describe, expect, it, vi } from "vitest";
import type { CellOutput } from "./cellOutput";
import { followOutput, revealDelta } from "./follow";

const view = { top: 0, bottom: 500 };

describe("revealDelta", () => {
  it("shows the whole cell when it fits", () => {
    // Below the view: scrolls until its end shows.
    expect(revealDelta(view, { top: 300, bottom: 700 }, { top: 400, bottom: 700 })).toBe(208);
    // Already shown.
    expect(revealDelta(view, { top: 100, bottom: 300 }, { top: 200, bottom: 300 })).toBe(0);
    // Above the view: scrolls back to its start.
    expect(revealDelta(view, { top: -100, bottom: 200 }, { top: 0, bottom: 200 })).toBe(-108);
  });

  it("shows as much of a tall output as fits, from its start", () => {
    // Its end fits once the code goes off the top.
    expect(revealDelta(view, { top: 100, bottom: 800 }, { top: 400, bottom: 800 })).toBe(308);
    // Taller than the view: its start comes to the top.
    expect(revealDelta(view, { top: 100, bottom: 2000 }, { top: 300, bottom: 2000 })).toBe(292);
  });
});

/** An element whose box on screen is `box()`. */
function placed(box: () => { top: number; bottom: number }): HTMLElement {
  const el = document.body.appendChild(document.createElement("div"));
  el.getBoundingClientRect = () => ({ ...box(), left: 0, right: 100, width: 100, height: box().bottom - box().top, x: 0, y: box().top, toJSON() {} });
  return el;
}

describe("followOutput", () => {
  let resized: (() => void) | undefined;
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resized = callback;
      }
      observe() {}
      disconnect() {
        resized = undefined;
      }
    },
  );
  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("keeps the output in view as it grows, until the user scrolls", async () => {
    let height = 100;
    const scroller = placed(() => ({ top: 0, bottom: 500 }));
    // The cell starts at 300, less what has been scrolled.
    const top = () => 300 - scroller.scrollTop;
    const element = placed(() => ({ top: top() + 50, bottom: top() + 50 + height }));
    const output = { element, info: { phase: "running" } } as unknown as CellOutput;
    const stop = followOutput(output, scroller, top);
    await Promise.resolve();
    expect(scroller.scrollTop).toBe(0);
    // A plot arrives.
    height = 400;
    resized?.();
    expect(scroller.scrollTop).toBe(258);
    // The user scrolls: following stops.
    scroller.dispatchEvent(new WheelEvent("wheel"));
    height = 600;
    resized?.();
    expect(scroller.scrollTop).toBe(258);
    expect(resized).toBeUndefined();
    stop();
  });

  it("stops a little after the run ends", () => {
    vi.useFakeTimers();
    const scroller = placed(() => ({ top: 0, bottom: 500 }));
    const element = placed(() => ({ top: 0, bottom: 10 }));
    const output = { element, info: { phase: "running" } } as unknown as CellOutput;
    followOutput(output, scroller, () => 0);
    vi.advanceTimersByTime(5000);
    expect(resized).toBeDefined();
    output.info!.phase = "done";
    vi.advanceTimersByTime(1000);
    expect(resized).toBeDefined();
    vi.advanceTimersByTime(1000);
    expect(resized).toBeUndefined();
  });
});
