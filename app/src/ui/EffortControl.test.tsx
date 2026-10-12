import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import { EffortControl } from "./EffortControl";

const LEVELS = ["low", "medium", "high", "xhigh", "max"];

function mount(start: string | undefined, disabled?: string) {
  const root = document.body.appendChild(document.createElement("div"));
  const [value, setValue] = createSignal(start);
  const changes: string[] = [];
  const dispose = render(
    () => (
      <EffortControl
        levels={LEVELS}
        value={value()}
        recommended="high"
        disabled={disabled}
        onChange={(level) => {
          changes.push(level);
          setValue(level);
        }}
      />
    ),
    root,
  );
  const track = root.querySelector(".eff-track") as HTMLElement;
  const key = (key: string) => track.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
  return {
    root,
    track,
    key,
    changes,
    name: () => root.querySelector(".eff-v")?.textContent,
    done() {
      dispose();
      root.remove();
    },
  };
}

describe("EffortControl", () => {
  it("shows the recommended level when none is chosen, with its tick", () => {
    const view = mount(undefined);
    expect(view.name()).toBe("High");
    expect(view.track.getAttribute("aria-valuenow")).toBe("2");
    expect(view.root.querySelectorAll(".eff-stop.rec")).toHaveLength(1);
    expect(view.root.querySelector(".eff-rec")?.textContent).toBe("Recommended");
    view.done();
  });

  it("steps with the arrow keys, Home and End, and turns to stars at the top", () => {
    const view = mount("high");
    view.key("ArrowRight");
    expect(view.name()).toBe("Extra high");
    view.key("End");
    expect(view.name()).toBe("Max");
    expect(view.track.classList.contains("galaxy")).toBe(true);
    view.key("ArrowRight");
    view.key("Home");
    expect(view.changes).toEqual(["xhigh", "max", "low"]);
    expect(view.track.classList.contains("galaxy")).toBe(false);
    view.done();
  });

  it("can't be changed when disabled", () => {
    const view = mount("medium", "Codex changes effort in its own /model menu");
    view.key("ArrowRight");
    expect(view.changes).toEqual([]);
    expect(view.root.querySelector(".eff")?.getAttribute("title")).toContain("/model");
    view.done();
  });
});
