import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { baseExtensions } from "../editor/setup";
import { canEdit, lastEditor, runEdit } from "../editor/editing";
import { EditCapsule, MachineTabs } from "./TitleBar";

const mount = (ui: () => JSX.Element) => {
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(ui, root);
  return { root, dispose };
};

describe("title bar", () => {
  afterEach(() => document.body.replaceChildren());

  it("shows a tab per machine, the picked one raised, and a dot where an agent waits", () => {
    const onPick = vi.fn();
    const onAdd = vi.fn();
    const { root, dispose } = mount(() => (
      <MachineTabs
        tabs={[
          { host: "local", path: "/Users/huan/thesis", waiting: true },
          { host: "gpu01", path: "/home/huan/pbmc-atlas", waiting: true },
        ]}
        current="gpu01"
        onPick={onPick}
        onAdd={onAdd}
      />
    ));
    const tabs = [...root.querySelectorAll<HTMLButtonElement>(".mtab")];
    expect(tabs.map((t) => t.textContent)).toEqual(["Local~/thesis", "gpu01~/pbmc-atlas"]);
    expect(tabs.map((t) => t.getAttribute("aria-selected"))).toEqual(["false", "true"]);
    // The machine shown doesn't need a dot: its own Agents button has one.
    expect(tabs.map((t) => !!t.querySelector(".dot"))).toEqual([true, false]);
    tabs[0].click();
    expect(onPick).toHaveBeenCalledWith("local");
    root.querySelector<HTMLButtonElement>(".captabs .tbi")!.click();
    expect(onAdd).toHaveBeenCalled();
    dispose();
  });

  it("dims the edit buttons that can't act and names what Save saves", () => {
    const save = vi.fn();
    const { root, dispose } = mount(() => (
      <EditCapsule buttons={{ undo: () => {}, paste: () => {}, save, saveAll: () => {}, saveName: "qc.R", unsaved: ["qc.R", "de.qmd"] }} />
    ));
    const buttons = [...root.querySelectorAll<HTMLButtonElement>(".tbi")];
    expect(buttons.map((b) => b.disabled)).toEqual([false, true, true, true, false, false, false]);
    expect(buttons[5].title).toBe("Save qc.R ⌘S");
    expect(buttons[6].title).toBe("Save all: qc.R, de.qmd ⌥⌘S");
    buttons[5].click();
    expect(save).toHaveBeenCalled();
    dispose();
  });

  it("acts on the editor last in focus", async () => {
    const parent = document.body.appendChild(document.createElement("div"));
    const view = new EditorView({ state: EditorState.create({ doc: "x <- 1", extensions: baseExtensions() }), parent });
    view.focus();
    view.dispatch({ changes: { from: 6, insert: "0" } });
    expect(lastEditor(parent)).toBe(view);
    expect(lastEditor(document.createElement("div"))).toBeUndefined();
    expect(canEdit(view)).toMatchObject({ undo: true, redo: false, cut: false, copy: false });
    await runEdit(view, "undo");
    expect(view.state.doc.toString()).toBe("x <- 1");
    expect(canEdit(view)).toMatchObject({ undo: false, redo: true });
    view.dispatch({ selection: { anchor: 0, head: 1 } });
    expect(canEdit(view)).toMatchObject({ cut: true, copy: true });
    view.destroy();
  });
});
