// An agent's conversation as the chat pane shows it. The daemon keeps what
// the agent's hooks report (crates/daemon/src/terminal/chat.rs): the user's
// messages, the parts of each reply, each step with its result, and what the
// agent asks the user. Here that becomes a list of items: your messages,
// replies, one row per step, cards for what waits on you, and a footer after
// each turn.

export interface Place {
  file?: string;
  cell?: string;
  line?: number;
  /** The names of what the user added to the message with @. */
  refs?: string[];
}

/** What the user answers to an `ask`: allow (once, for the chat or the project), or deny. */
export interface Answer {
  behavior: "allow" | "deny";
  scope?: "once" | "chat" | "project";
  /** For a question: each question's answer. */
  answers?: Record<string, string>;
  message?: string;
}

export interface ChatEvent {
  kind: string;
  turn?: string;
  at_ms?: number;
  text?: string;
  place?: Place;
  message?: string;
  index?: number;
  final?: boolean;
  id?: string;
  tool?: string;
  // Tools' inputs and outputs are whatever the agent's tools take and give.
  // biome-ignore lint: free-form JSON
  input?: any;
  // biome-ignore lint: free-form JSON
  output?: any;
  // biome-ignore lint: free-form JSON
  error?: any;
  interrupted?: boolean;
  ms?: number;
  answer?: Answer | null;
  source?: string;
  model?: string;
  /** For an ask: what allowing it for the chat or the project allows from then on. */
  remember?: string[];
  /** For an ask: for how long it can be allowed (`chat`, `project`); both when not given. */
  scopes?: string[];
  /** For an ask: the id of the step it is about, when the agent says. */
  step?: string;
}

export interface ChatEntry {
  seq: number;
  event: ChatEvent;
}

/** A request waiting for the user (or answered). `answer` is undefined while it waits, null if the agent stopped waiting. */
export interface Ask {
  id: string;
  tool: string;
  // biome-ignore lint: free-form JSON
  input: any;
  answer?: Answer | null;
  remember?: string[];
  scopes?: string[];
}

export type StepStatus = "run" | "wait" | "done" | "fail" | "deny" | "cancel";

export interface Step {
  kind: "step";
  key: string;
  id: string;
  tool: string;
  // biome-ignore lint: free-form JSON
  input: any;
  // biome-ignore lint: free-form JSON
  output?: any;
  error?: string;
  ms?: number;
  status: StepStatus;
  ask?: Ask;
}

export type ChatItem =
  | { kind: "user"; key: string; text: string; place?: Place }
  | { kind: "reply"; key: string; text: string }
  | Step
  /** A request with no step to show it on. */
  | { kind: "ask"; key: string; ask: Ask }
  | { kind: "footer"; key: string; at_ms?: number; ms?: number; text: string }
  | { kind: "note"; key: string; text: string }
  /** Something went wrong: the agent couldn't start a turn, or a turn failed. */
  | { kind: "error"; key: string; text: string }
  | { kind: "thinking"; key: string };

export interface Chat {
  items: ChatItem[];
  /** Requests waiting for the user, oldest first. */
  waiting: Ask[];
  /** The first message, as a title. */
  title?: string;
}

interface Turn {
  id: string;
  prompt?: ChatEvent;
  events: ChatEntry[];
  stop?: ChatEvent;
}

/**
 * The chat from the conversation's entries. `working` is whether the agent
 * says it is busy: a turn the agent was stopped in has no `stop` entry.
 */
