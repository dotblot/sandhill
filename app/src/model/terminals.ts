// The terminals of a project window: agents (Claude Code, Codex) and shells
// that the daemon keeps running on its machine. The window follows each one
// from the moment it opens: output goes to the terminal's view, agents'
// reports (busy, waiting for you, model and context) update `list`, and an
// agent's conversation, as its hooks report it, goes to `chat`.

import { type Accessor, createSignal, type Setter } from "solid-js";
import type { Backend } from "../backend";
import {
  type AgentInfo,
  type AgentOptions,
  TERMINAL_PREVIEW,
  TERMINAL_SHELL,
  type TerminalEvent,
  type TerminalInfo,
  type TerminalSubscribed,
} from "../backend/types";
import { SUBMIT_DELAY_MS } from "./agents";
import type { Answer, ChatEntry } from "./chat";

/** Output kept for a terminal no view shows yet. Older output is dropped. */
const MAX_PENDING = 4 << 20;

/** Where a terminal's output goes. `reset` comes before a replay that doesn't follow on. */
export interface TerminalSink {
  write(data: Uint8Array): void;
  reset?(): void;
}

interface Follow {
  /** Bytes of output received: where the next output starts. -1 before any. */
  end: number;
  sink?: TerminalSink;
  pending: Uint8Array[];
  pendingBytes: number;
  subscribed: boolean;
}

/** An agent's conversation so far. */
interface Conversation {
  entries: Accessor<ChatEntry[]>;
  set: Setter<ChatEntry[]>;
  /** The next entry's number: where a replay starts. */
  next: number;
  /** Entries received but not shown yet: a replay comes as many events. */
  pending: ChatEntry[];
  flush?: ReturnType<typeof setTimeout>;
}

export class TerminalStore {
  /** This project's terminals, oldest first. */
  readonly list: () => TerminalInfo[];
  private setList: Setter<TerminalInfo[]>;
  private follows = new Map<string, Follow>();
  private chats = new Map<string, Conversation>();

  constructor(
    private backend: Backend,
    /** The project folder; only its terminals are listed. */
    readonly project: string,
  ) {
    [this.list, this.setList] = createSignal<TerminalInfo[]>([]);
    backend.onTerminal((event) => this.onEvent(event));
  }

  /** The agents' terminals: not shells, nor the ones documents' previews run in. */
  agents(): TerminalInfo[] {
    return this.list().filter((t) => t.kind !== TERMINAL_SHELL && t.kind !== TERMINAL_PREVIEW);
  }

  shells(): TerminalInfo[] {
    return this.list().filter((t) => t.kind === TERMINAL_SHELL);
  }

  get(id: string): TerminalInfo | undefined {
    return this.list().find((t) => t.id === id);
  }

  /** Agents waiting for the person: a question or a permission prompt. */
  waiting(): TerminalInfo[] {
    return this.agents().filter((t) => t.running && t.agent?.state === "waiting");
  }

  /**
   * Fetches the project's terminals and follows each, picking up where it
   * left off after a reconnect.
   */
  async refresh() {
    const all = await this.backend.call<TerminalInfo[]>("list_terminals");
    const mine = all.filter((t) => t.project === this.project).sort((a, b) => a.created_ms - b.created_ms);
    this.setList(mine);
    const ids = new Set(mine.map((t) => t.id));
    for (const id of this.follows.keys()) if (!ids.has(id)) this.follows.delete(id);
    for (const id of this.chats.keys()) if (!ids.has(id)) this.forgetChat(id);
    for (const terminal of mine) {
      const follow = this.follows.get(terminal.id);
      if (follow) follow.subscribed = false;
      await this.subscribe(terminal.id);
    }
  }

  listAgents(): Promise<AgentInfo[]> {
    return this.backend.call<AgentInfo[]>("list_agents");
  }

