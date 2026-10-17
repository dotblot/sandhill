import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import type { DirEntry } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { FileTree } from "./FileTree";

const dir = (name: string): DirEntry => ({ name, kind: "dir", size: 0, mtime_ms: 0 });
const file = (name: string): DirEntry => ({ name, kind: "file", size: 10, mtime_ms: 0 });

const FILES: Record<string, DirEntry[]> = {
  "/p": [dir("a"), file("z.py")],
  "/p/a": [dir("b"), file("y.py")],
  "/p/a/b": [file("x.py")],
};

const settle = () => new Promise((r) => setTimeout(r, 0));

describe("FileTree", () => {
  it("draws guide lines from each open folder's chevron to its children", async () => {
    const ws = { call: async (_method: string, params: { path: string }) => FILES[params.path] ?? [] } as unknown as Workspace;
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <FileTree ws={ws} root="/p" refreshKey={0} onOpen={() => {}} />, root);
    await settle();
    const item = (name: string) => [...root.querySelectorAll(".tree-item")].find((r) => r.textContent?.includes(name)) as HTMLElement;
    item("a").click();
    await settle();
    item("b").click();
    await settle();
    const rows = [...root.querySelectorAll(".tree-item")].map((row) => {
      const lines = [...row.querySelectorAll(".tg, .te")].map((g) => `${g.className.replace(" ", ".")}@${(g as HTMLElement).style.left}`);
      return [row.querySelector(".tree-name")?.textContent, ...lines].join(" ");
    });
    expect(rows).toEqual([
      "a",
      // a's line runs on past b to y.py.
      "b tg@14px te@14px",
      "x.py tg@14px tg.end@29px te@29px",
      // The last child: a's line stops at its middle.
      "y.py tg.end@14px te@14px",
      "z.py",
    ]);
    expect(item("a").getAttribute("aria-expanded")).toBe("true");
    dispose();
    root.remove();
  });
});