export function buildChat(entries: ChatEntry[], working: boolean): Chat {
  const answers = new Map<string, Answer | null>();
  for (const { event } of entries) {
    if (event.kind === "answered" && event.id) answers.set(event.id, event.answer ?? null);
  }
  // Turns in the order they started, with the notes between them.
  const order: (Turn | ChatEntry)[] = [];
  const turns = new Map<string, Turn>();
  for (const entry of entries) {
    const { event } = entry;
    if (event.kind === "answered") continue;
    if (!event.turn) {
      if (event.kind === "session" || event.kind === "error") order.push(entry);
      continue;
    }
    let turn = turns.get(event.turn);
    if (!turn) {
      turn = { id: event.turn, events: [] };
      turns.set(event.turn, turn);
      order.push(turn);
    }
    if (event.kind === "prompt") turn.prompt = event;
    else if (event.kind === "stop") turn.stop = event;
    else turn.events.push(entry);
  }

  const items: ChatItem[] = [];
  const waiting: Ask[] = [];
  let title: string | undefined;
  const lastTurn = [...order].reverse().find((t): t is Turn => "events" in t);
  for (const part of order) {
    if (!("events" in part)) {
      if (part.event.kind === "error") {
        items.push({ kind: "error", key: `e${part.seq}`, text: part.event.text ?? "" });
        continue;
      }
      const note = sessionNote(part.event);
      if (note) items.push({ kind: "note", key: `n${part.seq}`, text: note });
      continue;
    }
    const turn = part;
    // A turn has ended when the agent said so, or when another began, or the agent is idle.
    const ended = !!turn.stop || turn !== lastTurn || !working;
    if (turn.prompt?.text !== undefined) {
      items.push({ kind: "user", key: `u${turn.id}`, text: turn.prompt.text, place: turn.prompt.place });
      title ??= titleOf(turn.prompt.text);
    }
    const steps = new Map<string, Step>();
    const replies = new Map<string, { item: { kind: "reply"; key: string; text: string }; parts: Map<number, string> }>();
    const replyTexts: string[] = [];
    // Hooks run side by side: put things in the order they happened.
    const events = [...turn.events].sort((a, b) => (a.event.at_ms ?? 0) - (b.event.at_ms ?? 0) || a.seq - b.seq);
    for (const { seq, event } of events) {
      switch (event.kind) {
        case "step": {
          const id = event.id ?? `s${seq}`;
          const step: Step = { kind: "step", key: `s${id}`, id, tool: event.tool ?? "", input: event.input ?? {}, status: "run" };
          steps.set(id, step);
          items.push(step);
          break;
        }
        case "step_done":
        case "step_failed": {
          const id = event.id ?? `s${seq}`;
          let step = steps.get(id);
          if (!step) {
            step = { kind: "step", key: `s${id}`, id, tool: event.tool ?? "", input: {}, status: "run" };
            steps.set(id, step);
            items.push(step);
          }
          if (event.kind === "step_done") step.output = event.output;
          else step.error = errorText(event.error) || (event.interrupted ? "Stopped" : "Failed");
          step.ms = event.ms;
          break;
        }
        case "ask": {
          const ask: Ask = {
            id: event.id ?? `a${seq}`,
            tool: event.tool ?? "",
            input: event.input ?? {},
            answer: answers.get(event.id ?? ""),
            remember: event.remember,
            scopes: event.scopes,
          };
          if (ask.answer === undefined && !ended) waiting.push(ask);
          const step = askedStep(steps, ask, event.step);
          if (step) step.ask = ask;
          else items.push({ kind: "ask", key: `a${ask.id}`, ask });
          break;
        }
        case "error":
          items.push({ kind: "error", key: `e${seq}`, text: event.text ?? "" });
          break;
        case "text": {
          const id = event.message ?? `m${seq}`;
          let reply = replies.get(id);
          if (!reply) {
            reply = { item: { kind: "reply", key: `r${id}`, text: "" }, parts: new Map() };
            replies.set(id, reply);
            items.push(reply.item);
          }
          reply.parts.set(event.index ?? reply.parts.size, event.text ?? "");
          reply.item.text = [...reply.parts.entries()]
            .sort((a, b) => a[0] - b[0])
            .map(([, text]) => text)
            .join("");
          break;
        }
      }
    }
    for (const step of steps.values()) step.status = stepStatus(step, ended);
    for (const reply of replies.values()) replyTexts.push(reply.item.text.trim());
    if (ended) {
      const start = turn.prompt?.at_ms;
      const end = turn.stop?.at_ms;
      items.push({
        kind: "footer",
        key: `f${turn.id}`,
        at_ms: end,
        ms: start !== undefined && end !== undefined ? end - start : undefined,
        text: replyTexts.filter(Boolean).join("\n\n"),
      });
    } else if (!waiting.length) {
      items.push({ kind: "thinking", key: `t${turn.id}` });
    }
  }
  return { items, waiting, title };
}

