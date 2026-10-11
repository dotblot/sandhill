// App-wide state: the daemon link, kernels, sessions and their event
// subscriptions. Documents ask it for a session per language and run code
// through it.

import { createSignal, type Setter } from "solid-js";
import type { Backend, LinkState } from "../backend";
import {
  type CellRef,
  type DaemonEvent,
  EVENT_SESSION_ENDED,
  type FileOutputs,
  type KernelspecInfo,
  type PingReply,
  type RunStarted,
  type SessionInfo,
} from "../backend/types";
import { RunStore } from "./runs";

export class Workspace {
  readonly runs = new RunStore();
  readonly link: () => LinkState;
  readonly host: () => PingReply | undefined;
  readonly kernelspecs: () => KernelspecInfo[];
  readonly sessions: () => SessionInfo[];
  /** Bumped whenever a run starts or finishes, for busy indicators. */
  readonly activity: () => number;

  private setLink: Setter<LinkState>;
  private setHost: Setter<PingReply | undefined>;
  private setKernelspecs: Setter<KernelspecInfo[]>;
  private setSessions: Setter<SessionInfo[]>;
  private setActivity: Setter<number>;
  /** Session id -> highest event seq received, for resubscribing after a reconnect. */
  private subscribed = new Map<string, number>();

  constructor(readonly backend: Backend) {
    [this.link, this.setLink] = createSignal<LinkState>({ status: "connecting" });
    [this.host, this.setHost] = createSignal<PingReply>();
    [this.kernelspecs, this.setKernelspecs] = createSignal<KernelspecInfo[]>([]);
    [this.sessions, this.setSessions] = createSignal<SessionInfo[]>([]);
    [this.activity, this.setActivity] = createSignal(0);

    backend.onEvent((event) => this.onEvent(event));
    backend.onLink((state) => {
      this.setLink(state);
      if (state.status === "connected") void this.onConnected();
    });
    this.runs.watchAll((run) => {
      if (run.phase !== "running") this.setActivity((n) => n + 1);
    });
  }

  call<T>(method: string, params?: Record<string, unknown>): Promise<T> {
    return this.backend.call<T>(method, params);
  }

  async refreshSessions() {
    this.setSessions(await this.call<SessionInfo[]>("list_sessions"));
  }

  liveSessions(language?: string): SessionInfo[] {
    return this.sessions().filter(
      (s) => s.status !== "ended" && (!language || s.language.toLowerCase() === language),
    );
  }

  isBusy(session: string): boolean {
    this.activity();
    return this.runs.pending(session).length > 0;
  }

  async startSession(kernel: string, cwd?: string): Promise<SessionInfo> {
    const info = await this.call<SessionInfo>("start_session", { kernel, cwd });
    await this.subscribe(info.id, info.last_seq);
    await this.refreshSessions();
    return info;
  }

  async shutdown(session: string) {
    await this.call("shutdown", { session });
    await this.refreshSessions();
  }

  interrupt(session: string) {
    return this.call("interrupt", { session });
  }

  async execute(session: string, code: string, cell?: CellRef): Promise<string> {
    if (!this.subscribed.has(session)) {
      const info = this.sessions().find((s) => s.id === session);
      await this.subscribe(session, info?.last_seq ?? 0);
    }
    const started = await this.call<RunStarted>("execute", { session, code, cell });
    return started.run;
  }

  /** Saved and in-progress outputs for a file, merged into the run store. */
  async fileOutputs(file: string): Promise<FileOutputs> {
    const snapshot = await this.call<FileOutputs>("file_outputs", { file });
    for (const cell of snapshot.cells) this.runs.applySnapshot(cell, snapshot.file, snapshot.seq);
    const live = new Set(this.liveSessions().map((s) => s.id));
    for (const session of new Set(snapshot.cells.map((c) => c.session))) {
      if (live.has(session)) await this.subscribe(session, snapshot.seq);
    }
    return snapshot;
  }

  /**
   * Streams a session's events after `since`. Once subscribed, every later
   * event arrives, so a second call is a no-op; the run store joins
   * snapshots with what it already received.
   */
  async subscribe(session: string, since: number) {
    if (this.subscribed.has(session)) return;
    this.subscribed.set(session, since);
    try {
      await this.call("subscribe", { session, since_seq: since });
    } catch (err) {
      this.subscribed.delete(session);
      throw err;
    }
  }

  private onEvent(event: DaemonEvent) {
    const seen = this.subscribed.get(event.session);
    if (seen === undefined || event.seq > seen) this.subscribed.set(event.session, event.seq);
    this.runs.apply(event);
    if (event.msg_type === EVENT_SESSION_ENDED) {
      this.runs.endSession(event.session);
      void this.refreshSessions();
    }
  }

  private async onConnected() {
    try {
      const [host, specs] = await Promise.all([
        this.call<PingReply>("ping"),
        this.call<KernelspecInfo[]>("list_kernelspecs"),
      ]);
      this.setHost(host);
      this.setKernelspecs(specs);
      await this.refreshSessions();
      // A new connection has no subscriptions; pick up where we left off.
      const previous = [...this.subscribed];
      this.subscribed.clear();
      const live = new Set(this.liveSessions().map((s) => s.id));
      for (const [session, seq] of previous) {
        if (live.has(session)) await this.subscribe(session, seq);
        else this.runs.endSession(session);
      }
    } catch (err) {
      console.error("setting up the daemon connection failed", err);
    }
  }
}

/** Picks a kernel for a language: the one named, else Ark for R, python3 for Python. */
export function pickKernel(
  specs: KernelspecInfo[],
  language: string,
  preferred?: string,
): KernelspecInfo | undefined {
  const lang = normalizeLanguage(language);
  const matching = specs.filter((s) => normalizeLanguage(s.language) === lang);
  const byName = (name?: string) => (name ? matching.find((s) => s.name === name) : undefined);
  return (
    byName(preferred) ??
    (lang === "r" ? (byName("ark") ?? byName("ir")) : undefined) ??
    (lang === "python" ? byName("python3") : undefined) ??
    matching[0]
  );
}

export function normalizeLanguage(language: string): string {
  const l = language.toLowerCase();
  return l === "python3" ? "python" : l;
}
