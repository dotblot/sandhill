# sandhilld protocol

Clients talk to `sandhilld` over a byte stream carrying **one JSON object per
line**: a Unix socket locally (`~/.local/state/sandhill/sandhilld.sock`), or
the stdin/stdout of `ssh host sandhilld attach --start` for a remote server.
Types live in `crates/proto`; this page is the overview.

## Requests and responses

```json
{"id": 1, "method": "start_session", "params": {"kernel": "python3", "cwd": "/data/project"}}
{"type": "response", "id": 1, "result": {"id": "8f90b94e…", "status": "running", …}}
{"type": "response", "id": 2, "error": "no running session abc"}
```

Responses can arrive out of order; match them by `id`.

| Method | Params | Result |
|---|---|---|
| `ping` | | `{protocol_version, daemon_version, home, hostname, build, user, started_ms, started_by}` |
| `list_kernelspecs` | | `[{name, display_name, language, env?}]` |
| `start_session` | `kernel`, `cwd?`, `project?` | `SessionInfo` once the kernel is ready |
| `list_sessions` | | `[SessionInfo]`, including ended sessions |
| `execute` | `session`, `code`, `cell?: {file, cell_id}`, `quiet?` | `{run, msg_id}` |
| `interrupt` | `session` | `null` |
| `shutdown` | `session` | `null` |
| `subscribe` | `session`, `since_seq?` | `{session, from_seq}`, then events |
| `unsubscribe` | `session` | `null` |
| `history` | `session`, `since_seq`, `limit?` | `{events, next_seq}` |
| `get_blob` | `hash`, `near?` | `{hash, data}` |
| `read_file` | `path` | `{path, text, mtime_ms}` |
| `read_notebook` | `path` | `{path, text, mtime_ms}`, plots and large values as blob references |
| `write_notebook` | `path`, `text`, `expected_mtime_ms?` | `{path, mtime_ms}`; blob references in outputs are written out as their values |
| `write_file` | `path`, `text`, `expected_mtime_ms?` | `{path, mtime_ms}` |
| `list_dir` | `path` | `[{name, kind, size, mtime_ms}]`, directories first |
| `file_outputs` | `file` | `{file, cells, seq}` |
| `open_project` | `path` | `ProjectInfo` with its saved `state` |
| `list_recent_projects` | | `[ProjectInfo]`, most recent first, `state` null |
| `save_project_state` | `path`, `state` | `null` |
| `forget_project` | `path` | `null` |
| `system_stats` | `path?` | `SystemStats` for the daemon's machine |
| `read_log` | `bytes?` | `{path, text, size}`: whole lines from the end of the daemon's log, 64 KiB unless `bytes` says |
| `stop_daemon` | | `null`, then the daemon stops; kernels and terminals keep running for the next one |
| `stat_files` | `paths` | `[{path, mtime_ms?, size}]`; no `mtime_ms` when a file is missing |
| `list_agents` | | `[{kind, name, path?, version?, install}]` |
| `start_terminal` | `kind`, `cwd?`, `project?`, `cols?`, `rows?`, `options?`, `prompt?` | `TerminalInfo` |
| `list_terminals` | | `[TerminalInfo]`, oldest first |
| `subscribe_terminal` | `terminal`, `from?`, `chat_from?` | `{terminal, from}`, then terminal messages |
| `unsubscribe_terminal` | `terminal` | `null` |
| `terminal_input` | `terminal`, `data` | `null` |
| `answer_terminal` | `terminal`, `id`, `answer` | `null` |
| `tell_terminal` | `terminal`, `command` | `null` |
| `resize_terminal` | `terminal`, `cols`, `rows` | `null` |
| `close_terminal` | `terminal` | `null` once its program has ended and its files are gone |
| `set_focus` | `project`, `focus` | `null` |
| `get_focus` | `project` | `Focus` or `null` |
| `add_cell` | `file`, `code`, `language?`, `after?`, `run?`, `project?` | `CellEdited` |
| `edit_cell` | `file`, `cell_id`, `code`, `run?`, `project?` | `CellEdited` |
| `answer` | `id`, `result?`, `error?` | `null` |

`SessionInfo` is `{id, kernel, language, language_version?, cwd, pid, status, created_ms, ended_ms?, last_seq, project?}`
with `status` one of `starting`, `running`, `ended`. `language_version` is
what the kernel reports in its `kernel_info_reply` ("4.4.1", "3.12.4").