/**
 * The step a request is about: the one it names (Codex), else the latest one
 * of that tool with the same input (Claude Code's hooks don't say).
 */
function askedStep(steps: Map<string, Step>, ask: Ask, id: string | undefined): Step | undefined {
  // A step can ask more than once (Codex: more access, then the command); the latest shows.
  const named = id !== undefined ? steps.get(id) : undefined;
  if (named) return named;
  const same = JSON.stringify(ask.input);
  return [...steps.values()]
    .reverse()
    .find((s) => s.tool === ask.tool && !s.ask && s.output === undefined && JSON.stringify(s.input) === same);
}

function stepStatus(step: Step, ended: boolean): StepStatus {
  if (step.output !== undefined) return "done";
  if (step.error !== undefined) return "fail";
  if (step.ask?.answer?.behavior === "deny") return "deny";
  if (ended) return "cancel";
  if (step.ask && step.ask.answer === undefined) return "wait";
  return "run";
}

function sessionNote(event: ChatEvent): string | undefined {
  switch (event.source) {
    case "compact":
      return "Conversation compacted";
    case "clear":
      return "Conversation cleared";
    case "resume":
      return "Conversation resumed";
    default:
      return undefined;
  }
}

/** A conversation's title: its first message. */
export function chatTitle(entries: ChatEntry[]): string | undefined {
  const first = entries.find((e) => e.event.kind === "prompt" && e.event.text)?.event.text;
  return first ? titleOf(first) : undefined;
}

/** When the conversation last changed, if it has. */
export function lastActive(entries: ChatEntry[]): number | undefined {
  for (let i = entries.length - 1; i >= 0; i--) {
    const at = entries[i]?.event.at_ms;
    if (at) return at;
  }
  return undefined;
}

/**
 * A title from a first message (draft 31): its first few words, without
 * file names, @ mentions, "Please" or "Can you", or a trailing "in" or "of".
 */
export function titleOf(text: string): string {
  const first = (text.trim().split(/[.?!](?:\s|$)|\n/)[0] ?? "")
    .replace(/@\S+/g, " ")
    .replace(/^\s*(please|can you|could you|would you)\s+/i, "");
  const words: string[] = [];
  for (const raw of first.split(/\s+/)) {
    const w = raw.replace(/^[`"'“‘(]+|[`"'”’),:;]+$/g, "");
    if (!w) continue;
    // A file name ends the title once it has a few words, else it is left out.
    if (w.includes("/") || /\.[A-Za-z][A-Za-z0-9]*$/.test(w)) {
      if (words.length >= 3) break;
      continue;
    }
    words.push(w);
    if (words.length === 6) break;
  }
  while (words.length > 2 && /^(in|of|for|to|the|a|an|and|with|on|from|at|by)$/i.test(words[words.length - 1] ?? "")) words.pop();
  const title = words.join(" ");
  return title ? title.charAt(0).toUpperCase() + title.slice(1) : "New conversation";
}

// biome-ignore lint: free-form JSON
function errorText(error: any): string {
  if (error === undefined || error === null) return "";
  if (typeof error === "string") return error;
  return contentText(error) || JSON.stringify(error);
}

/** The text of an MCP tool's result: `[{type: "text", text}, …]`. */
// biome-ignore lint: free-form JSON
export function contentText(value: any): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) {
    return (
      value
        // Claude Code adds where it saved each image: "[Image: source: /…/tool-results/….png]".
        .filter((c) => c?.type === "text" && typeof c.text === "string" && !/^\[Image: source: [^\]]+\]$/.test(c.text))
        .map((c) => c.text)
        .join("\n")
    );
  }
  if (value && typeof value === "object" && Array.isArray(value.content)) return contentText(value.content);
  return "";
}

/** Images in a tool's result, as data URLs: MCP's `{type: "image", data, mimeType}`, or the
 * `{type: "image", source: {data, media_type}}` Claude Code's hooks get. */
