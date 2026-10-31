// Wire types shared with the daemon. The Rust definitions in
// crates/proto/src/lib.rs are the source of truth.

import type { ChatEntry } from "../model/chat";

export const EVENT_RUN_STARTED = "sandhill.run_started";
export const EVENT_RUN_FINISHED = "sandhill.run_finished";
export const EVENT_SESSION_ENDED = "sandhill.session_ended";
/** `View(df)` or `%view df` ran: `{title}` is what to open in the Data Explorer. */
export const EVENT_VIEW = "sandhill.view";
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
  /** The user the daemon runs as. */
  user?: string;
  /** When the daemon started, in ms since the epoch; 0 from older daemons. */
  started_ms?: number;
  /** "app" (attach --start), "launchd", "systemd", or empty when started by hand. Missing from older daemons. */
  started_by?: string;
}

/** The end of the daemon's log. */
export interface LogTail {
  path: string;
  text: string;
  size: number;
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
  /** What each running job uses; missing from older daemons. */
  jobs?: JobUsage[];
}

export interface JobUsage {
  terminal: string;
  /** Share of one core since the last `system_stats`, as `top` counts (200 is two cores); none at first. */
  cpu?: number;
  rss: number;
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
  /** Tables and diffs open beside the documents, in tab order, and the id of the one in front. */
  extras?: SavedExtra[];
  activeExtra?: string;
  /** Project settings were offered once (saved or not), so they aren't again. */
  setupAsked?: boolean;
}

/** Where a project's notebooks and Quarto documents run: their own folder, or the project's. */
export type DocumentsRunIn = "document" | "project";

/** A project's shared settings, kept in `.sandhill/project.toml`. */
export interface ProjectSettings {
  /** The kernel R runs in, by name; without one, what the project's files point to. */
  r?: string;
  python?: string;
  documents: DocumentsRunIn;
  /** Files of NAME=value lines, relative to the project, whose variables its kernels start with. */
  env_files: string[];
  /** Where agent conversations are kept: in the project's folder (.sandhill/agents/), or on the machine. */
  chat_history?: ChatHistory;
}

export type ChatHistory = "project" | "machine";

/** An environment a project's files point to, such as the conda env its environment.yml names. */
export interface FoundEnvironment {
  language: string;
  /** What says so: renv.lock, environment.yml, .venv. */
  file: string;
  /** "renv", "conda env omicverse", ".venv". */
  label: string;
  /** The installed kernel that runs in it. */
  kernel?: string;
  /** What to do to get a kernel for it, when there is none. */
  hint?: string;
}

/** A project's settings and what its folder suggests. */
export interface ProjectSetup {
  settings: ProjectSettings;
  /** Whether .sandhill/project.toml exists. */
  saved: boolean;
  /** Why project.toml couldn't be read. */
  problem?: string;
  /** Best first for each language. */
  found: FoundEnvironment[];
  /** The kernel R runs in: the saved one, else the first found that is installed. */
  r?: string;
  python?: string;
}

/** A Data Explorer or diff tab, as kept for next time. */
export type SavedExtra =
  | { kind: "table"; session: string; language: string; path: string[] }
  | { kind: "diff"; repo: string; file: string; staged: boolean; sha?: string };

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

export interface Deleted {
  path: string;
  /** Where the Trash keeps it, when it was moved there. */
  trashed_to?: string;
}

