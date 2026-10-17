import { describe, expect, it } from "vitest";
import { askView, buildChat, type ChatEntry, type ChatEvent, displayName, type Step, stepView } from "./chat";
import recorded from "./chat.recorded.json";

// A real conversation, as the daemon kept it: Claude looked around, asked to
// add a cell, was allowed, and answered.
const real = recorded as ChatEntry[];

let seq = 0;
const entry = (event: ChatEvent): ChatEntry => ({ seq: seq++, event });
const turn = (id: string, events: ChatEvent[]) => events.map((e) => entry({ turn: id, ...e }));

describe("buildChat", () => {
  it("shows a recorded conversation", () => {
    const chat = buildChat(real, false);
    const kinds = chat.items.map((i) => i.kind);
    expect(kinds).toEqual(["user", "step", "step", "step", "step", "reply", "footer"]);
    expect(chat.title).toMatch(/^Use add_cell to add a cell/);
    expect(chat.waiting).toEqual([]);

    const user = chat.items[0] as { place?: { file: string } };
    expect(user.place?.file).toBe("Untitled.qmd");

    const steps = chat.items.filter((i): i is Step => i.kind === "step");
    // In the order they happened, not the order the hooks reported them.
    expect(steps.map((s) => displayName(s.tool))).toEqual(["get_focus", "list_sessions", "bash", "add_cell"]);
    expect(steps.every((s) => s.status === "done")).toBe(true);
    // Each request is shown on its step, with the answer.
    const add = steps[3]!;
    expect(add.ask?.answer).toEqual({ behavior: "allow" });
    expect(steps[2]!.ask).toBeUndefined();

    const footer = chat.items[6] as { text: string; ms?: number };
    expect(footer.text).toMatch(/^I added an R cell/);
    expect(footer.ms).toBeGreaterThan(0);
  });

  it("waits for an answer while the turn goes on", () => {
    const entries = real.filter((e) => e.event.kind !== "stop" && e.event.kind !== "text");
    // Drop the add_cell answer and its result.
    const unanswered = entries.filter((e) => e.seq < 14);
    const chat = buildChat(unanswered, true);
    expect(chat.waiting.map((a) => displayName(a.tool))).toEqual(["add_cell"]);
    const add = chat.items.find((i): i is Step => i.kind === "step" && i.tool.endsWith("add_cell"))!;
    expect(add.status).toBe("wait");
    // Something waits on the user, so it isn't "thinking".
    expect(chat.items.at(-1)?.kind).toBe("step");
  });

  it("thinks after the last step until the turn ends", () => {
    const chat = buildChat(
      turn("t1", [
        { kind: "prompt", text: "hi", at_ms: 1 },
        { kind: "step", id: "a", tool: "Bash", input: { command: "ls" }, at_ms: 2 },
        { kind: "step_done", id: "a", tool: "Bash", output: { stdout: "x" }, ms: 5, at_ms: 3 },
      ]),
      true,
    );
    expect(chat.items.map((i) => i.kind)).toEqual(["user", "step", "thinking"]);
  });

  it("puts a reply's parts together in order", () => {
    const chat = buildChat(
      turn("t1", [
        { kind: "prompt", text: "hi", at_ms: 1 },
        { kind: "text", message: "m", index: 1, text: "world", at_ms: 3 },
        { kind: "text", message: "m", index: 0, text: "Hello ", at_ms: 2 },
        { kind: "stop", at_ms: 4 },
      ]),
      false,
    );
    expect(chat.items[1]).toMatchObject({ kind: "reply", text: "Hello world" });
    expect(chat.items[2]).toMatchObject({ kind: "footer", text: "Hello world", ms: 3 });
  });

  it("marks steps of a stopped turn, and a denied one", () => {
    const entries = [
      ...turn("t1", [
        { kind: "prompt", text: "one", at_ms: 1 },
        { kind: "step", id: "a", tool: "Bash", input: { command: "rm x" }, at_ms: 2 },
        { kind: "ask", id: "q1", tool: "Bash", input: { command: "rm x" }, at_ms: 3 },
      ]),
      entry({ kind: "answered", id: "q1", answer: { behavior: "deny" }, at_ms: 4 }),
      ...turn("t2", [
        { kind: "prompt", text: "two", at_ms: 5 },
        { kind: "step", id: "b", tool: "Bash", input: { command: "sleep 100" }, at_ms: 6 },
      ]),
    ];
    const chat = buildChat(entries, false);
    const steps = chat.items.filter((i): i is Step => i.kind === "step");
    expect(steps.map((s) => s.status)).toEqual(["deny", "cancel"]);
    expect(chat.items.filter((i) => i.kind === "footer")).toHaveLength(2);
    expect(chat.waiting).toEqual([]);
  });

  it("shows a question with no step as its own card", () => {
    const chat = buildChat(
      turn("t1", [
        { kind: "prompt", text: "plan it", at_ms: 1 },
        { kind: "ask", id: "q", tool: "AskUserQuestion", input: { questions: [{ question: "Which?", options: [{ label: "A" }] }] }, at_ms: 2 },
      ]),
      true,
    );
    expect(chat.items.map((i) => i.kind)).toEqual(["user", "ask"]);
    expect(chat.waiting).toHaveLength(1);
    const view = askView(chat.waiting[0]!);
    expect(view.questions?.[0]?.question).toBe("Which?");
  });

  it("puts Codex's requests on the step they name", () => {
    const chat = buildChat(
      turn("t1", [
        { kind: "prompt", text: "make a file", at_ms: 1 },
        { kind: "step", id: "call_1", tool: "Bash", input: { command: "echo hi > made.txt" }, at_ms: 2 },
        // The request has the command's reason in its input, so the input doesn't match the step's.
        {
          kind: "ask",
          id: "q1",
          step: "call_1",
          tool: "Bash",
          input: { command: "echo hi > made.txt", description: "Write made.txt" },
          remember: ["this command"],
          scopes: ["chat"],
          at_ms: 3,
        },
      ]),
      true,
    );
    expect(chat.items.map((i) => i.kind)).toEqual(["user", "step"]);
    const step = chat.items[1] as Step;
    expect(step.status).toBe("wait");
    expect(step.ask).toMatchObject({ id: "q1", scopes: ["chat"] });
    expect(chat.waiting.map((a) => a.id)).toEqual(["q1"]);
  });

  it("shows what went wrong, in a turn or before one", () => {
    const chat = buildChat(
      [
        entry({ kind: "session", source: "startup", model: "gpt-5.5" }),
        entry({ kind: "error", text: "Codex couldn't compact the conversation" }),
        ...turn("t1", [
          { kind: "prompt", text: "hi", at_ms: 1 },
          { kind: "error", text: "You've hit your usage limit.", at_ms: 2 },
          { kind: "stop", at_ms: 3 },
        ]),
      ],
      false,
    );
    expect(chat.items.map((i) => i.kind)).toEqual(["error", "user", "error", "footer"]);
    expect(chat.items[2]).toMatchObject({ text: "You've hit your usage limit." });
  });

  it("notes a compacted or resumed conversation", () => {
    const chat = buildChat([entry({ kind: "session", source: "startup" }), entry({ kind: "session", source: "compact" })], false);
    expect(chat.items).toEqual([expect.objectContaining({ kind: "note", text: "Conversation compacted" })]);
  });
});