// biome-ignore lint: free-form JSON
export function contentImages(value: any): string[] {
  const items: unknown[] = Array.isArray(value) ? value : Array.isArray(value?.content) ? value.content : [];
  return items.flatMap((c) => {
    // biome-ignore lint: free-form JSON
    const item = c as any;
    if (item?.type !== "image") return [];
    const data = item.data ?? item.source?.data;
    const type = item.mimeType ?? item.source?.media_type ?? "image/png";
    return typeof data === "string" ? [`data:${type};base64,${data}`] : [];
  });
}

/** How a step's row and card show it. */
export interface StepView {
  /** In bold code font: `run_code`, `bash`, `read`. */
  name: string;
  /** A short plain description. */
  desc: string;
  /** A file the description names, which opens on a click. */
  file?: string;
  /** On the right: "R · 0.5 s", "lines 1–40 of 120", "+7 −0". */
  detail?: string;
  added?: number;
  removed?: number;
  code?: string;
  language?: string;
  /** Where the code ran, for the card's header. */
  where?: string;
  output?: string;
  images: string[];
  /** A diff: lines with "+", "-" or " " in front. */
  diff?: string[];
}

const SANDHILL = "mcp__sandhill__";

/** What a step did, for its row and its card. */
export function stepView(step: Pick<Step, "tool" | "input" | "output" | "error">, linked?: string): StepView {
  const input = step.input ?? {};
  const view: StepView = { name: displayName(step.tool), desc: "", images: contentImages(step.output) };
  const firstLine = (text: string) => (text ?? "").trim().split("\n")[0] ?? "";
  if (step.tool.startsWith(SANDHILL)) {
    const text = contentText(step.output);
    const [head, ...rest] = text.split("\n");
    const ran = sandhillRun(head ?? "");
    switch (view.name) {
      case "run_code":
        view.desc = input.description ?? firstLine(input.code);
        view.code = input.code;
        view.language = input.language ?? ran.language;
        view.where = head;
        view.output = rest.join("\n").trimEnd();
        view.detail = ran.detail;
        break;
      case "add_cell":
      case "edit_cell": {
        const file = input.file ?? ran.file ?? linked;
        view.file = file;
        view.desc = view.name === "add_cell" ? `Add a cell to ${file ?? "the document"}` : `Change ${input.cell ?? "a cell"} in ${file ?? "the document"}`;
        view.code = input.code;
        view.language = input.language ?? ran.language;
        view.where = head;
        view.output = rest.join("\n").trimEnd();
        if (view.name === "add_cell") {
          view.added = lineCount(input.code);
          view.removed = 0;
          view.diff = (input.code ?? "").split("\n").map((l: string) => `+${l}`);
        } else {
          // The cell's old code isn't in the call, so there's no diff to show.
          const n = lineCount(input.code);
          view.detail = n === 1 ? "1 line" : `${n} lines`;
        }
        break;
      }
      case "list_variables":
        view.desc = input.language ? `List the ${languageName(input.language)} session's variables` : "List the session's variables";
        view.output = text;
        break;
      case "list_sessions":
        view.desc = "List the running sessions";
        view.output = text;
        break;
      case "get_focus":
        view.desc = "See where you are";
        view.output = text;
        break;
      case "get_outputs":
        view.desc = input.cell ? `Read the outputs of ${input.cell}` : "Read the outputs";
        view.file = input.file;
        view.output = text;
        break;
      case "list_jobs":
        view.desc = "List the jobs";
        view.output = text;
        break;
      case "read_job_log":
        view.desc = input.job ? `Read the log of job ${input.job}` : "Read the latest job's log";
        view.output = text;
        break;
      default:
        view.desc = "";
        view.output = text;
    }
    return view;
  }
  // biome-ignore lint: free-form JSON
  const out: any = step.output ?? {};
  switch (step.tool) {
    case "Bash":
      view.desc = input.description || firstLine(input.command);
      view.code = input.command;
      view.language = "shell";
      view.output = [out.stdout, out.stderr].filter((s) => typeof s === "string" && s).join("\n");
      break;
    case "Read": {
      view.file = input.file_path;
      view.desc = baseName(input.file_path);
      const file = out.file ?? {};
      if (typeof file.content === "string") {
        const start = file.startLine ?? 1;
        view.output = file.content
          .split("\n")
          .map((line: string, i: number) => `${String(start + i).padStart(4)}  ${line}`)
          .join("\n");
        if (file.numLines !== undefined) {
          const last = start + file.numLines - 1;
          view.detail = file.totalLines && file.totalLines > file.numLines ? `lines ${start}–${last} of ${file.totalLines}` : `${file.numLines} lines`;
        }
      }
      break;
    }
    case "Edit":
    case "MultiEdit": {
      view.file = input.file_path;
      view.desc = `Edit ${baseName(input.file_path)}`;
      const edits = step.tool === "Edit" ? [input] : (input.edits ?? []);
      const diff: string[] = edits.flatMap((e: { old_string?: string; new_string?: string }) => [
        ...(e.old_string ?? "").split("\n").map((l) => `-${l}`),
        ...(e.new_string ?? "").split("\n").map((l) => `+${l}`),
      ]);
      view.diff = diff;
      view.added = diff.filter((l) => l.startsWith("+")).length;
      view.removed = diff.filter((l) => l.startsWith("-")).length;
      view.detail = `+${view.added} −${view.removed}`;
      break;
    }
    case "Write":
      view.file = input.file_path;
      view.desc = `Write ${baseName(input.file_path)}`;
      view.diff = (input.content ?? "").split("\n").map((l: string) => `+${l}`);
      view.added = lineCount(input.content);
      view.removed = 0;
      view.detail = `+${view.added}`;
      break;
    case "NotebookEdit":
      view.file = input.notebook_path;
      view.desc = `Edit a cell in ${baseName(input.notebook_path)}`;
      view.code = input.new_source;
      break;
    case "Grep":
      view.desc = `Search for “${input.pattern ?? ""}”${input.path ? ` in ${baseName(input.path)}` : ""}`;
      view.output = typeof out.content === "string" ? out.content : (out.filenames ?? []).join("\n");
      if (typeof out.numFiles === "number") view.detail = `${out.numFiles} ${out.numFiles === 1 ? "file" : "files"}`;
      break;
    case "Glob":
      view.desc = `Find ${input.pattern ?? "files"}`;
      view.output = (out.filenames ?? []).join("\n");
      if (typeof out.numFiles === "number") view.detail = `${out.numFiles} ${out.numFiles === 1 ? "file" : "files"}`;
      break;
    case "WebSearch":
      // Codex knows the query only once it has searched.
      view.desc = input.query || out.query || "";
      break;
    case "WebFetch":
      view.desc = input.url ?? "";
      break;
    case "Task":
    case "Agent":
      view.desc = input.description ?? "";
      view.output = contentText(out) || (typeof out === "string" ? out : "");
      break;
    case "ToolSearch": {
      // Claude Code loads tools it was told about only by name.
      const query: string = input.query ?? "";
      const names = query.startsWith("select:") ? query.slice(7).split(",").filter(Boolean) : [];
      view.desc = names.length ? `Load ${names.map((n) => (n.startsWith("mcp__") ? displayName(n.trim()) : n.trim())).join(", ")}` : `Find tools for “${query}”`;
      if (Array.isArray(out.matches)) view.output = out.matches.join("\n");
      break;
    }
    case "AskUserQuestion": {
      const questions: { question?: string }[] = input.questions ?? [];
      const answers: Record<string, string> = out.answers ?? {};
      view.desc = questions.length === 1 ? (questions[0]?.question ?? "") : `Ask ${questions.length} questions`;
      const given = questions.map((q) => answers[q.question ?? ""]).filter(Boolean);
      if (given.length) view.detail = given.join(", ");
      view.output = questions.map((q) => `${q.question ?? ""}\n› ${answers[q.question ?? ""] ?? "no answer"}`).join("\n\n");
      break;
    }
    case "ExitPlanMode":
      view.desc = "Share a plan";
      view.output = input.plan ?? "";
      break;
    case "apply_patch": {
      // Codex's edits: each file's change, as a whole file or a unified diff.
      const changes: PatchChange[] = input.changes ?? [];
      const one = changes.length === 1 ? changes[0] : undefined;
      view.file = one?.path;
      view.desc = one ? patchTitle(one) : `Edit ${changes.length} files`;
      view.diff = changes.flatMap((change) => [...(one ? [] : [` ${change.path}`]), ...patchLines(change)]);
      view.added = view.diff.filter((l) => l.startsWith("+")).length;
      view.removed = view.diff.filter((l) => l.startsWith("-")).length;
      break;
    }
    case "reasoning": {
      // Codex's summary of what it is thinking: a bold title, then more.
      const text: string = input.text ?? out.text ?? "";
      view.desc = firstLine(text).replace(/\*\*/g, "");
      view.output = text;
      break;
    }
    case "view_image":
      view.file = input.path;
      view.desc = `Look at ${baseName(input.path)}`;
      break;
    case "permissions":
      view.desc = input.reason ?? "";
      view.output = permissionLines(input.permissions).join("\n");
      break;
    case "TodoWrite":
      view.desc = "Update the to-do list";
      view.output = (input.todos ?? [])
        .map((t: { content?: string; status?: string }) => `${t.status === "completed" ? "✓" : t.status === "in_progress" ? "›" : "○"} ${t.content ?? ""}`)
        .join("\n");
      break;
    default:
      view.desc = "";
      if (step.output !== undefined) view.output = contentText(step.output) || JSON.stringify(step.output, null, 1);
      else if (Object.keys(input).length) view.output = JSON.stringify(input, null, 1);
  }
  return view;
}