`SystemStats` is `{cpu: {percent, cores, model, load}, memory: {total, used},
gpus: [{name, percent, memory_used, memory_total}], disk?: {path, total, used,
available}, sessions: [{session, rss}]}`, sizes in bytes. CPU `percent` is the
whole machine's use since the previous call (or over a quarter second for the
first), `load` the one-minute load average. Memory `used` leaves out caches the
system can drop. `gpus` come from `nvidia-smi` when there is one. `disk` is the
file system holding `path` (default: the home folder). `sessions` gives the
memory each running kernel holds, with the processes it started.

`ProjectInfo` is `{path, name, opened_ms, exists, state}`. A project is a
folder on the daemon's machine, named by its absolute path without a trailing
slash (the daemon normalizes what it is given). `open_project` records it as
opened now and returns the `state` the app last saved for it with
`save_project_state` (any JSON; the app keeps its open tabs and shown outputs
there), or `null`. The daemon keeps that state in its own journal, never in
the folder, so opening a colleague's repository leaves no trace in it. `forget_project`
removes a project from the recent list and touches no files. A session
started with `project` lists it in its `SessionInfo`, also after a daemon
restart.

`build` identifies the daemon's executable (a short sha256). `sandhilld attach
--start` stops a running daemon whose `build` differs from its own (an older
app left it running) and starts its own build; kernels keep running and the
new daemon reattaches to them.

`started_by` says what started the daemon: `app` (`attach --start`),
`launchd` or `systemd`, or empty when it was run by hand. The app's
connection pop-up shows it, with `user` and how long ago `started_ms` was.

A `quiet` execute stores no history and doesn't stop the queue on an error,
for code the user didn't write (the agents' variable listing).

`list_kernelspecs` lists the kernels in Jupyter's standard folders and also
those inside conda environments, pyenv versions and the environment of the
first Python on `PATH`, which need no `ipykernel install`. Kernels found in a
conda environment are named `conda-env-<env>-py` (or `-r`), as
nb_conda_kernels names them, and run with the environment activated. `env`
names the environment a kernel runs in.

## Events

```json
{"type": "event", "event": {"seq": 42, "session": "8f90…", "run": "f3d7…",
  "msg_type": "stream", "ts_ms": 1791681116453, "content": {"name": "stdout", "text": "hello\n"}}}
