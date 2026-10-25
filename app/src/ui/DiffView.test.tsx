import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import { Workspace } from "../model/workspace";
import { DiffView, diffTab, diffTitle } from "./DiffView";

const asked: string[] = [];
vi.mock("./dialogs", () => ({
  notify: vi.fn(),
  ask: vi.fn(async (message: string) => {
    asked.push(message);
    return "Throw away";
  }),
}));

const REPO = "/home/huan/pbmc";

const TWO_HUNKS = "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -10,2 +10,3 @@\n x\n+new\n+more\n y\n";

function fakeWorkspace(text?: string) {
  const calls: { method: string; params: Record<string, any> }[] = [];
  let staged = false;
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "git_diff":
          return {
            file: params.file,
            text: staged ? "" : (text ?? "@@ -1,3 +1,3 @@ library(Seurat)\n x <- 1\n-y <- 2\n+y <- 3\n z <- 4\n"),
            truncated: false,
            binary: false,
            lines: !params.sha,
          } as T;
        case "git_run":
          staged = true;
          return { output: "" } as T;
        default:
          return null as T;
      }
    },
    onEvent() {},
    onTerminal() {},
    onRequest() {},
    openLink() {},
    onLink() {},
    blobUrl: () => "",
    reconnect: async () => {},
    disconnect: async () => {},
  };
  return { ws: new Workspace(backend, REPO), calls };
}

const settle = async () => {
  for (let i = 0; i < 6; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

afterEach(() => {
  document.body.replaceChildren();
});

describe("DiffView", () => {
  it("names its tab by what it compares", () => {
    expect(diffTitle(diffTab({ repo: REPO, file: "R/qc.R", staged: false }))).toBe("qc.R (changes)");
    expect(diffTitle(diffTab({ repo: REPO, file: "R/qc.R", staged: true }))).toBe("qc.R (staged)");
    expect(diffTitle(diffTab({ repo: REPO, file: "R/qc.R", staged: false, sha: "abcdef123" }))).toBe("qc.R @ abcdef1");
    expect(diffTab({ repo: REPO, file: "a", staged: true }).id).not.toBe(diffTab({ repo: REPO, file: "a", staged: false }).id);
  });

  it("shows the lines with their numbers, and stages the file", async () => {
    const { ws, calls } = fakeWorkspace();
    const staged: string[] = [];
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(
      () => <DiffView ws={ws} tab={diffTab({ repo: REPO, file: "R/qc.R", staged: false })} shown onOpen={() => {}} onStaged={() => staged.push("yes")} />,
      root,
    );
    await settle();
    expect(calls[0]).toEqual({ method: "git_diff", params: { path: REPO, file: "R/qc.R", staged: false, sha: undefined } });
    expect(root.querySelector(".dl.hunk")?.textContent).toBe("Lines 1–3StageThrow awaylibrary(Seurat)");
    const rows = [...root.querySelectorAll(".dl:not(.hunk)")].map((row) => [...row.children].map((c) => c.textContent));
    expect(rows).toEqual([
      ["1", "1", "", "x <- 1"],
      ["2", "", "−", "y <- 2"],
      ["", "2", "+", "y <- 3"],
      ["3", "3", "", "z <- 4"],
    ]);
    expect(root.querySelector(".dcount")?.textContent).toBe("+1−1");
    ([...root.querySelectorAll(".diffbar button")].find((b) => b.textContent === "Stage") as HTMLButtonElement).click();
    await settle();
    expect(calls.find((c) => c.method === "git_run")?.params).toEqual({ path: REPO, action: { op: "stage", files: ["R/qc.R"] } });
    expect(staged).toEqual(["yes"]);
    expect(root.querySelector(".vempty")?.textContent).toBe("No changes left in this file.");
    dispose();
  });

  it("stages the lines picked by their numbers, and a hunk from its header", async () => {
    const { ws, calls } = fakeWorkspace(TWO_HUNKS);
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <DiffView ws={ws} tab={diffTab({ repo: REPO, file: "R/qc.R", staged: false })} shown onOpen={() => {}} />, root);
    await settle();
    const row = (text: string) => [...root.querySelectorAll(".dl")].find((r) => r.querySelector(".dt")?.textContent === text)!;
    const down = (text: string, init: MouseEventInit = {}) =>
      row(text).querySelector(".dn")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0, ...init }));
    expect(root.querySelector(".dhint")?.textContent).toBe("Click line numbers to pick lines");
    // Context lines and the text itself pick nothing.
    down("a");
    row("B").querySelector(".dt")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
    expect(root.querySelectorAll(".dl.picked").length).toBe(0);
    // A click, then Shift-click adds the range between.
    down("b");
    window.dispatchEvent(new MouseEvent("mouseup"));
    down("new", { shiftKey: true });
    expect([...root.querySelectorAll(".dl.picked .dt")].map((d) => d.textContent)).toEqual(["b", "B", "new"]);
    // Clicking a picked line unpicks it.
    down("B");
    window.dispatchEvent(new MouseEvent("mouseup"));
    expect([...root.querySelectorAll(".dl.picked .dt")].map((d) => d.textContent)).toEqual(["b", "new"]);
    const button = (text: string) => [...root.querySelectorAll("button")].find((b) => b.textContent === text) as HTMLButtonElement;
    button("Stage 2 lines").click();
    await settle();
    expect(calls.filter((c) => c.method === "git_run").map((c) => c.params.action)).toEqual([
      { op: "stage_lines", file: "R/qc.R", diff: TWO_HUNKS, lines: [2, 7] },
    ]);
    dispose();

    const second = fakeWorkspace(TWO_HUNKS);
    const again = document.body.appendChild(document.createElement("div"));
    const done = render(() => <DiffView ws={second.ws} tab={diffTab({ repo: REPO, file: "R/qc.R", staged: false })} shown onOpen={() => {}} />, again);
    await settle();
    const hunks = again.querySelectorAll(".dl.hunk");
    ([...hunks[1].querySelectorAll("button")].find((b) => b.textContent === "Throw away") as HTMLButtonElement).click();
    await settle();
    expect(asked).toEqual(["Throw away these changes to qc.R?"]);
    expect(second.calls.find((c) => c.method === "git_run")?.params.action).toEqual({ op: "discard_lines", file: "R/qc.R", diff: TWO_HUNKS, lines: [7, 8] });
    done();
  });

  it("offers no picking in a commit's diff", async () => {
    const { ws } = fakeWorkspace(TWO_HUNKS);
    const root = document.body.appendChild(document.createElement("div"));
    const dispose = render(() => <DiffView ws={ws} tab={diffTab({ repo: REPO, file: "R/qc.R", staged: false, sha: "abcdef123" })} shown onOpen={() => {}} />, root);
    await settle();
    expect(root.querySelector(".dhint")).toBeNull();
    expect(root.querySelector(".dhacts")).toBeNull();
    expect(root.querySelector(".dl.pickable")).toBeNull();
    dispose();
  });
});