/** "run_code" for Sandhill's tools, "bash" or "read" for the agent's own, "github · create_issue" for other servers'. */
export function displayName(tool: string): string {
  if (tool.startsWith(SANDHILL)) return tool.slice(SANDHILL.length);
  const mcp = /^mcp__(.+?)__(.+)$/.exec(tool);
  if (mcp) return `${mcp[1]} · ${mcp[2]}`;
  // A request from a server (Codex): `mcp__github`.
  const server = /^mcp__(.+)$/.exec(tool);
  if (server) return server[1] ?? tool;
  const names: Record<string, string> = {
    WebSearch: "web_search",
    WebFetch: "web_fetch",
    TodoWrite: "todo",
    MultiEdit: "edit",
    NotebookEdit: "edit_notebook",
    ToolSearch: "tools",
    AskUserQuestion: "question",
    ExitPlanMode: "plan",
    apply_patch: "edit",
    reasoning: "thinking",
  };
  return names[tool] ?? tool.toLowerCase();
}

/** What Sandhill's run tools say first: "Ran in R session 1a2b (R 4.4.1, kernel ark) · idle · in /data · ok · 0.5 s". */
function sandhillRun(head: string): { language?: string; detail?: string; file?: string } {
  const language = /\b(R|Python)\b(?: session)?/.exec(head)?.[1];
  const seconds = /· ([\d.]+ s)\s*$/.exec(head)?.[1];
  const failed = /· (error|failed)\b/.test(head);
  const file = /\bto (\S+?) (?:below|after)\b/.exec(head)?.[1] ?? /\bof (\S+\.\w+)\b/.exec(head)?.[1];
  const parts = [language, failed ? "error" : seconds].filter(Boolean);
  return { language: language?.toLowerCase(), detail: parts.length ? parts.join(" · ") : undefined, file };
}

