// A window's state: the daemon link, kernels, sessions and their event
// subscriptions. Documents ask it for a session per language and run code
// through it. In a project window, sessions it starts are tagged with the
// project, and documents in the same folder share them, as scripts share
// RStudio's console.

import { createSignal, type Setter } from "solid-js";
import type { Backend, LinkState } from "../backend";
import {
  type CellRef,
  type DaemonEvent,
  EVENT_SESSION_ENDED,
  EVENT_VIEW,
  type FileOutputs,
  type KernelspecInfo,
  type PingReply,
  type ProjectSettings,
  type ProjectSetup,
  type RunStarted,
  type SessionInfo,
} from "../backend/types";
import { RunStore } from "./runs";

export class Workspace {
  readonly runs = new RunStore();
  readonly link: () => LinkState;
  readonly host: () => PingReply | undefined;
  /** When the link last came up (Date.now()), while it is up. */
  readonly connectedSince: () => number | undefined;
  readonly kernelspecs: () => KernelspecInfo[];
  readonly sessions: () => SessionInfo[];
  /** Bumped whenever a run starts or finishes, for busy indicators. */
  readonly activity: () => number;
  /** Restarted sessions, old id -> new id, so every document using one follows it. */
  readonly replacements: () => Record<string, string>;
  /** The project's settings and what its folder points to, once read. */
  readonly setup: () => ProjectSetup | undefined;

  private setLink: Setter<LinkState>;
  private setHost: Setter<PingReply | undefined>;
  private setConnectedSince: Setter<number | undefined>;
  private setKernelspecs: Setter<KernelspecInfo[]>;
  private setSessions: Setter<SessionInfo[]>;
  private setActivity: Setter<number>;
  private setReplacements: Setter<Record<string, string>>;
  private setSetup: Setter<ProjectSetup | undefined>;
  /** Session id -> highest event seq received, for resubscribing after a reconnect. */
  private subscribed = new Map<string, number>();
  /** The session each language last ran code in, from this window. */
  private lastUsed = new Map<string, string>();
  /** Sessions being started for documents to share, by kernel and folder. */
  private sharedStarts = new Map<string, Promise<SessionInfo>>();
  private viewers = new Set<(session: string, title: string) => void>();

