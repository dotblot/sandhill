import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../backend";
import type { GitCommitInfo, GitLog, GitStatus } from "../backend/types";
import { Workspace } from "../model/workspace";
import { ask, notify } from "./dialogs";
import { type DiffRequest, GitPane } from "./GitPane";

vi.mock("./dialogs", () => ({ ask: vi.fn(async () => "Ignore it"), askText: vi.fn(async () => undefined), notify: vi.fn() }));

const REPO = "/home/huan/pbmc";

const STATUS: GitStatus = {
  repo: REPO,
  branch: "main",
  head: "c3",
  upstream: "origin/main",
  ahead: 1,
  behind: 0,
  files: [
    { path: "analysis.qmd", staged: "M", staged_lines: { added: 12, removed: 0 } },
    { path: "R/01_qc.R", unstaged: "M", lines: { added: 3, removed: 1 } },
    { path: "pbmc3k.h5ad", unstaged: "?", size: 60 * 1024 * 1024 },
  ],
  more: false,
  stashes: 0,
  refs: "r1",
  branches: ["exp/harmony", "main"],
  remote_branches: ["origin/main"],
  remotes: ["origin"],
  user_name: "Huan Yang",
  user_email: "huan@example.org",
};

const LOG: GitLog = {
  more: false,
  commits: [
    {
      sha: "c3",
      parents: ["c1"],
      author: "Huan Yang",
      email: "huan@example.org",
      time_ms: Date.now() - 3600_000,
      subject: "Label clusters with marker genes",
      refs: [{ kind: "head", name: "main" }],
    },
    {
      sha: "c2",
      parents: ["c1"],
      author: "Codex",
      email: "codex@openai.com",
      time_ms: Date.now() - 7200_000,
      subject: "Try harmony integration",
      refs: [{ kind: "branch", name: "exp/harmony" }],
    },
    {
      sha: "c1",
      parents: [],
      author: "Huan Yang",
      email: "huan@example.org",
      time_ms: Date.now() - 86400_000,
      subject: "Add QC violin plots",
      refs: [
        { kind: "remote", name: "origin/main" },
        { kind: "tag", name: "v0.1.0" },
      ],
      coauthors: ["Claude <noreply@anthropic.com>"],
    },
  ],
};

const SHOWN: GitCommitInfo = {
  sha: "c1",
  parents: [],
  author: "Huan Yang",
  email: "huan@example.org",
  time_ms: 0,
  message: "Add QC violin plots\n\nCo-authored-by: Claude <noreply@anthropic.com>",
  files: [{ path: "analysis.qmd", status: "A", lines: { added: 30, removed: 0 } }],
};