```

- `seq` increases across the whole daemon. To resume after a disconnect,
  subscribe with `since_seq` set to the last `seq` you saw.
- Without `since_seq`, `subscribe` sends only new events.
- Kernel outputs keep their Jupyter `msg_type` and `content`: `stream`,
  `display_data`, `update_display_data`, `execute_input`, `execute_result`,
  `error`, `clear_output`.
- Daemon events use a `sandhill.` prefix:
  - `sandhill.run_started` `{code, cell}`: always the first event of a run.
  - `sandhill.run_finished` `{status, execution_count?}`: `status` is the
    kernel's `ok`, `error` or `aborted`, or `unknown` if the run ended while
    no daemon was connected to hear the reply.
  - `sandhill.session_ended` `{reason}`.

## Terminals and agents

The daemon keeps terminals: a shell (`kind` `shell`, the user's login shell)
or a coding agent (`claude` for Claude Code, `codex` for Codex), each running
unmodified in its own pseudo-terminal on the daemon's machine. `list_agents`
says which agent CLIs are installed, and how to install the others.

`TerminalInfo` is `{id, kind, title, cwd, project?, created_ms, running,
exit?: {code?, signal?, ended_ms}, end, options?, agent?, outdated?}`. `end`
is the number of output bytes so far. `options` is what the agent was started
with: `{model?, effort?, permission?, documents?}`, where `permission` is
`supervised`, `edits`, `auto`, `full` or `plan`. `outdated` is true for an agent that
another build of sandhilld started, such as before an update: it keeps that
build's MCP server, hooks and instructions until it is started again, so the
app suggests a new session. Without `cwd`, a terminal starts in the `project`
folder, else the home folder. An agent's `prompt` is its first message, which
it answers as it starts.

After `subscribe_terminal`, output arrives as

```json
{"type": "terminal", "terminal": {"terminal": "3f2a9c1e04b7", "offset": 1024, "data": "<base64>"}}
```

`offset` is where `data` starts in the terminal's output, so a client that
reconnects subscribes with `from` set to the end of what it has. Without
`from`, the replay starts at the recent output (the last MiB, from a line
start). The reply's `from` is where the replay really starts; it is later
than asked when that output is no longer kept (the last 4 MiB are). Other
terminal messages carry `agent` (what an agent reported, below) or `exit`.

Agents report through hooks that run `sandhilld terminal-event`: `agent` is
`{state: "busy" | "waiting" | "idle", message?, mode?, status?, ts_ms}`.
`waiting` means it asks the user something (a permission prompt, a question).
`mode` is Claude Code's permission mode as its hooks last reported it
(`default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`...), so a mode
changed in the terminal shows.
`status` comes from Claude Code's status line: `{model, model_id, context,
context_size, effort?, rate_limits?, cost_usd?, version}`, with `context` the
percent of the context window in use and `rate_limits` the plan's 5-hour and
weekly use.

Claude Code's hooks also record its conversation, so the app can show it as
a chat while the agent runs unmodified in its terminal. With `chat_from`,
`subscribe_terminal` also sends the conversation from entry `chat_from` on (0
for all of it), then each new entry, as terminal messages with `chat`:
`{seq, event}`. Each `event` has a `kind` and `at_ms`:

- `prompt {turn, text, place?}`: the user's message, with where they were.
- `text {turn, message, index, text, final}`: part of a reply.
- `step {turn, id, tool, input}`, then `step_done {turn, id, output, ms}` or
  `step_failed {turn, id, error, interrupted}`.
- `ask {turn, id, tool, input, remember?, scopes?, step?}`: the agent waits
  for permission, or for answers to `AskUserQuestion`, until an
  `answered {id, answer}` entry. `remember` is what allowing it for the chat
  or the project would also allow from then on (`Bash(npm test:*)`,
  `files in /data`); `scopes` says which of `chat` and `project` it can be
  allowed for, when not both. `step` is the id of the step it is about, when
  the agent says (Codex); otherwise it is the latest step of that tool with
  the same input.
- `stop {turn}`, and `session {source, model?}` when a session starts,
  resumes, is cleared or compacted.
- `error {turn?, text}`: something went wrong, such as a turn Codex couldn't
  start or finish; without `turn` it is about the conversation.

`answer_terminal` answers an `ask`: `answer` is `{behavior: "allow" |
"deny", scope?: "once" | "chat" | "project", answers?: {question: answer}}`.

Codex runs differently: `start_terminal` with kind `codex` runs Sandhill's
bridge, `sandhilld codex-chat`, in the terminal, and the bridge runs
`codex app-server` and writes the conversation and the agent's reports
itself, from what Codex sends. The terminal shows a transcript. Codex's
steps take the shapes above where there is one (`Bash`, `TodoWrite`,
`mcp__sandhill__add_cell`); its own are `apply_patch` (`{changes: [{path,
kind: "add" | "delete" | "update", diff, move_path?}]}`, `diff` being the
whole file when added or deleted) and `reasoning` (`{text}`, its summary of
what it is thinking). `permissions` asks for more access
(`{reason, permissions}`). What the user does in the chat goes to the bridge
with `tell_terminal`; `command` is one of `{kind: "say", text}`,
`{kind: "interrupt"}`, `{kind: "set", model?, effort?, permission?}` (from
the next turn on; `model: null` is Codex's default), `{kind: "compact"}` and
`{kind: "login", method: "browser" | "code"}`. Commands sent while the bridge
isn't connected wait for it. Its `agent` reports also have `permission` (the
mode it is in, named as in `options`), `models` (`[{id, label, efforts?,
effort?, default?}]`), `account` (`{type, email?, plan?}`, or null when it
isn't signed in), `auth_required` and `login` (`{url, code?}` while signing
in, or `{error}`).

Each agent gets Sandhill's MCP server, `sandhilld mcp --project <folder>`,
which it starts itself. Its tools reach the user's live sessions through this
same protocol: `list_sessions`, `list_variables`, `run_code` and
`get_outputs`; and the user's documents (below): `get_focus`, `add_cell` and
`edit_cell`. Claude Code's prompt hook (`terminal-event prompt`) adds where
the user is to each prompt.

## Windows and agents' edits

A project window tells the daemon where the user is with `set_focus`:
`Focus` is `{file?, cell?, line?, selection?, open, ts_ms}`, the document in
front, its current cell (the chunk with the cursor, or the notebook cell last
in focus), the cursor's line, the selected text, and every document open in
the window. The daemon keeps the latest place per project, also after the
window has gone (`ts_ms` says when); `get_focus` returns it.

`add_cell` puts code into a document as a new cell after cell `after`, or
after the last cell (the request to a window then has no `after`): a chunk in a `.qmd`/`.Rmd`, a code cell in a notebook,
or code at the end of an `.R`/`.py` script. `edit_cell` replaces a cell's
code. The result is `CellEdited`: `{file, cell_id, language, in_window,
kernel?}`.

- When a window has the file open (it is in that connection's `open`), the
  daemon asks the window, which edits the document as the user would (one ⌘Z
  undoes it), saves it, answers, and with `run` then runs the cell as its Run
  button does. `in_window` is true. The run shows up in `file_outputs` under
  `cell_id` (for a script, the console entry it runs as).
- Otherwise the daemon edits the file itself and leaves running the cell to
  the caller. `kernel` is the kernel the document asks for: a notebook's
  kernelspec, or a Quarto document's `jupyter:`.
- Either way, the project's place moves to the new cell, so the next one
  goes below it.

The daemon asks a window with

```json
{"type": "request", "request": {"id": 7, "project": "/data/pbmc", "method": "add_cell",
  "params": {"file": "/data/pbmc/analysis.qmd", "code": "dim(seu)", "after": "qc", "run": true}}}
