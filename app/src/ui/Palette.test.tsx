import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import { Palette } from "./Palette";

const tick = () => new Promise((r) => setTimeout(r, 0));

function setup() {
  const opened: string[] = [];
  const ran: string[] = [];
  let closed = 0;
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => (
      <Palette
        root="/p"
        commands={[
          { label: "Save", keys: "⌘S", run: () => ran.push("save") },
          { label: "Hide console", keys: "⌘J", run: () => ran.push("console") },
        ]}
        files={async () => ["/p/analysis.qmd", "/p/scripts/01_qc.R"]}
        onOpenFile={(path) => opened.push(path)}
        onClose={() => closed++}
      />
    ),
    root,
  );
  const input = root.querySelector("input") as HTMLInputElement;
  const type = (text: string) => {
    input.value = text;
    input.dispatchEvent(new InputEvent("input", { bubbles: true }));
  };
  const key = (k: string) => input.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  const labels = () => [...root.querySelectorAll(".pal-i")].map((i) => i.querySelector(".pal-label")?.textContent);
  return { root, opened, ran, closed: () => closed, type, key, labels, dispose: () => (dispose(), root.remove()) };
}

describe("Palette", () => {
  it("lists files, then commands, and narrows them as you type", async () => {
    const t = setup();
    await tick();
    expect(t.labels()).toEqual(["analysis.qmd", "01_qc.R", "Save", "Hide console"]);
    // Files outside the project's top folder say which folder they are in.
    expect([...t.root.querySelectorAll(".pal-i")].map((i) => i.querySelector("small")?.textContent)).toEqual([
      undefined,
      "scripts",
      "⌘S",
      "⌘J",
    ]);
    t.type("qc");
    expect(t.labels()[0]).toBe("01_qc.R");
    t.key("Enter");
    expect(t.opened).toEqual(["/p/scripts/01_qc.R"]);
    expect(t.closed()).toBe(1);
    t.dispose();
  });

  it("runs a command picked with the arrow keys", async () => {
    const t = setup();
    await tick();
    t.type("console");
    expect(t.labels()).toEqual(["Hide console"]);
    t.key("Enter");
    expect(t.ran).toEqual(["console"]);
    t.type("zzz");
    expect(t.root.querySelector(".pal-none")?.textContent).toBe("Nothing matches");
    t.key("Escape");
    expect(t.closed()).toBe(2);
    t.dispose();
  });
});
