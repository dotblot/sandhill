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