/** A file in a Codex patch: `diff` is the whole file when it is added or deleted. */
interface PatchChange {
  path: string;
  kind?: "add" | "delete" | "update";
  diff?: string;
  move_path?: string;
}

function patchTitle(change: PatchChange): string {
  const name = baseName(change.path);
  if (change.move_path) return `Move ${name} to ${baseName(change.move_path)}`;
  if (change.kind === "add") return `Add ${name}`;
  if (change.kind === "delete") return `Delete ${name}`;
  return `Edit ${name}`;
}

/** A patch's lines with "+", "-" or " " in front; hunks after the first start with " ⋯". */
function patchLines(change: PatchChange): string[] {
  const lines = (change.diff ?? "").replace(/\n$/, "").split("\n");
  if (change.kind === "add") return lines.map((l) => `+${l}`);
  if (change.kind === "delete") return lines.map((l) => `-${l}`);
  const out: string[] = [];
  for (const line of lines) {
    if (line.startsWith("@@")) {
      if (out.length) out.push(" ⋯");
    } else if (!line.startsWith("--- ") && !line.startsWith("+++ ") && !line.startsWith("\\")) {
      out.push(/^[+\- ]/.test(line) ? line : ` ${line}`);
    }
  }
  return out;
}

/** What more access Codex asks for: `{network: {enabled}, fileSystem: {read, write}}`. */
// biome-ignore lint: free-form JSON
function permissionLines(permissions: any): string[] {
  if (!permissions || typeof permissions !== "object") return [];
  const lines: string[] = [];
  if (permissions.network?.enabled) lines.push("Use the network");
  const files = permissions.fileSystem ?? permissions.file_system ?? {};
  for (const path of files.write ?? []) lines.push(`Write to ${path}`);
  for (const path of files.read ?? []) lines.push(`Read ${path}`);
  return lines.length ? lines : [JSON.stringify(permissions, null, 1)];
}

