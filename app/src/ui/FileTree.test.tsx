import { createRoot } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DirEntry } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { createTree, FileTree, type TreeActions } from "./FileTree";

const dir = (name: string, size = 0): DirEntry => ({ name, kind: "dir", size, mtime_ms: 0 });
const file = (name: string, size = 10): DirEntry => ({ name, kind: "file", size, mtime_ms: 0 });

const settle = () => new Promise((r) => setTimeout(r, 0));

function setup(files: Record<string, DirEntry[]>, host = "local") {
  const calls: [string, Record<string, unknown>][] = [];
  const failing = new Map<string, string>();
  const ws = {
    backend: { host, reveal: vi.fn(async () => {}) },
    call: async (method: string, params: Record<string, unknown>) => {
      calls.push([method, params]);
      const error = failing.get(method);
      if (error) throw new Error(error);
      if (method === "list_dir") return files[params.path as string] ?? [];
      return { path: params.path ?? params.to, mtime_ms: 1 };
    },
  } as unknown as Workspace;
  const actions: TreeActions = {
    openable: (path) => /\.(qmd|R|py|ipynb|html)$/.test(path),
    openLabel: (path) => (path.endsWith(".html") ? "Open in Viewer" : "Open"),
    open: vi.fn(),
    newText: (name) => `# ${name}\n`,
    release: vi.fn(async (path: string) => [`${path}/x.R`]),
    reopen: vi.fn(),
  };
  const root = document.body.appendChild(document.createElement("div"));
  let tree!: ReturnType<typeof createTree>;
  const dispose = createRoot((dispose) => {
    tree = createTree(() => "/p");
    render(() => <FileTree ws={ws} tree={tree} actions={actions} refreshKey={0} />, root);
    return dispose;
  });
  const row = (name: string) => [...root.querySelectorAll(".tree-item")].find((r) => r.querySelector(".tree-name")?.textContent === name) as HTMLElement;
  const names = () => [...root.querySelectorAll(".tree-item")].map((r) => r.querySelector(".tree-name")?.textContent ?? "(field)");
  const field = () => root.querySelector<HTMLInputElement>(".tname")!;
  const type = (value: string) => {
    field().value = value;
    field().dispatchEvent(new InputEvent("input", { bubbles: true }));
  };
  const key = (k: string) => field().dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  const menuItem = (label: string) =>
    [...document.querySelectorAll<HTMLButtonElement>(".treemenu .mm-i")].find((b) => b.textContent?.startsWith(label))!;
  const rightClick = (el: Element) => el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 20, clientY: 20 }));
  return { root, tree, calls, failing, actions, row, names, field, type, key, menuItem, rightClick, dispose };
}