  /** Starts an agent or a shell; `prompt` is an agent's first message. */
  async start(kind: string, options: AgentOptions, size?: { cols: number; rows: number }, prompt?: string): Promise<TerminalInfo> {
    const info = await this.backend.call<TerminalInfo>("start_terminal", {
      kind,
      project: this.project,
      cols: size?.cols,
      rows: size?.rows,
      options,
      prompt,
    });
    // Nothing has been missed: its output starts at 0.
    if (!this.follows.has(info.id)) this.follows.set(info.id, newFollow(0));
    this.setList((list) => [...list.filter((t) => t.id !== info.id), info]);
    await this.subscribe(info.id);
    return info;
  }

  /** Names a conversation (or takes its name away, with a blank name). */
  async rename(id: string, name: string): Promise<TerminalInfo> {
    const info = await this.backend.call<TerminalInfo>("rename_terminal", { terminal: id, name });
    this.update(id, (t) => ({ ...t, name: info.name }));
    return info;
  }

  /**
   * Starts a terminal's program again where it left off, ending it first if
   * it runs: an agent resumes its conversation, with `options` if given and
   * `prompt` as its next message. Its output and conversation go on.
   */
  async restart(id: string, options?: AgentOptions, size?: { cols: number; rows: number }, prompt?: string): Promise<TerminalInfo> {
    const info = await this.backend.call<TerminalInfo>("restart_terminal", {
      terminal: id,
      options,
      cols: size?.cols,
      rows: size?.rows,
      prompt,
    });
    this.update(id, () => info);
    // The old program's stream has ended: follow the new one from where it got to.
    const follow = this.follows.get(id);
    if (follow) follow.subscribed = false;
    await this.subscribe(id);
    return info;
  }

  /** Sends output to `sink`, starting with what came before it attached. */
  attach(id: string, sink: TerminalSink): () => void {
    let follow = this.follows.get(id);
    if (!follow) {
      follow = newFollow(-1);
      this.follows.set(id, follow);
    }
    follow.sink = sink;
    for (const chunk of follow.pending.splice(0)) sink.write(chunk);
    follow.pendingBytes = 0;
    const attached = follow;
    return () => {
      if (attached.sink === sink) attached.sink = undefined;
    };
  }

  input(id: string, data: string): Promise<void> {
    return this.backend.call("terminal_input", { terminal: id, data });
  }

  /** An agent's conversation, as its hooks report it; empty for one that keeps none. */
  chat(id: string): ChatEntry[] {
    return this.conversation(id).entries();
  }

  /** Answers what the agent asked (an `ask` entry). */
  answer(id: string, ask: string, answer: Answer): Promise<void> {
    return this.backend.call("answer_terminal", { terminal: id, id: ask, answer });
  }

  /**
   * Tells an agent whose chat Sandhill drives (Codex) what the user did:
   * `{kind: "say", text}`, `{kind: "interrupt"}`, `{kind: "set", model?, effort?, permission?}`,
   * `{kind: "compact"}` or `{kind: "login", method}`.
   */
  tell(id: string, command: Record<string, unknown>): Promise<void> {
    return this.backend.call("tell_terminal", { terminal: id, command });
  }

  /** Sends a message as if typed: one paste, so line breaks don't send it early, then Enter. */
  async say(id: string, text: string) {
    await this.input(id, `\x1b[200~${text.replace(/\r?\n/g, "\r")}\x1b[201~`);
    await new Promise((done) => setTimeout(done, SUBMIT_DELAY_MS));
    await this.input(id, "\r");
  }

  /** Stops what the agent is doing, as Esc does. */
  interrupt(id: string): Promise<void> {
    return this.input(id, "\x1b");
  }

  resize(id: string, cols: number, rows: number): Promise<void> {
    return this.backend.call("resize_terminal", { terminal: id, cols, rows });
  }

  /** Ends the program (as closing its window would) and forgets the terminal. */
  async close(id: string) {
    await this.backend.call("close_terminal", { terminal: id });
    this.follows.delete(id);
    this.forgetChat(id);
    this.setList((list) => list.filter((t) => t.id !== id));
  }