function lineCount(text: string | undefined): number {
  return text ? text.split("\n").length : 0;
}

function baseName(path: string | undefined): string {
  if (!path) return "";
  return path.split("/").pop() ?? path;
}

function languageName(language: string): string {
  return language.toLowerCase() === "r" ? "R" : language.charAt(0).toUpperCase() + language.slice(1);
}

/** How a request is put to the user. */
export interface AskView {
  title: string;
  /** Under the title: where it goes. */
  detail?: string;
  file?: string;
  view: StepView;
  questions?: Question[];
  /** A plan to approve (Claude's plan mode). */
  plan?: string;
}

export interface Question {
  question: string;
  header?: string;
  multiSelect?: boolean;
  options: { label: string; description?: string }[];
}

export function askView(ask: Ask, linked?: string, agent = "Claude"): AskView {
  const view = stepView({ tool: ask.tool, input: ask.input }, linked);
  const input = ask.input ?? {};
  if (ask.tool === "AskUserQuestion") {
    return { title: `${agent} has a question`, view, questions: input.questions ?? [] };
  }
  if (ask.tool === "ExitPlanMode") {
    return { title: "Carry out this plan?", view, plan: input.plan ?? "" };
  }
  if (/^mcp__/.test(ask.tool) && !ask.tool.slice(5).includes("__")) {
    // A server asking for something other than a tool call (through Codex).
    return { title: input.message || `${view.name} asks for your answer`, detail: input.url ?? undefined, view: { ...view, output: undefined } };
  }
  switch (view.name) {
    case "add_cell":
      return {
        title: `Add a cell to ${view.file ?? "the document"}?`,
        file: view.file,
        detail: `${input.after ? `Below ${input.after}` : "Below your active cell"}${input.run === false ? "" : ", then run it"}`,
        view,
      };
    case "edit_cell":
      return { title: `Change ${input.cell ?? "a cell"} in ${view.file ?? "the document"}?`, file: view.file, detail: input.run === false ? undefined : "Then run it", view };
    case "run_code":
      return { title: `Run this ${view.language ? `${languageName(view.language)} ` : ""}code in your session?`, view };
    case "bash":
      return { title: "Run this command?", detail: input.description, view };
    case "edit":
      // Codex's patches say why, when it asks for more than the sandbox allows.
      if (ask.tool === "apply_patch") return { title: `${view.desc}?`, file: view.file, detail: input.reason, view };
      return { title: `Edit ${baseName(view.file)}?`, file: view.file, view };
    case "write":
      return { title: `Write ${baseName(view.file)}?`, file: view.file, view };
    case "permissions":
      return { title: `Give ${agent} more access?`, detail: input.reason, view: { ...view, desc: "" } };
    case "web_fetch":
      return { title: `Open ${input.url ?? "a web page"}?`, view };
    case "web_search":
      return { title: "Search the web?", detail: input.query, view };
    // Steps that only look, which ask in Restricted mode.
    case "read":
      return { title: `Read ${baseName(view.file) || "a file"}?`, file: view.file, view };
    case "grep":
    case "glob":
    case "list_variables":
    case "list_sessions":
    case "get_focus":
    case "get_outputs":
    case "list_jobs":
    case "read_job_log":
      return { title: view.desc ? `${view.desc}?` : `Allow ${view.name}?`, file: view.file, view };
    default:
      return { title: `Allow ${view.name}?`, detail: view.desc || undefined, view };
  }
}