describe("FileTree", () => {
  let dispose = () => {};
  afterEach(() => {
    dispose();
    document.body.replaceChildren();
    localStorage.clear();
  });

  it("draws a guide line through each open folder's chevron column, with no elbows", async () => {
    const t = setup({ "/p": [dir("a"), file("z.py")], "/p/a": [dir("b"), file("y.py")], "/p/a/b": [file("x.py")] });
    dispose = t.dispose;
    await settle();
    t.row("a").click();
    await settle();
    t.row("b").click();
    await settle();
    const rows = [...t.root.querySelectorAll(".tree-item")].map((row) => {
      const lines = [...row.querySelectorAll(".tg")].map((g) => (g as HTMLElement).style.left);
      return [row.querySelector(".tree-name")?.textContent, (row as HTMLElement).style.paddingLeft, ...lines].join(" ");
    });
    expect(rows).toEqual(["a 8px", "b 24px 15px", "x.py 40px 15px 31px", "y.py 24px 15px", "z.py 8px"]);
    expect(t.root.querySelector(".te")).toBeNull();
    expect(t.row("a").getAttribute("aria-expanded")).toBe("true");
  });

  it("shows dotfiles like .env and .sandhill, but not git's folder", async () => {
    const t = setup({ "/p": [dir(".git"), dir(".sandhill"), file(".env"), file(".DS_Store"), file("a.R")] });
    dispose = t.dispose;
    await settle();
    expect(t.names()).toEqual([".sandhill", ".env", "a.R"]);
  });

  it("sorts by size with folders first, asking the daemon for folder sizes", async () => {
    const t = setup({ "/p": [dir("small", 1), dir("big", 900), file("a.csv", 5), file("b.csv", 50)] });
    dispose = t.dispose;
    await settle();
    expect(t.names()).toEqual(["big", "small", "a.csv", "b.csv"]);
    t.tree.setSort("size");
    await settle();
    expect(t.names()).toEqual(["big", "small", "b.csv", "a.csv"]);
    expect(t.calls).toContainEqual(["list_dir", { path: "/p", sizes: true }]);
    expect(localStorage.getItem("sandhill.treeSort")).toBe("size");
  });

  it("makes a new file in the selected folder from a name field, then opens it", async () => {
    const t = setup({ "/p": [dir("data"), file("a.R")], "/p/data": [file("s.csv")] });
    dispose = t.dispose;
    await settle();
    t.row("data").click();
    await settle();
    t.tree.startNew("qmd");
    await settle();
    // The field is the folder's first child, with the name before .qmd selected.
    expect(t.names()).toEqual(["data", "(field)", "s.csv", "a.R"]);
    expect(t.field().value).toBe("untitled.qmd");
    expect([t.field().selectionStart, t.field().selectionEnd]).toEqual([0, 8]);
    t.type("volcano.qmd");
    t.key("Enter");
    await settle();
    expect(t.calls).toContainEqual(["write_file", { path: "/p/data/volcano.qmd", text: "# volcano.qmd\n", create: true }]);
    expect(t.actions.open).toHaveBeenCalledWith("/p/data/volcano.qmd");
    expect(t.field()).toBeNull();
  });

  it("keeps the field open with a red line when the name is taken or bad", async () => {
    const t = setup({ "/p": [file("a.R")] });
    dispose = t.dispose;
    await settle();
    t.tree.startNew("folder");
    await settle();
    t.type("a:b");
    t.key("Enter");
    await settle();
    expect(t.root.querySelector(".tnew-err")?.textContent).toMatch(/can't contain/);
    t.failing.set("make_dir", "exists: /p/a.R already exists");
    t.type("a.R");
    expect(t.root.querySelector(".tnew-err")).toBeNull();
    t.key("Enter");
    await settle();
    expect(t.root.querySelector(".tnew-err")?.textContent).toBe("“a.R” already exists in p");
    t.key("Escape");
    await settle();
    expect(t.field()).toBeNull();
  });

  it("renames in place, closing and reopening documents open from it", async () => {
    const t = setup({ "/p": [dir("old")] });
    dispose = t.dispose;
    await settle();
    t.rightClick(t.row("old"));
    await settle();
    t.menuItem("Rename…").click();
    await settle();
    expect(t.field().value).toBe("old");
    t.type("new");
    t.key("Enter");
    await settle();
    expect(t.actions.release).toHaveBeenCalledWith("/p/old");
    expect(t.calls).toContainEqual(["move_path", { from: "/p/old", to: "/p/new" }]);
    expect(t.actions.reopen).toHaveBeenCalledWith(["/p/new/x.R"]);
  });

  it("moves things to the Trash on this Mac, and offers the New items on empty space", async () => {
    const t = setup({ "/p": [file("notes.md")] });
    dispose = t.dispose;
    await settle();
    t.rightClick(t.row("notes.md"));
    await settle();
    const labels = [...document.querySelectorAll(".treemenu .mm-i span:nth-child(2)")].map((s) => s.textContent);
    expect(labels).toEqual(["Rename…", "Duplicate", "Reveal in Finder", "Copy path", "Move to Trash"]);
    t.menuItem("Move to Trash").click();
    await settle();
    expect(t.calls).toContainEqual(["delete_path", { path: "/p/notes.md", trash: true }]);

    t.rightClick(t.root.querySelector(".tree")!);
    await settle();
    expect(document.querySelector(".treemenu .mm-h")?.textContent).toBe("New in p");
    expect(t.menuItem("Move to Trash")).toBeUndefined();
  });

  it("opens an HTML report in the Viewer, saying so", async () => {
    const t = setup({ "/p": [file("multiqc_report.html"), file("notes.md")] });
    dispose = t.dispose;
    await settle();
    t.rightClick(t.row("multiqc_report.html"));
    await settle();
    t.menuItem("Open in Viewer").click();
    expect(t.actions.open).toHaveBeenCalledWith("/p/multiqc_report.html");
    t.rightClick(t.row("notes.md"));
    await settle();
    expect(t.menuItem("Open")).toBeUndefined();
  });

  it("duplicates with Finder's names and downloads from a server", async () => {
    const t = setup({ "/p": [file("fig3.qmd"), file("fig3 copy.qmd")] }, "gpu01");
    dispose = t.dispose;
    await settle();
    t.rightClick(t.row("fig3.qmd"));
    await settle();
    expect(t.menuItem("Delete…")).toBeDefined();
    expect(t.menuItem("Reveal in Finder")).toBeUndefined();
    t.menuItem("Duplicate").click();
    await settle();
    expect(t.calls).toContainEqual(["copy_path", { from: "/p/fig3.qmd", to: "/p/fig3 copy 2.qmd" }]);
  });
});