```

sent only to clients that called `set_focus`. The window working on
`project` answers with `answer`: `{id, result: {cell_id, language}}` or
`{id, error}`.

## Blobs

Binary MIME values (images other than SVG, PDF) and any value over 64 KiB are
stored once, by SHA-256, and replaced in the event with
`{"sandhill_blob": "<hash>", "size": <bytes>}`. Fetch them with `get_blob`.
The data is the original JSON string (base64 for images).

## Files and notebook outputs

Paths are absolute on the daemon's machine (a leading `~` is expanded), so
the app edits files the same way locally and on a server.

- `write_file` replaces the file atomically. Pass the `mtime_ms` you read to
  refuse the write, with an error starting `conflict:`, if the file changed
  on disk in the meantime.
- To tie a run to a notebook cell, pass `cell: {file, cell_id}` to `execute`.
  For `.ipynb` the cell id is the notebook's own cell `id`. For `.qmd` and
  `.Rmd` it is the chunk label, or `chunk-<n>` (1-based position) for an
  unlabeled chunk.
- `file_outputs` returns the latest run of each cell of a file, including
  runs still going: `{cell_id, run, session, language, code, started_ms,
  finished_ms?, status?, execution_count?, outputs}`, with `outputs` as
  nbformat 4 output objects. Subscribe to each session with `since_seq` set
  to the returned `seq` to continue from exactly that point.
- When a run from a `.qmd` or `.Rmd` cell finishes, the daemon saves its
  outputs next to the document, whether or not a client is connected:

  ```
  analysis.qmd
  .analysis.qmd.out/
    index.json      {"sandhill_outputs": 1, "document", "cells": {cell_id: {code, language,
                     kernel, run, started_ms, finished_ms, status, execution_count, outputs}}}
    blobs/<sha256>  values the outputs refer to as {"sandhill_blob": hash, "size": n}
  ```

  `get_blob` with `near` set to the document also looks in its `blobs/`.
- `.ipynb` outputs live in the notebook itself, which only the app writes.
  On opening a notebook, the app takes outputs from `file_outputs` for
  cells whose latest run is newer than the file.
- `read_notebook` reads a `.ipynb` the way the app wants it: output values
  that are binary or over 64 KB are moved to the blob store and replaced by
  blob references, so plots load by URL rather than as base64 text.
- `write_notebook` takes the notebook back with its blob references (and
  those of new outputs from the journal) and writes each referenced value
  into the file, so a save doesn't bring the plots through the app either.
  Send the text as nbformat lays it out (keys sorted, one-space indent);
  values put back are laid out the same way, text split into lines. It
  checks `expected_mtime_ms` like `write_file`, and fails without touching
  the file if a referenced value is missing from the store. With a daemon
  that predates these two calls, the app reads with `read_file` and puts
  the values back itself (with `get_blob`) before `write_file`.

## Guarantees

- Outputs are journaled whether or not a client is connected.
- Kernels run in their own process group and survive a daemon restart; the
  next daemon reconnects with the same ZeroMQ identity, so replies to code
  sent before the restart still arrive.
- Output a kernel prints in the short window while no daemon is running is
  lost. Long jobs should write logs to files as well.
- Terminals run under a small `sandhilld terminal-host` process each, in
  their own process group, so they also survive a daemon restart, and their
  output is kept on disk while no client is connected.