  constructor(
    readonly backend: Backend,
    /** The project folder this window works on, if any. */
    readonly project?: string,
  ) {
    [this.link, this.setLink] = createSignal<LinkState>({ status: "connecting" });
    [this.host, this.setHost] = createSignal<PingReply>();
    [this.connectedSince, this.setConnectedSince] = createSignal<number>();
    [this.kernelspecs, this.setKernelspecs] = createSignal<KernelspecInfo[]>([]);
    [this.sessions, this.setSessions] = createSignal<SessionInfo[]>([]);
    [this.activity, this.setActivity] = createSignal(0);
    [this.replacements, this.setReplacements] = createSignal<Record<string, string>>({});
    [this.setup, this.setSetup] = createSignal<ProjectSetup>();

    backend.onEvent((event) => this.onEvent(event));
    backend.onLink((state) => {
      const up = state.status === "connected";
      if (up !== (this.link().status === "connected")) this.setConnectedSince(up ? Date.now() : undefined);
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

  /** Asks the daemon for its kernels again, say after the user installed one. */
  async refreshKernelspecs(): Promise<KernelspecInfo[]> {
    const specs = await this.listKernelspecs();
    this.setKernelspecs(specs);
    return specs;
  }

  /** The kernels, in a project window with those of the environments in its folder (a .venv). */
  private listKernelspecs(): Promise<KernelspecInfo[]> {
    const all = () => this.call<KernelspecInfo[]>("list_kernelspecs");
    // A daemon from before project settings doesn't know the call.
    return this.project ? this.call<KernelspecInfo[]>("list_project_kernelspecs", { path: this.project }).catch(all) : all();
  }

  /** Reads the project's settings again, and what its folder points to. */
  async refreshSetup(): Promise<ProjectSetup | undefined> {
    if (!this.project) return undefined;
    const setup = await this.call<ProjectSetup>("get_project_settings", { path: this.project });
    this.setSetup(setup);
    return setup;
  }

  /** Writes the project's settings to .sandhill/project.toml. */
  async saveSettings(settings: ProjectSettings): Promise<ProjectSetup> {
    const setup = await this.call<ProjectSetup>("save_project_settings", { path: this.project, settings });
    this.setSetup(setup);
    return setup;
  }

  /** The kernel the project runs `language` in, saved or found in its folder, if there is one. */
  projectKernel(language: string): string | undefined {
    const lang = normalizeLanguage(language);
    const setup = this.setup();
    return lang === "r" ? setup?.r : lang === "python" ? setup?.python : undefined;
  }

  /**
   * The kernel to start for `language`: the project's, else the one the
   * document asks for, else the usual one. None when the project's kernel
   * isn't among `specs`.
   */
  pick(language: string, wanted?: KernelWanted, specs = this.kernelspecs()): KernelspecInfo | undefined {
    const name = this.projectKernel(language);
    if (name) return specs.find((k) => k.name === name);
    return pickKernel(specs, language, wanted);
  }

  /** Where a notebook or Quarto document in `dir` runs its code. */
  documentFolder(dir: string): string {
    return this.project && this.setup()?.settings.documents === "project" ? this.project : dir;
  }

  liveSessions(language?: string): SessionInfo[] {
    return this.sessions().filter(
      (s) => s.status !== "ended" && (!language || s.language.toLowerCase() === language),
    );
  }

  /** Live sessions to offer in this window: its project's, and ones that belong to no project. */
  projectSessions(language?: string): SessionInfo[] {
    return this.liveSessions(language).filter((s) => !s.project || s.project === this.project);
  }

  /**
   * Whether a session is running code. For sessions this window follows,
   * from their events; for others, from the daemon's count when the session
   * list was last fetched.
   */
  isBusy(session: string): boolean {
    this.activity();
    // Quiet runs (a variable listing) don't make it busy.
    if (this.runs.pending(session).some((r) => !r.quiet)) return true;
    if (this.subscribed.has(session)) return false;
    return (this.sessions().find((s) => s.id === session)?.pending_runs ?? 0) > 0;
  }

  /**
   * A running session of this project that a document in `cwd` can join:
   * the one this window last ran `language` in, else the newest. With
   * `kernel`, only sessions of that kernel.
   */
  sessionToJoin(language: string, cwd: string, kernel?: string): SessionInfo | undefined {
    const lang = normalizeLanguage(language);
    const candidates = this.liveSessions().filter(
      (s) =>
        s.project === this.project &&
        normalizeLanguage(s.language) === lang &&
        s.cwd === cwd &&
        (!kernel || s.kernel === kernel),
    );
    const last = this.lastUsed.get(lang);
    return (
      candidates.find((s) => s.id === last) ??
      candidates.reduce<SessionInfo | undefined>((a, b) => (a && a.created_ms >= b.created_ms ? a : b), undefined)
    );
  }

  /** Starts a session for documents to share; documents asking at the same time get the same one. */
  startShared(kernel: string, cwd: string): Promise<SessionInfo> {
    const key = `${kernel}\n${cwd}`;
    let pending = this.sharedStarts.get(key);
    if (!pending) {
      pending = this.startSession(kernel, cwd).finally(() => this.sharedStarts.delete(key));
      this.sharedStarts.set(key, pending);
    }
    return pending;
  }

  /** Records that `old` was restarted as `fresh`. */
  replaced(old: string, fresh: string) {
    this.setReplacements((r) => ({ ...r, [old]: fresh }));
  }

  /** Follows restarts from a session id to the session that replaced it. */
  current(session: string): string {
    const replacements = this.replacements();
    const seen = new Set<string>();
    while (replacements[session] && !seen.has(session)) {
      seen.add(session);
      session = replacements[session];
    }
    return session;
  }

  async startSession(kernel: string, cwd?: string): Promise<SessionInfo> {
    const info = await this.call<SessionInfo>("start_session", { kernel, cwd, project: this.project });
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
    const language = this.sessions().find((s) => s.id === session)?.language;
    if (language) this.lastUsed.set(normalizeLanguage(language), session);
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

  /** Calls `listener` when code in a session this window follows asks to view a table. */
  onView(listener: (session: string, title: string) => void): () => void {
    this.viewers.add(listener);
    return () => this.viewers.delete(listener);
  }

  private onEvent(event: DaemonEvent) {
    const seen = this.subscribed.get(event.session);
    if (seen === undefined || event.seq > seen) this.subscribed.set(event.session, event.seq);
    if (event.msg_type === EVENT_VIEW) {
      for (const listener of this.viewers) listener(event.session, String(event.content?.title ?? ""));
      return;
    }
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
        this.listKernelspecs(),
        this.refreshSetup().catch((err) => console.warn("could not read the project's settings", err)),
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

/** The kernel a document asks for, as a notebook's kernelspec metadata records it. */
export interface KernelWanted {
  name?: string;
  display_name?: string;
}

/**
 * Picks a kernel for a language: the environment the document names, else
 * the kernel it names, else Ark for R and python3 for Python.
 */
export function pickKernel(
  specs: KernelspecInfo[],
  language: string,
  wanted: KernelWanted = {},
): KernelspecInfo | undefined {
  const lang = normalizeLanguage(language);
  const matching = specs.filter((s) => normalizeLanguage(s.language) === lang);
  const byName = (name?: string) => (name ? matching.find((s) => s.name === name) : undefined);
  const byEnv = wantedEnvs(wanted)
    .map((env) => matching.find((s) => s.env === env))
    .find((s) => s);
  return (
    byEnv ??
    byName(wanted.name) ??
    (lang === "r" ? (byName("ark") ?? byName("ir")) : undefined) ??
    (lang === "python" ? byName("python3") : undefined) ??
    matching[0]
  );
}

/**
 * Environments a document may be naming. VS Code records the environment as
 * the display name (with the kernel name "python3"); nb_conda_kernels names
 * kernels `conda-env-<env>-py` and shows "Python [conda env:<env>]".
 */
export function wantedEnvs(wanted: KernelWanted): string[] {
  const envs: string[] = [];
  const conda = wanted.name && /^conda-env-(.+)-[^-]+$/.exec(wanted.name);
  if (conda) envs.push(conda[1]);
  if (wanted.name && /^conda-(root|base)-/.test(wanted.name)) envs.push("base");
  const shown = wanted.display_name && /\[conda env:(.+?)\]/.exec(wanted.display_name);
  if (shown) envs.push(shown[1]);
  if (wanted.display_name) envs.push(wanted.display_name);
  return envs;
}

export function normalizeLanguage(language: string): string {
  const l = language.toLowerCase();
  return l === "python3" ? "python" : l;
}
