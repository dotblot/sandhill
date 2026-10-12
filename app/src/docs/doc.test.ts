import { history, undo } from "@codemirror/commands";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";
import { changedOnDisk, replaceText } from "./doc";

function editor(text: string, onChange: (fromDisk: boolean) => void = () => {}) {
  return new EditorView({
    parent: document.body,
    state: EditorState.create({
      doc: text,
      extensions: [
        history(),
        EditorView.updateListener.of((u) => u.docChanged && onChange(changedOnDisk(u.transactions))),
      ],
    }),
  });
}

describe("replaceText", () => {
  it("changes only the part that differs, so the cursor stays put", () => {
    const view = editor("library(Seurat)\nseu <- readRDS('a.rds')\nseu\n");
    // The cursor on the last line, after the change.
    view.dispatch({ selection: { anchor: 40 } });
    replaceText(view, "library(Seurat)\nseu <- readRDS('pbmc.rds')\nseu\n");
    expect(view.state.doc.toString()).toBe("library(Seurat)\nseu <- readRDS('pbmc.rds')\nseu\n");
    expect(view.state.selection.main.head).toBe(43);
    view.destroy();
  });

  it("marks the change as coming from disk, and it can be undone", () => {
    const seen: boolean[] = [];
    const view = editor("a\nb\n", (fromDisk) => seen.push(fromDisk));
    replaceText(view, "a\nb\nc\n");
    expect(seen).toEqual([true]);
    undo(view);
    expect(view.state.doc.toString()).toBe("a\nb\n");
    expect(seen).toEqual([true, false]);
    view.destroy();
  });

  it("does nothing when the text is the same", () => {
    const seen: boolean[] = [];
    const view = editor("same", (fromDisk) => seen.push(fromDisk));
    replaceText(view, "same");
    expect(seen).toEqual([]);
    view.destroy();
  });
});