function fakeWorkspace(status: GitStatus = STATUS) {
  const calls: { method: string; params: Record<string, any> }[] = [];
  const backend: Backend = {
    host: "local",
    async call<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
      calls.push({ method, params });
      switch (method) {
        case "list_sessions":
          return [] as T;
        case "git_status":
          return status as T;
        case "git_log":
          return LOG as T;
        case "git_show":
          return SHOWN as T;
        case "git_run":
          return { output: "" } as T;
        case "git_write_message":
          return { summary: "Add QC violin plots", body: "Per sample.", agent: "claude" } as T;
        case "read_file":
          throw new Error("no such file");
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
  const ws = new Workspace(backend, REPO);
  const runs = () => calls.filter((c) => c.method === "git_run").map((c) => c.params.action);
  return { ws, calls, runs };
}

const settle = async () => {
  for (let i = 0; i < 6; i++) await new Promise((resolve) => setTimeout(resolve, 0));
};

function mount(ws: Workspace) {
  const diffs: DiffRequest[] = [];
  const opened: string[] = [];
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(
    () => <GitPane ws={ws} project={REPO} shown onDiff={(r) => diffs.push(r)} onOpen={(p) => opened.push(p)} />,
    root,
  );
  return { root, dispose, diffs, opened };
}

const texts = (root: HTMLElement, selector: string) => [...root.querySelectorAll(selector)].map((e) => e.textContent);

beforeEach(() => {
  vi.mocked(ask).mockClear();
  vi.mocked(notify).mockClear();
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("GitPane", () => {
  it("shows the branch, the graph with avatars and refs, and the changed files", async () => {
    const { ws } = fakeWorkspace();
    const { root, dispose, diffs, opened } = mount(ws);
    await settle();
    expect(root.querySelector(".gbranch")?.textContent).toBe("main");
    expect(root.querySelector(".ghead .ahead")?.textContent).toBe("↑1");
    expect(texts(root, ".gsubj")).toEqual(["Working tree", "Label clusters with marker genes", "Try harmony integration", "Add QC violin plots"]);
    expect(texts(root, ".gcol-msg.wt .gbadge")).toEqual(["1 staged", "2 unstaged"]);
    // The checked-out branch, a branch, and a tag beside origin/main.
    expect(texts(root, ".gcol-ref .gn")).toEqual(["main", "exp/harmony", "origin/main"]);
    expect(texts(root, ".gcol-ref .gref.more")).toEqual(["+1"]);
    // People's initials, Codex's and Claude Code's.
    expect(texts(root, ".ggraph text")).toEqual(["HY", "CX", "CC"]);
    // Two lanes: the branch beside main.
    expect(root.querySelector(".ggraph")?.getAttribute("width")).toBe("40");

    expect(texts(root, ".gsec-t")).toEqual(["Staged", "Unstaged"]);
    expect(texts(root, ".gfile .gname")).toEqual(["analysis.qmd", "01_qc.R", "pbmc3k.h5ad"]);
    expect(texts(root, ".gfile .gdir")).toEqual(["", "R", ""]);
    expect(texts(root, ".gfile .gst")).toEqual(["M", "M", "A"]);
    expect(texts(root, ".gfile .gbig")).toEqual(["60 MB"]);
    (root.querySelectorAll(".gfile")[1] as HTMLElement).click();
    expect(diffs).toEqual([{ repo: REPO, file: "R/01_qc.R", staged: false }]);
    (root.querySelector('.gfile button[aria-label="Open analysis.qmd"]') as HTMLButtonElement).click();
    expect(opened).toEqual([`${REPO}/analysis.qmd`]);
    expect(diffs).toHaveLength(1);
    dispose();
  });

  it("shows a picked commit's message and files", async () => {
    const { ws, calls } = fakeWorkspace();
    const { root, dispose, diffs } = mount(ws);
    await settle();
    (root.querySelectorAll(".grow")[3] as HTMLElement).click();
    await settle();
    expect(calls.some((c) => c.method === "git_show" && c.params.sha === "c1")).toBe(true);
    expect(root.querySelector(".gd-who")?.textContent).toBe("Huan Yang with Claude Code");
    expect(root.querySelector(".gd-msg")?.textContent).toContain("Add QC violin plots");
    (root.querySelector(".gdetail .gfile") as HTMLElement).click();
    expect(diffs).toEqual([{ repo: REPO, file: "analysis.qmd", staged: false, sha: "c1" }]);
    (root.querySelector('.gdetail button[aria-label="Close"]') as HTMLButtonElement).click();
    await settle();
    expect(root.querySelector(".gdetail")).toBeNull();
    expect(root.querySelector(".gmsg")).not.toBeNull();
    dispose();
  });

  it("stages, asks an agent for the message, and commits", async () => {
    const { ws, runs } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    (root.querySelector('.gfile button[aria-label="Stage R/01_qc.R"]') as HTMLButtonElement).click();
    await settle();
    expect(runs()).toEqual([{ op: "stage", files: ["R/01_qc.R"] }]);

    // A file too big for GitHub: Ignore it adds it to .gitignore, which is staged instead.
    (root.querySelector('.gfile button[aria-label="Stage pbmc3k.h5ad"]') as HTMLButtonElement).click();
    await settle();
    expect(ask).toHaveBeenCalledOnce();
    expect(vi.mocked(ask).mock.calls[0][0]).toBe("Stage pbmc3k.h5ad? It is 60 MB.");
    expect(runs()[1]).toEqual({ op: "stage", files: [".gitignore"] });

    (root.querySelector(".gwrite") as HTMLButtonElement).click();
    await settle();
    const summary = root.querySelector('input[aria-label="Commit summary"]') as HTMLInputElement;
    const body = root.querySelector('textarea[aria-label="Commit description"]') as HTMLTextAreaElement;
    expect(summary.value).toBe("Add QC violin plots");
    expect(body.value).toBe("Per sample.");
    summary.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true }));
    await settle();
    expect(runs()[2]).toEqual({ op: "commit", message: "Add QC violin plots\n\nPer sample.", amend: false });
    expect(summary.value).toBe("");
    dispose();
  });

  it("merges a branch after asking, and settles a conflict by taking a side", async () => {
    const { ws, runs } = fakeWorkspace();
    const { root, dispose } = mount(ws);
    await settle();
    (root.querySelector('button[title="Branches"]') as HTMLButtonElement).click();
    await settle();
    const item = (label: string) => [...document.querySelectorAll(".gmenu .mm-i")].find((b) => b.textContent === label) as HTMLButtonElement;
    item("Merge into main…").click();
    await settle();
    expect(texts(document.body, ".gmenu .mm-h")).toEqual(["Merge into main"]);
    expect(texts(document.body, ".gmenu .mm-i")).toEqual(["exp/harmony", "origin/main"]);
    vi.mocked(ask).mockResolvedValueOnce("Merge");
    item("exp/harmony").click();
    await settle();
    expect(vi.mocked(ask).mock.calls[0][0]).toBe("Merge exp/harmony into main?");
    expect(runs()).toEqual([{ op: "merge", branch: "exp/harmony" }]);
    dispose();

    const merging = fakeWorkspace({
      ...STATUS,
      state: "merging",
      merge_message: "Merge branch 'exp/harmony'",
      files: [{ path: "R/qc.R", unstaged: "M", conflict: true }],
    });
    const second = mount(merging.ws);
    await settle();
    expect(second.root.querySelector(".gstate")?.textContent).toContain("or pick Mine or Theirs");
    const summary = second.root.querySelector('input[aria-label="Commit summary"]') as HTMLInputElement;
    expect(summary.value).toBe("Merge branch 'exp/harmony'");
    (second.root.querySelector('.gfile.conflict button[title^="Take"]') as HTMLButtonElement).click();
    await settle();
    expect(merging.runs()).toEqual([{ op: "resolve", file: "R/qc.R", side: "theirs" }]);
    // Ending a merge commits even with nothing new staged.
    expect((second.root.querySelector(".gcommit button") as HTMLButtonElement).disabled).toBe(false);
    second.dispose();
  });

  it("offers to start a repository, and asks for a name and email to commit", async () => {
    const { ws, runs } = fakeWorkspace({ ...STATUS, repo: "" });
    const { root, dispose } = mount(ws);
    await settle();
    expect(root.querySelector(".gnone p")?.textContent).toBe("This project isn't a Git repository yet.");
    (root.querySelector(".gnone button") as HTMLButtonElement).click();
    await settle();
    expect(runs()).toEqual([{ op: "init" }]);
    dispose();

    const fresh = fakeWorkspace({ ...STATUS, user_name: "", user_email: "" });
    const second = mount(fresh.ws);
    await settle();
    const name = second.root.querySelector('input[aria-label="Your name"]') as HTMLInputElement;
    const email = second.root.querySelector('input[aria-label="Your email"]') as HTMLInputElement;
    name.value = "Huan Yang";
    name.dispatchEvent(new InputEvent("input", { bubbles: true }));
    email.value = "huan@example.org";
    email.dispatchEvent(new InputEvent("input", { bubbles: true }));
    (second.root.querySelector(".guser button") as HTMLButtonElement).click();
    await settle();
    expect(fresh.runs()).toEqual([{ op: "set_user", name: "Huan Yang", email: "huan@example.org" }]);
    second.dispose();
  });
});