export interface FileBytes {
  path: string;
  offset: number;
  /** The bytes, in base64. */
  data: string;
  /** The whole file's size. */
  size: number;
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

/** One of a session's runs, for its Console (`session_runs`). */
export interface SessionRun {
  run: string;
  /** The document cell it came from; none for code typed in the Console or an agent's run_code. */
  cell?: CellRef;
  /** The agent that ran it, like "Claude". */
  by?: string;
  code: string;
  started_ms: number;
  finished_ms?: number;
  status?: string;
  execution_count?: number;
  outputs: Output[];
}

export interface SessionRuns {
  session: string;
  runs: SessionRun[];
  seq: number;
}

/** An image a run showed, for the Plots pane (`session_plots`). */
export interface Plot {
  run: string;
  /** Where it is in the journal. */
  seq: number;
  ts_ms: number;
  /** The document cell the run came from; none for the Console or an agent's run_code. */
  cell?: CellRef;
  /** The agent that ran it, like "Claude". */
  by?: string;
  code: string;
  /** The output's MIME bundle: images as blob references, small SVGs inline. */
  data: Record<string, unknown>;
  metadata?: Record<string, any> | null;
}

export interface SessionPlots {
  session: string;
  plots: Plot[];
  seq: number;
}

/** Where the daemon serves a page (`serve_page`, `serve_html`): `http://127.0.0.1:{port}{path}` on its machine. */
export interface PageUrl {
  port: number;
  path: string;
}

/** A document's preview (`preview_document`): `quarto preview` in a terminal of its own. */
export interface PreviewInfo {
  /** Where its page is on the daemon's machine: `http://127.0.0.1:{port}/…`. */
  url: string;
  /** Closing it ends the preview. */
  terminal: TerminalInfo;
}

/** Whether a plot's session can draw it again (`redraw_plot`), and why not. */
export interface PlotRedraw {
  can: boolean;
  reason?: string | null;
  /** With `show`: the PNG drawn, in base64. */
  png?: string | null;
}

/** A session's variable, or a part of one (`list_variables`). */
export interface Variable {
  name: string;
  kind: "data" | "value" | "function" | string;
  class: string;
  size?: number;
  /** Whether it has parts to list: columns, items, slots. */
  children: boolean;
  /** Whether it opens in the Data Explorer: a data frame or a matrix. */
  table?: boolean;
  summary: string;
}

export interface Variables {
  session: string;
  path: string[];
  variables: Variable[];
  /** How many more parts there are than listed. */
  more: number;
}

/** A sort key for `table_rows`. */
export interface TableSort {
  /** The column's position, from 0. */
  column: number;
  descending?: boolean;
}

/** A filter for `table_rows`: rows pass when `column` meets it. */
export interface TableFilter {
  column: number;
  /** <, <=, >, >=, =, != (one value), between (two), in and not in (any), contains, missing, present. */
  op: string;
  values?: string[];
}

export interface TableColumn {
  name: string;
  kind: "number" | "text" | "category" | "logical" | "date" | "other" | string;
  /** The language's own type: dbl, fct, float64, category. */
  type: string;
}

/** Rows of a table (`table_rows`). */
export interface TableRows {
  session: string;
  path: string[];
  /** The table as code, like `seu@meta.data` or `adata.obs`. */
  code: string;
  /** Whether it's a matrix (or, in Python, an array) rather than a data frame. */
  matrix?: boolean;
  /** All its columns; empty when only cells were asked for (`cells_only`). */
  columns: TableColumn[];
  /** The column each row's cells start at: they are the columns asked for, from this one on. */
  column_offset?: number;
  /** How many columns it has. */
  width: number;
  /** How many rows it has. */
  total: number;
  /** How many pass the filters. */
  rows: number;
  offset: number;
  /** Each returned row's name or index label. */
  labels: string[];
  /** Cells by row: numbers in number columns, text, or null where missing. */
  values: (number | string | null)[][];
}

export interface ColumnSummary {
  column: number;
  missing: number;
  distinct?: number;
  min?: number;
  max?: number;
  mean?: number;
  median?: number;
  /** Counts in equal bins from min to max. */
  histogram?: number[];
  /** The commonest values, commonest first (a factor's in level order). */
  top?: { value: string; count: number }[];
}

export interface TableSummary {
  session: string;
  path: string[];
  rows: number;
  columns: ColumnSummary[];
}

export interface HelpTopic {
  /** The function or topic: `read.csv`, `pandas.read_csv`. */
  topic: string;
  /** R's package, or Python's top-level module. */
  package: string;
  title: string;
}

export interface HelpTopics {
  session: string;
  topics: HelpTopic[];
  /** How many more matched than are listed. */
  more: number;
  /** R's vignettes: a package's, or those whose name or title matched. */
  vignettes?: HelpVignette[];
}

/** An R package's vignette, as it was built when the package was installed. */
export interface HelpVignette {
  /** Its name, as `vignette()` takes it. */
  name: string;
  package: string;
  title: string;
  /** The built file, HTML or PDF, on the session's machine. */
  file: string;
  /** Its R code, if the package has it. */
  code?: string;
}

export interface HelpMember {
  name: string;
  kind: string;
  title: string;
}

export interface HelpPage {
  session: string;
  topic: string;
  package: string;
  title: string;
  version: string;
  /** R: the page's HTML body. */
  html?: string;
  /** Python: module, class, function, method, property or value. */
  kind?: string;
  /** Python: `(adata, target_sum=None)`. */
  signature?: string;
  /** Python: the docstring. */
  text?: string;
  members?: HelpMember[];
  /** R: other packages with a page on the same topic. */
  alternatives?: HelpTopic[];
}

export interface HelpSource {
  session: string;
  topic: string;
  package: string;
  text: string;
  file?: string;
  line?: number;
}

export interface Package {
  name: string;
  version: string;
  title: string;
  library: string;
  /** R: attached with library(). Python: imported by the user's code. */
  attached: boolean;
  loaded: boolean;
  /** R: part of R itself. */
  base: boolean;
  /** Python: the modules it provides. */
  modules?: string[];
  /** Python: what installed it, like pip or conda. */
  installer?: string;
}

export interface Packages {
  session: string;
  packages: Package[];
  libraries: string[];
  cran: boolean;
  bioconductor: boolean;
  conda: boolean;
}

/** R packages with a newer version on CRAN or Bioconductor (`package_updates`). */
export interface PackageUpdates {
  session: string;
  updates: PackageUpdate[];
  repos: string[];
  /** Repositories that couldn't be reached. */
  unreached?: string[];
}

export interface PackageUpdate {
  name: string;
  installed: string;
  available: string;
  library: string;
  bioconductor: boolean;
}

/** A repository's state, from `git_status`; `repo` is empty outside one. */
export interface GitStatus {
  repo: string;
  /** The branch checked out; absent when HEAD is detached. */
  branch?: string;
  /** HEAD's commit; absent before the first commit. */
  head?: string;
  upstream?: string;
  ahead: number;
  behind: number;
  files: GitFile[];
  more: boolean;
  stashes: number;
  /** "merging", "rebasing", "cherry-picking" or "reverting". */
  state?: string;
  /** While merging, git's message for the merge's commit. */
  merge_message?: string;
  /** Changes whenever a branch, tag or HEAD moves. */
  refs: string;
  branches: string[];
  remote_branches: string[];
  /** Tags, the newest first. */
  tags?: string[];
  remotes: string[];
  user_name: string;
  user_email: string;
}

export interface GitLines {
  added: number;
  removed: number;
  binary?: boolean;
}

export interface GitFile {
  /** Relative to the repository; a folder ends with "/". */
  path: string;
  from?: string;
  /** M, A, D, R, C or T; absent when nothing is staged. */
  staged?: string;
  /** M, D, T, ? (untracked) or U (conflict). */
  unstaged?: string;
  conflict?: boolean;
  staged_lines?: GitLines;
  lines?: GitLines;
  size?: number;
}

export interface GitRef {
  kind: "head" | "branch" | "remote" | "tag";
  name: string;
}

export interface GitCommit {
  sha: string;
  parents: string[];
  author: string;
  email: string;
  time_ms: number;
  subject: string;
  refs?: GitRef[];
  coauthors?: string[];
}

export interface GitLog {
  commits: GitCommit[];
  more: boolean;
}

export interface GitChange {
  path: string;
  from?: string;
  status: string;
  lines?: GitLines;
}

/** The branch's own commits, oldest first, that can be squashed and reordered (`git_own_commits`). */
export interface GitOwnCommits {
  commits: { sha: string; time_ms: number; message: string }[];
  /** The commit they're on top of; none from the first commit. */
  base?: string;
  /** Why older commits aren't among them, or why there are none. */
  note?: string;
}

/** A commit's place in a rewrite: folded into the one before it, or with a new message. */
export interface GitRewrite {
  sha: string;
  squash?: boolean;
  message?: string;
}

export interface GitCommitInfo {
  sha: string;
  parents: string[];
  author: string;
  email: string;
  time_ms: number;
  message: string;
  files: GitChange[];
}

export interface GitDiff {
  file: string;
  text: string;
  truncated: boolean;
  binary: boolean;
  /** Whether its lines can be staged, unstaged or thrown away one by one. */
  lines?: boolean;
}

export type GitAction =
  | { op: "stage" | "unstage"; files: string[] }
  | { op: "discard"; files: string[] }
  /** `lines` are places among the lines of `diff`, the text git_diff gave. */
  | { op: "stage_lines" | "unstage_lines" | "discard_lines"; file: string; diff: string; lines: number[] }
  | { op: "commit"; message: string; amend: boolean }
  | { op: "fetch" | "push" | "undo_commit" | "stash_pop" | "abort" | "continue" | "init" }
  /** With `rebase`, the branch's own commits go on top of what's pulled. */
  | { op: "pull"; rebase?: boolean }
  | { op: "switch" | "merge" | "rebase"; branch: string }
  /** An annotated tag on HEAD, or on `sha`. */
  | { op: "create_tag"; name: string; sha?: string }
  | { op: "delete_tag" | "push_tag"; name: string }
  /** Settles a conflicted file with one side's version of all of it. */
  | { op: "resolve"; file: string; side: "ours" | "theirs" }
  | { op: "create_branch"; name: string }
  | { op: "stash"; message?: string }
  | { op: "set_user"; name: string; email: string }
  /** The branch's own commits made again in this order, oldest first, on top of `base`. */
  | { op: "rewrite"; base?: string; commits: GitRewrite[] };

export interface GitMessage {
  summary: string;
  body: string;
  agent: string;
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
/** `kind` of the terminal a document's preview runs in. */
export const TERMINAL_PREVIEW = "preview";
/** `kind` of the terminal a job runs in (`start_job`). */
export const TERMINAL_JOB = "job";

/** What a job runs: a script, with the program for its kind, or a command line. */
export interface JobSpec {
  path?: string;
  /** A command line; with `path`, the arguments after the script. */
  command?: string;
}

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
  /** "console": the agent's code runs in the Console instead of going into the linked file. */
  code?: string;
  /** The file the conversation is pinned to; unset, it follows the editor. */
  pin?: string;
  /** false: web search is off. */
  web?: boolean;
  /** false: where the user is in the editor stays out of their messages. */
  context?: boolean;
  /** What the user added to the first message with @. */
  refs?: MessageRef[];
}

/** Something added to a message with @: a file, a chunk, a variable, a plot or a file from the Mac. */
export interface MessageRef {
  kind: "file" | "chunk" | "variable" | "plot" | "attach";
  /** How the chip shows it: "analysis.qmd › markers". */
  name: string;
  /** A file as the agent should use it: relative to the project when inside it. */
  path?: string;
  /** A chunk's label or a notebook cell's id. */
  cell?: string;
  session?: string;
  language?: string;
}

/** What an agent reports through its hooks and status line, or (Codex) what Sandhill's bridge to it reports. */
export interface AgentReport {
  state?: "busy" | "waiting" | "idle";
  message?: string;
  /** Claude Code's permission mode, from its hooks: `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`... */
  mode?: string;
  /** Claude Code's session, from its hooks: what `restart_terminal` resumes. */
  session_id?: string;
  /** Codex's thread: what `restart_terminal` resumes. */
  thread?: string;
  /** Codex: the mode it is in, as `AgentOptions.permission` names it. */
  permission?: string;
  /** Sandhill's own mode on top of the agent's, when one is on: `restricted` or `ask`. */
  guard?: string | null;
  /** Where the agent's code goes: "console", or null for the linked file. */
  code?: string | null;
  /** The message box's choices (see `AgentOptions`): null when unpinned. */
  pin?: string | null;
  web?: boolean;
  context?: boolean;
  /** What the next message takes, until it does. */
  refs?: MessageRef[] | null;
  /** The file the last message works in: the pinned one, else the one in front then. */
  document?: string;
  status?: AgentStatus;
  /** Codex: the models it offers. */
  models?: AgentModel[];
  /** Codex: the account it is signed in with, null when it isn't. */
  account?: { type?: string; email?: string; plan?: string } | null;
  /** Codex: it needs signing in to OpenAI to work (not when it uses another provider). */
  auth_required?: boolean;
  /** Codex: a sign-in in progress: where to go, the code to enter there, or what went wrong. */
  login?: { url?: string; code?: string; error?: string } | null;
  ts_ms?: number;
}

/** A model an agent offers, with its thinking efforts. */
export interface AgentModel {
  id: string;
  label: string;
  efforts?: string[];
  /** Its default effort. */
  effort?: string;
  default?: boolean;
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
  /** When its program last started; an exit from before is the old program's. */
  started_ms?: number;
  running: boolean;
  exit?: TerminalExit;
  /** Bytes of output so far. */
  end: number;
  options?: AgentOptions;
  agent?: AgentReport;
  /** An agent started before the daemon was updated: it has the old tools. */
  outdated?: boolean;
  /** The name the user gave it. */
  name?: string;
  /** For a job: what it runs. */
  job?: JobSpec;
}

export interface TerminalSubscribed {
  terminal: string;
  /** Where the replayed output starts. */
  from: number;
}

/** Where the user is in a project window, as `set_focus` tells the daemon. */
export interface Focus {
  file?: string;
  cell?: string;
  line?: number;
  selection?: string;
  /** Every document open in the window; agents' edits to these come to the window. */
  open: string[];
}

/** Something the daemon asks the window working on `project` to do. */
export interface AppRequest {
  id: number;
  project: string;
  method: "add_cell" | "edit_cell" | "git_credential" | string;
  params: {
    file: string;
    code: string;
    language?: string;
    after?: string;
    cell_id?: string;
    run?: boolean;
    /** git_credential: what git or ssh asks, and whether to hide what's typed. */
    prompt?: string;
    secret?: boolean;
  };
}

/** News from a subscribed terminal: output, an agent report, or its end. */
export interface TerminalEvent {
  terminal: string;
  offset?: number;
  /** Output bytes, base64. */
  data?: string;
  agent?: AgentReport;
  exit?: TerminalExit;
  /** An entry of the agent's conversation (see `model/chat.ts`). */
  chat?: ChatEntry;
}
