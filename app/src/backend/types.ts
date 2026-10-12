// Wire types shared with the daemon. The Rust definitions in
// crates/proto/src/lib.rs are the source of truth.

export const EVENT_RUN_STARTED = "sandhill.run_started";
export const EVENT_RUN_FINISHED = "sandhill.run_finished";
export const EVENT_SESSION_ENDED = "sandhill.session_ended";
export const BLOB_KEY = "sandhill_blob";
export const CONFLICT_ERROR = "conflict:";

export interface DaemonEvent {
  seq: number;
  session: string;
  run?: string;
  msg_type: string;
  ts_ms: number;
  content: any;
}

export interface PingReply {
  protocol_version: number;
  daemon_version: string;
  home: string;
  hostname: string;
}

export interface KernelspecInfo {
  name: string;
  display_name: string;
  language: string;
  /** The environment it runs in: a conda env, a venv or a pyenv version. */
  env?: string;
}

export type SessionStatus = "starting" | "running" | "ended";

export interface SessionInfo {
  id: string;
  kernel: string;
  language: string;
  cwd: string;
  pid?: number;
  status: SessionStatus;
  created_ms: number;
  ended_ms?: number;
  last_seq: number;
  /** The project folder the session was started for, if any. */
  project?: string;
  /** Runs sent to the kernel that have not finished: the one running and any queued. */
  pending_runs?: number;
  /** The language's version as the kernel reports it, like "4.4.1". */
  language_version?: string;
}

/** What the daemon's machine is doing. */
export interface SystemStats {
  cpu: { percent: number; cores: number; model: string; load: number };
  memory: { total: number; used: number };
  gpus: { name: string; percent: number; memory_used: number; memory_total: number }[];
  disk?: { path: string; total: number; used: number; available: number };
  /** Resident memory of each running kernel, with the processes it started. */
  sessions: { session: string; rss: number }[];
}

/** A folder opened as a project on the daemon's machine. */
export interface ProjectInfo {
  path: string;
  name: string;
  opened_ms: number;
  /** False when the folder has been moved or deleted. */
  exists: boolean;
  /** What the app saved with save_project_state; null until then. */
  state: ProjectState | null;
}

/** A project window's personal state, which the daemon keeps for it. */
export interface ProjectState {
  /** Open documents, in tab order. */
  tabs?: string[];
  active?: string;
  /** Per document, the cells whose outputs are shown; the rest start hidden. */
  outputs?: Record<string, string[]>;
}

export interface CellRef {
  file: string;
  cell_id: string;
}

export interface RunStarted {
  run: string;
  msg_id: string;
}

export interface FileText {
  path: string;
  text: string;
  mtime_ms: number;
}

export interface FileWritten {
  path: string;
  mtime_ms: number;
}

export interface DirEntry {
  name: string;
  kind: "file" | "dir" | "other";
  size: number;
  mtime_ms: number;
}

export interface CellOutputs {
  cell_id: string;
  run: string;
  session: string;
  language: string;
  code: string;
  started_ms: number;
  finished_ms?: number;
  status?: string;
  execution_count?: number;
  outputs: Output[];
}

export interface FileOutputs {
  file: string;
  cells: CellOutputs[];
  seq: number;
}

/** An nbformat 4 output object. */
export interface Output {
  output_type: "stream" | "display_data" | "execute_result" | "error" | string;
  [key: string]: any;
}

export interface BlobRef {
  [BLOB_KEY]: string;
  size: number;
}

export function isBlobRef(value: unknown): value is BlobRef {
  return typeof value === "object" && value !== null && BLOB_KEY in value;
}

/** Contents of `.<name>.out/index.json` next to a .qmd/.Rmd file. */
export interface SidecarIndex {
  sandhill_outputs: number;
  document: string;
  cells: Record<string, SavedCell>;
}

export interface SavedCell {
  code: string;
  language: string;
  kernel: string;
  run: string;
  started_ms: number;
  finished_ms?: number;
  status?: string;
  execution_count?: number;
  outputs: Output[];
}

export interface Blob {
  hash: string;
  /** The original JSON string value (base64 for binary MIME types). */
  data: string;
}

/** `kind` of a plain shell terminal; agents use their own kind, like "claude". */
export const TERMINAL_SHELL = "shell";

export interface FileStat {
  path: string;
  /** Missing when the file doesn't exist. */
  mtime_ms?: number;
  size: number;
}

/** An agent CLI the daemon can start in a terminal. */
export interface AgentInfo {
  kind: string;
  name: string;
  /** Where the CLI is; missing when it isn't installed. */
  path?: string;
  version?: string;
  /** A shell command that installs it. */
  install: string;
}

/** How to start an agent. Missing fields leave the CLI's own defaults. */
export interface AgentOptions {
  model?: string;
  effort?: string;
  /** "supervised", "edits", "auto" or "full". */
  permission?: string;
  documents?: string[];
}

/** What an agent reports through its hooks and status line. */
export interface AgentReport {
  state?: "busy" | "waiting" | "idle";
  message?: string;
  /** Claude Code's permission mode, from its hooks: `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`... */
  mode?: string;
  status?: AgentStatus;
  ts_ms?: number;
}

/** Claude Code's status line, as the daemon keeps it. */
export interface AgentStatus {
  model?: string;
  model_id?: string;
  /** Percent of the context window in use. */
  context?: number;
  context_size?: number;
  effort?: string;
  rate_limits?: Record<string, { used_percentage?: number; resets_at?: string | number }>;
  cost_usd?: number;
  version?: string;
}

export interface TerminalExit {
  code?: number;
  signal?: number;
  ended_ms: number;
}

export interface TerminalInfo {
  id: string;
  kind: string;
  title: string;
  cwd: string;
  project?: string;
  created_ms: number;
  running: boolean;
  exit?: TerminalExit;
  /** Bytes of output so far. */
  end: number;
  options?: AgentOptions;
  agent?: AgentReport;
}

export interface TerminalSubscribed {
  terminal: string;
  /** Where the replayed output starts. */
  from: number;
}

/** News from a subscribed terminal: output, an agent report, or its end. */
export interface TerminalEvent {
  terminal: string;
  offset?: number;
  /** Output bytes, base64. */
  data?: string;
  agent?: AgentReport;
  exit?: TerminalExit;
}