  private async subscribe(id: string) {
    let follow = this.follows.get(id);
    if (!follow) {
      follow = newFollow(-1);
      this.follows.set(id, follow);
    }
    if (follow.subscribed) return;
    follow.subscribed = true;
    try {
      // From where this window got to; the first time, the recent output.
      const from = follow.end >= 0 ? follow.end : undefined;
      // An agent's conversation too, from the entry this window got to.
      const agent = this.get(id)?.kind !== TERMINAL_SHELL;
      const chat_from = agent ? this.conversation(id).next : undefined;
      const reply = await this.backend.call<TerminalSubscribed>("subscribe_terminal", { terminal: id, from, chat_from });
      // Output we had is gone from the daemon's log: start over from what it kept.
      if (follow.end >= 0 && reply.from > follow.end) {
        follow.end = reply.from;
        follow.sink?.reset?.();
      }
    } catch (err) {
      follow.subscribed = false;
      throw err;
    }
  }

  private onEvent(event: TerminalEvent) {
    const follow = this.follows.get(event.terminal);
    if (!follow) return;
    if (event.data !== undefined && event.offset !== undefined) {
      let bytes = decodeBase64(event.data);
      let offset = event.offset;
      // Output seen before, as when two windows follow one terminal, or a replay overlaps.
      if (follow.end >= 0 && offset < follow.end) {
        bytes = bytes.subarray(Math.min(bytes.length, follow.end - offset));
        offset = follow.end;
      }
      if (bytes.length) {
        follow.end = offset + bytes.length;
        this.deliver(follow, bytes);
      }
    }
    if (event.chat) this.addChat(event.terminal, event.chat);
    // Each report is the agent's whole state.
    if (event.agent) this.update(event.terminal, (t) => ({ ...t, agent: event.agent }));
    // Not the end of a program started again since.
    const exit = event.exit;
    if (exit) this.update(event.terminal, (t) => (exit.ended_ms < (t.started_ms ?? 0) ? t : { ...t, running: false, exit }));
  }

  private conversation(id: string): Conversation {
    let chat = this.chats.get(id);
    if (!chat) {
      const [entries, set] = createSignal<ChatEntry[]>([]);
      chat = { entries, set, next: 0, pending: [] };
      this.chats.set(id, chat);
    }
    return chat;
  }

  private addChat(id: string, entry: ChatEntry) {
    const chat = this.conversation(id);
    // Seen before, as when a replay overlaps.
    if (entry.seq < chat.next) return;
    chat.next = entry.seq + 1;
    chat.pending.push(entry);
    chat.flush ??= setTimeout(() => {
      chat.flush = undefined;
      const added = chat.pending.splice(0);
      chat.set((entries) => [...entries, ...added]);
    });
  }

  private forgetChat(id: string) {
    const chat = this.chats.get(id);
    if (chat?.flush) clearTimeout(chat.flush);
    this.chats.delete(id);
  }

  private deliver(follow: Follow, bytes: Uint8Array) {
    if (follow.sink) {
      follow.sink.write(bytes);
      return;
    }
    follow.pending.push(bytes);
    follow.pendingBytes += bytes.length;
    while (follow.pendingBytes > MAX_PENDING && follow.pending.length > 1) {
      follow.pendingBytes -= follow.pending.shift()!.length;
    }
  }

  private update(id: string, change: (t: TerminalInfo) => TerminalInfo) {
    this.setList((list) => list.map((t) => (t.id === id ? change(t) : t)));
  }
}

function newFollow(end: number): Follow {
  return { end, pending: [], pendingBytes: 0, subscribed: false };
}

export function decodeBase64(data: string): Uint8Array {
  const text = atob(data);
  const bytes = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) bytes[i] = text.charCodeAt(i);
  return bytes;
}