describe("stepView", () => {
  it("describes Sandhill's tools", () => {
    const steps = buildChat(real, false).items.filter((i): i is Step => i.kind === "step");
    const add = stepView(steps[3]!);
    expect(add.name).toBe("add_cell");
    expect(add.code).toContain("data.frame");
    expect(add.diff?.every((l) => l.startsWith("+"))).toBe(true);
    expect(add.output).toBeTruthy();
    expect(stepView(steps[0]!).desc).toBe("See where you are");
    // An edit has only the new code, so it shows as code, not as a diff.
    const edit = stepView({ tool: "mcp__sandhill__edit_cell", input: { cell: "chunk-4", code: "a\nb" } }, "analysis.qmd");
    expect(edit).toMatchObject({ desc: "Change chunk-4 in analysis.qmd", detail: "2 lines", code: "a\nb" });
    expect(edit.diff).toBeUndefined();
  });

  it("shows a question with its answer", () => {
    const question = { question: "Which plot?", header: "Plot", multiSelect: false, options: [{ label: "Bar" }, { label: "Scatter" }] };
    const view = stepView({ tool: "AskUserQuestion", input: { questions: [question] }, output: { questions: [question], answers: { "Which plot?": "Scatter" } } });
    expect(view).toMatchObject({ name: "question", desc: "Which plot?", detail: "Scatter", output: "Which plot?\n› Scatter" });
  });

  it("describes Claude Code's own tools", () => {
    expect(stepView({ tool: "Bash", input: { command: "ls -la\npwd", description: "List files" }, output: { stdout: "a", stderr: "" } })).toMatchObject({
      name: "bash",
      desc: "List files",
      output: "a",
    });
    const edit = stepView({ tool: "Edit", input: { file_path: "/p/a.R", old_string: "x <- 1", new_string: "x <- 2\ny <- 3" } });
    expect(edit).toMatchObject({ name: "edit", desc: "Edit a.R", added: 2, removed: 1, detail: "+2 −1" });
    const read = stepView({
      tool: "Read",
      input: { file_path: "/p/a.R" },
      output: { file: { content: "a\nb", startLine: 1, numLines: 2, totalLines: 10 } },
    });
    expect(read.detail).toBe("lines 1–2 of 10");
    expect(read.output).toBe("   1  a\n   2  b");
    expect(displayName("mcp__github__create_issue")).toBe("github · create_issue");
  });

  it("describes Codex's own steps", () => {
    const add = stepView({ tool: "apply_patch", input: { changes: [{ path: "R/qc.R", kind: "add", diff: "a <- 1\nb <- 2\n" }] }, output: { status: "completed" } });
    expect(add).toMatchObject({ name: "edit", desc: "Add qc.R", file: "R/qc.R", added: 2, removed: 0, diff: ["+a <- 1", "+b <- 2"] });
    const edits = stepView({
      tool: "apply_patch",
      input: {
        changes: [
          { path: "a.R", kind: "update", diff: "@@ -1,2 +1,2 @@\n keep\n-x <- 1\n+x <- 2\n@@ -9 +9 @@\n-y\n+z\n" },
          { path: "old.R", kind: "delete", diff: "gone\n" },
        ],
      },
    });
    expect(edits).toMatchObject({ desc: "Edit 2 files", added: 2, removed: 3 });
    expect(edits.file).toBeUndefined();
    expect(edits.diff).toEqual([" a.R", " keep", "-x <- 1", "+x <- 2", " ⋯", "-y", "+z", " old.R", "-gone"]);
    const moved = stepView({ tool: "apply_patch", input: { changes: [{ path: "a.R", kind: "update", move_path: "b.R", diff: "" }] } });
    expect(moved.desc).toBe("Move a.R to b.R");

    const thinking = stepView({ tool: "reasoning", input: { text: "**Planning the QC plot**\n\nI'll read the data first." } });
    expect(thinking).toMatchObject({ name: "thinking", desc: "Planning the QC plot", output: "**Planning the QC plot**\n\nI'll read the data first." });
    expect(stepView({ tool: "view_image", input: { path: "/p/umap.png" } })).toMatchObject({ name: "view_image", desc: "Look at umap.png" });
    expect(stepView({ tool: "WebSearch", input: { query: null }, output: { query: "scanpy umap" } }).desc).toBe("scanpy umap");
  });

  it("asks Codex's questions in its words", () => {
    const patch = askView({ id: "1", tool: "apply_patch", input: { changes: [{ path: "a.R", kind: "update", diff: "-a\n+b" }], reason: "Outside the folder" } });
    expect(patch).toMatchObject({ title: "Edit a.R?", file: "a.R", detail: "Outside the folder" });
    const access = askView(
      { id: "2", tool: "permissions", input: { reason: "To download the data", permissions: { network: { enabled: true }, fileSystem: { write: ["/data"] } } } },
      undefined,
      "Codex",
    );
    expect(access).toMatchObject({ title: "Give Codex more access?", detail: "To download the data" });
    expect(access.view.output).toBe("Use the network\nWrite to /data");
    const server = askView({ id: "3", tool: "mcp__github", input: { message: "Sign in to GitHub", url: "https://github.com/login" } });
    expect(server).toMatchObject({ title: "Sign in to GitHub", detail: "https://github.com/login" });
    expect(displayName("mcp__github")).toBe("github");
    expect(askView({ id: "4", tool: "AskUserQuestion", input: { questions: [] } }, undefined, "Codex").title).toBe("Codex has a question");
  });

  it("finds images in a tool's result", () => {
    const view = stepView({ tool: "mcp__sandhill__run_code", input: { code: "plot(1)" }, output: [{ type: "text", text: "Ran in R session 1 · ok · 0.2 s" }, { type: "image", data: "AAA", mimeType: "image/png" }] });
    expect(view.images).toEqual(["data:image/png;base64,AAA"]);
    // As Claude Code's hooks get it, with where it saved the image.
    const hooked = stepView({
      tool: "mcp__sandhill__add_cell",
      input: { code: "plot(1)" },
      output: [
        { type: "text", text: "Added R cell chunk-4 to analysis.qmd below cell chunk-3, and Sandhill ran it · ok · 0.4 s\n[plot 1]\n" },
        { type: "image", source: { type: "base64", media_type: "image/png", data: "BBB" } },
        { type: "text", text: "[Image: source: /home/u/.claude/projects/p/1/tool-results/mcp-sandhill-blob-1.png]" },
      ],
    });
    expect(hooked.images).toEqual(["data:image/png;base64,BBB"]);
    expect(hooked.output).not.toContain("Image: source");
    expect(view.detail).toBe("R · 0.2 s");
  });
});
