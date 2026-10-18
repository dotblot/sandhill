import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import { Workspace } from "../model/workspace";
import { DiffView, diffTab, diffTitle } from "./DiffView";

vi.mock("./dialogs", () => ({ notify: vi.fn() }));

const REPO = "/home/huan/pbmc";

function fakeWorkspace() {
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
            text: staged ? "" : "@@ -1,3 +1,3 @@ library(Seurat)\n x <- 1\n-y <- 2\n+y <- 3\n z <- 4\n",
            truncated: false,
            binary: false,
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
    expect(root.querySelector(".dl.hunk")?.textContent).toBe("Lines 1–3library(Seurat)");
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
});
