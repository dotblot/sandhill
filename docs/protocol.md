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
| `ping` | | `{protocol_version, daemon_version, home, hostname, build, user, started_ms, started_by, lifecycle?}`; lifecycle distinguishes `ready`, `maintenance`, `stopping`, omitted by older daemons |
| `list_kernelspecs` | | `[{name, display_name, language, env?}]` |
| `start_session` | `kernel`, `cwd?`, `project?` | `SessionInfo` once the kernel is ready |
| `list_sessions` | | `[SessionInfo]`, including ended sessions |
| `execute` | `session`, `code`, `cell?: {file, cell_id}`, `quiet?`, `by?` | `{run, msg_id}` |
| `complete` | `session`, `code`, `cursor_pos` | `{matches, cursor_start, cursor_end, metadata}`; Jupyter completion, no execution |
| `inspect` | `session`, `code`, `cursor_pos` | `{found, data}`; Jupyter `inspect_request` (detail level 0), no execution; `data` is the kernel's MIME bundle |
| `lsp_start` | `project`, `language`, `document_kind?`, `session?` | `{lsp, capabilities}`; capabilities is the full LSP InitializeResult |
| `lsp_send` | `lsp`, `message` | `null` after forwarding/handling the JSON-RPC object |
| `lsp_stop` | `lsp` | `null`; detaches only this connection |
| `session_runs` | `session`, `limit?` | `{session, runs, seq}`: the session's latest runs, for its Console |
| `session_plots` | `session`, `limit?` | `{session, plots, seq}`: the images the session's runs showed, for the Plots pane |
| `session_pages` | `session`, `limit?` | `{session, plots, seq}`: the web pages the session's Console and scripts showed, for the Viewer |
| `serve_page` | `path` | `{port, path}`: where the daemon serves a file and its folder, including HTML and binary PDF/image previews |
| `serve_html` | `html?`, `blob?` | `{port, path}`: where the daemon serves an output's HTML |
| `preview_document` | `path`, `project?` | `{url, terminal}` once the `quarto preview` of a `.qmd` or `.Rmd` answers |
| `open_tunnel` | `tunnel`, `port` | `null` once connected to `127.0.0.1:port` on the daemon's machine |
| `tunnel_write` | `tunnel`, `data` | not answered: bytes (base64) for the tunnel |
| `close_tunnel` | `tunnel` | not answered |
| `redraw_plot` | `session`, `run`, `image?`, `index?`, `file?`, `format?`, `width?`, `height?`, `dpi?`, `show?` | `{can, reason?, png?}`: whether the session can draw the plot again, or draws it into `file`, or with `show` answers a PNG |
| `list_variables` | `session`, `path?` | `Variables` |
| `find_variables` | `session`, `query`, `root?` | `{session, matches: [{path, name, class, summary, accessor, children, table}], complete}` |
| `delete_variables` | `session`, `names` | `null` once they are gone and memory is collected |
| `table_rows` | `session`, `path`, `offset?`, `limit?`, `sort?`, `filters?`, `search?`, `columns?`, `column_offset?`, `column_limit?`, `cells_only?` | `TableRows`: a page of a data frame or matrix, for the Data Explorer |
| `table_summary` | `session`, `path`, `filters?`, `search?`, `column_offset?`, `column_limit?` | `TableSummary`: each column's counts, range and commonest values |
| `table_export` | `session`, `path`, `sort?`, `filters?`, `search?`, `file` | `{path, mtime_ms}` once the CSV is written |
| `help_topics` | `session`, `query?`, `package?`, `limit?` | `HelpTopics`: the help topics that match, for the Help pane |
| `help_page` | `session`, `topic`, `package?` | `HelpPage`: R's page as HTML, or Python's signature, docstring and members |
| `help_source` | `session`, `topic`, `package?` | `{session, topic, package, text, file?, line?}`: a function's source |
| `list_packages` | `session` | `Packages`: what's installed for the session's language |
| `package_updates` | `session` | `PackageUpdates`: packages with newer versions (R: CRAN, Bioconductor; Python: PyPI, conda's channels) |
| `suggest_cell` | `cwd`, `language`, `cells`, `output?`, `agent?` | `{agent, code}`: a ghost cell, the code an agent suggests for the next cell (`code` null when it sees none) |
| `git_status` | `path` | `GitStatus`: the branch, the changed files and the refs of the repository `path` is in |
| `git_log` | `path`, `skip?`, `limit?` | `{commits, more}`: a page of every branch's commits, newest first |
| `git_show` | `path`, `sha` | `GitCommitInfo`: a commit's whole message and the files it changed |
| `git_own_commits` | `path` | `{commits, base?, note?}`: the branch's own commits that `rewrite` can squash and reorder |
| `git_diff` | `path`, `file`, `staged?`, `sha?` | `{file, text, truncated, binary}`: a file's unified diff |
| `git_run` | `path`, `action` | `{output}` once git has done it: stage, commit, push and the rest |
| `git_credential` | `token`, `prompt` | What the user typed for git's prompt; `sandhilld` calls it as git's askpass program |
| `git_write_message` | `path`, `agent?` | `{summary, body, agent}`: a commit message an agent wrote for the staged changes |
| `interrupt` | `session` | `null` |
| `shutdown` | `session` | `null` |
| `subscribe` | `session`, `since_seq?` | `{session, from_seq}`, then events |
| `unsubscribe` | `session` | `null` |
| `history` | `session`, `since_seq`, `limit?` | `{events, next_seq}` |
| `get_blob` | `hash`, `near?` | `{hash, data}` |
| `read_file` | `path` | `{path, text, mtime_ms}` |
| `read_notebook` | `path` | `{path, text, mtime_ms}`, plots and large values as blob references |
| `write_notebook` | `path`, `text`, `expected_mtime_ms?` | `{path, mtime_ms}`; blob references in outputs are written out as their values |
| `write_file` | `path`, `text`, `expected_mtime_ms?`, `base64?`, `create?` | `{path, mtime_ms}` |
| `list_dir` | `path`, `sizes?` | `[{name, kind, size, mtime_ms}]`, directories first |
| `make_dir` | `path` | `{path, mtime_ms}` once the folder and any above it exist |
| `move_path` | `from`, `to` | `{path, mtime_ms}` of `to`: a rename or move |
| `copy_path` | `from`, `to` | `{path, mtime_ms}` of `to`, a folder copied with everything in it |
| `delete_path` | `path`, `trash?` | `{path, trashed_to?}` |
| `read_bytes` | `path`, `offset?`, `limit?` | `{path, offset, data, size, mtime_ms}`: up to 4 MiB of the file, `data` in base64; `limit: 0` returns metadata without content |
| `list_files` | `path`, `limit?` | `[path]`: the files under the folder, relative to it, nearest first; hidden files and package folders (`node_modules`, `renv`, …) left out; at most 5000 unless `limit` |
| `file_outputs` | `file` | `{file, cells, seq}` |
| `clear_cell_outputs` | `file`, `cell_ids` | the number of runs cleared: the cells' finished runs leave `file_outputs` until they run again, and stay in `session_runs` |
| `open_project` | `path` | `ProjectInfo` with its saved `state` and whether the folder is `trusted` |
| `list_recent_projects` | | `[ProjectInfo]`, most recent first, `state` null |
| `save_project_state` | `path`, `state` | `null` |
| `forget_project` | `path` | `null` |
| `trust_folder` | `path` | `null`: the folder and those inside it are trusted |
| `untrust_folder` | `path` | `null`; what already runs there keeps running |
| `list_trusted_folders` | | `[{path, trusted_ms, exists}]`, by path |
| `get_project_settings` | `path` | `ProjectSetup`: the project's settings and the environments its files point to |
| `save_project_settings` | `path`, `settings` | `ProjectSetup` once `.sandhill/project.toml` is written |
| `list_project_kernelspecs` | `path` | `[{name, display_name, language, env?}]`: `list_kernelspecs` and the kernels of the environments inside the project's folder |
| `system_stats` | `path?` | `SystemStats` for the daemon's machine; optional CPU/memory/GPU readings and temperatures, `gpu_status` and bounded `gpu_error` distinguish unavailable metrics from no devices |
| `read_log` | `bytes?` | `{path, text, size}`: whole lines from the end of the daemon's log, 64 KiB unless `bytes` says |
| `stop_daemon` | | `null` is flushed before shutdown; app clients require an `attach --confirm-exit` bridge and confirm EOF plus its successful exit, not the reply alone; kernels and terminals remain independent |
| `prepare_attach` | `build` | `{restart: bool}`; lifecycle-gated reuse/restart decision for `attach --start`, serialized against cleanup/uninstall |
| `stat_files` | `paths` | `[{path, mtime_ms?, size?}]`; no `mtime_ms` when a file is missing; older daemons omit `size`, so use `read_bytes` metadata when enforcing a byte limit |
| `list_agents` | | `[{kind, name, path?, version?, install}]` |
| `agent_models` | `kind` | `[{id, label, default?, effort?, efforts?}]`: the models an agent offers before a message (`codex` uses `model/list`, `antigravity` uses `agy models`, `openscience` lists its configured models, `omp` uses `omp models --json` with its default model role and thinking level, `pi` uses `pi --list-models`); a live conversation's model list replaces this one; an error for an agent that isn't installed or lists its models itself |
| `agent_accounts` | | Read-only CLI-reported account identity and timestamped quota windows; never starts a conversation or extracts authentication tokens |
| `agent_accounts_query` | `kinds: [string]`, `force?` | Same account shape, only selected providers; empty selection queries none, force bypasses pre-request cached values while concurrent queries share work |
| `list_agent_accounts` | `refresh?` | `[AgentAccount]`: device-local runnable accounts; cached unless `refresh: true` requests official CLI identity/quota queries |
| `add_agent_account` | `kind`, `name`, `api_key?` | `AgentAccount`; Claude/Codex get an isolated login folder, Antigravity requires a Gemini API key |
| `rename_agent_account` | `id`, `name` | `AgentAccount`; added accounts only, preserving the ID and login folder |
| `remove_agent_account` | `id` | `null`; added accounts only, refused while a running terminal uses the account |
| `sign_in_agent_account` | `id`, `cols?`, `rows?` | `TerminalInfo` of kind `login`, running the agent's own sign-in |
| `use_agent_reset` | `id`, `credit?` | `{outcome, account}`; Codex accounts only; spends one banked reset |
| `start_terminal` | `kind`, `cwd?`, `project?`, `cols?`, `rows?`, `options?`, `prompt?` | `TerminalInfo` |
| `restart_terminal` | `terminal`, `options?`, `cols?`, `rows?`, `prompt?` | `TerminalInfo` |
| `start_job` | `path?`, `command?`, `cwd?`, `project?` | `TerminalInfo` |
| `stop_terminal` | `terminal` | `null`; the terminal and its output stay |
| `rename_terminal` | `terminal`, `name?` | `TerminalInfo` |
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
| `add_cell` | `file`, `code`, `language?`, `after?`, `run?`, `project?`, `by?` | `CellEdited` |
| `edit_cell` | `file`, `cell_id`, `code`, `run?`, `project?`, `by?` | `CellEdited` |
| `delete_cell` | `file`, `cell_id`, `project?`, `by?` | `CellEdited` |
| `answer` | `id`, `result?`, `error?` | `null` |
| `resource_inventory` | | `{resources: [{id, kind, label, project?, state}], errors}` |
| `stop_resources` | `force` | `{stopped, remaining: [resource], errors}`; explicit force confirmation required by clients |
| `storage_inventory` | | `{entries: [{id, category, path, bytes, removable, reason?}], errors}` |
| `clean_storage` | `categories: [string]` | `{success, results, entries, errors}`; selected categories only; project directories are protected |
| `uninstall_components` | `delete_data` | `{success, results, remaining, errors, data_preserved}`; data preserved by default |
| `begin_analysis` | `project`, `spec` | `{analysis, latest_check?, history, validity, export}`; new journal-backed record at `analysis.id` |
| `check_analysis` | `project`, `analysis`, `checkpoint`, `details?` | Same envelope with a new program-generated checkpoint and actual run evidence |
| `get_analysis` | `project`, `analysis` | Authoritative historical evidence; no kernel run and no assertion of current revalidation |
| `list_skills` | | `SkillList`: installed, suggested and built-in skills |
| `preview_skills` | `fetch` | `SkillPreview`: token, source, candidates and refusals; no install |
| `install_skills` | `token`, `names` | `[SkillInfo]`: installs or replaces the selected candidates |
| `set_skill_enabled` | `name`, `enabled` | `null`; off hides the skill from agents |
| `remove_skill` | `name` | `null`; deletes its files and index record |
| `export_skill` | `name` | `SkillExport`: files as base64, executable bits and original source |
| `get_skill` | `name?`, `file?` | Enabled skill summaries, or text, path and file list |
| `list_connectors` | | `ConnectorList`: added by title and Suggested entries |
| `add_connector` | `add` | `ConnectorInfo`; connects and lists tools before saving |
| `set_connector_enabled` | `name`, `enabled` | `null`; off stops its program |
| `remove_connector` | `name` | `null`; deletes the record, secret and unshared downloaded program |
| `test_connector` | `name` | `ConnectorInfo`; reconnects, refreshing tools or recording an error |
| `export_connector` | `name` | `ConnectorAdd`, without a secret |
| `connector_tools` | `name?` | Enabled connector summaries, or one connector's full tool schemas |
| `call_connector` | `name`, `tool`, `arguments`, `read_only` | MCP tool result, with content items and `isError` |

Management calls are device-wide, not project-wide. Clients must show affected
resources/projects and obtain confirmation before destructive operations.
`stop_resources` confirms actual shutdown and reports remaining resources; it
does not stop the daemon. Only explicit force may escalate. `uninstall_components`
requires stopped resources, verifies installation provenance and reports
unowned/changed components rather than deleting them. The client stops the
daemon after a successful operation and suppresses automatic reconnect.
Uninstall leaves an `uninstalled` marker in the state directory so stale clients
and service restarts cannot recreate removed resources. Automatic `serve` and
`attach --start` refuse this state until explicitly reinstalled.

`SessionInfo` is `{id, kernel, language, language_version?, cwd, pid, status, created_ms, ended_ms?, last_seq, project?}`
with `status` one of `starting`, `running`, `ended`. `language_version` is
what the kernel reports in its `kernel_info_reply` ("4.4.1", "3.12.4").

`SystemStats` is `{cpu: {percent, cores, model, load}, memory: {total, used},
gpus: [{name, percent, memory_used, memory_total}], disk?: {path, total, used,
available}, sessions: [{session, rss}], jobs: [{terminal, cpu?, rss}]}`, sizes in bytes. CPU `percent` is the
whole machine's use since the previous call (or over a quarter second for the
first), `load` the one-minute load average. Memory `used` leaves out caches the
system can drop. `gpus` come from `nvidia-smi` when there is one. `disk` is the
file system holding `path` (default: the home folder). `sessions` gives the
memory each running kernel holds, with the processes it started. `jobs` does
the same for each running job (below), with `cpu` the share of one core it
and its processes used since the previous call, as `top` counts (200 is two
cores busy); there is none the first time a job is measured.

`ProjectInfo` is `{path, name, opened_ms, exists, state, trusted}`. A project is a
folder on the daemon's machine, named by its absolute path without a trailing
slash (the daemon normalizes what it is given). `open_project` records it as
opened now and returns the `state` the app last saved for it with
`save_project_state` (any JSON; the app keeps its open tabs and shown outputs
there), or `null`. The daemon keeps that state in its own journal, never in
the folder, so opening a colleague's repository leaves no trace in it. `forget_project`
removes a project from the recent list and touches no files. A session
started with `project` lists it in its `SessionInfo`, also after a daemon
restart.

A project opened for the first time is not `trusted`: starting a kernel or
an agent acts on files in its folder (`.Rprofile`, env files, `CLAUDE.md`,
hooks, a repository's Git configuration). Until `trust_folder`, the calls
that start something there (`start_session`, `lsp_start`, `start_terminal`,
`start_job`, `restart_terminal`, `preview_document`, `suggest_cell` and the
`git_*` calls that run Git) fail with an error starting `Restricted Mode:`.
Trust is kept in the daemon's journal, never in the folder; it covers the
folders inside, and paths outside every opened project are not held back.
Projects recorded before folder trust existed are trusted. A daemon from
before it sends no `trusted`, which clients read as true. An agent's run in
a trusted folder gets `SANDHILL_TRUSTED_FOLDER`; the Codex and pi bridges
pass it on for that run only (Codex's `projects` override, pi's
`project_trust` event), so those agents do not ask again.

A project's shared settings live in `.sandhill/project.toml`, which can be
committed. `ProjectSettings` is `{r?, python?, documents, env_files,
chat_history}`: the kernel each language runs in (none: detect it), where
notebook and Quarto chunks run (`document`, their own folder, or `project`),
the env files, relative to the project, whose `NAME=value` lines its kernels
start with (`[".env"]` unless set), and where agent conversations are kept
(`project`, the default, or `machine`; see "Conversations kept with the
project" below). `save_project_settings` makes `.sandhill/` the first time,
with a `.gitignore` for `outputs/`, and keeps the rest of an existing file,
comments included. Saving `machine` moves the project's conversations out of
its folder. `ProjectSetup` is `{settings, saved, problem?,
found, r?, python?}`: `saved` says whether the file exists, `problem` why it
couldn't be read, `found` lists the environments the folder's files point
to, best first for each language, as `{language, file, label, kernel?,
hint?}` (`renv.lock`, the environments inside the folder such as `.venv` and
pixi's, `pixi.toml` or `uv.lock` before they are installed, and the conda
environment `environment.yml` names; `hint` says how to get a kernel for one
that has none), and `r` and `python` are the kernels the project runs: the
saved one, else the first found that is installed.

`start_session` with `project` starts the kernel with the variables of the
project's env files (they don't replace what the kernelspec sets, such as
the `PATH` of its environment). An R kernel starts in the project's folder,
so its `.Rprofile` and `.Renviron` apply (this is how renv loads a project's
library), and then moves to `cwd`. Kernels of the environments inside the
project's folder are found by name, as `list_project_kernelspecs` lists them.

`build` identifies the daemon's executable (a short sha256). `sandhilld attach
--start` stops a running daemon whose `build` differs from its own (an older
app left it running) and starts its own build; kernels keep running and the
new daemon reattaches to them.

`started_by` says what started the daemon: `app` (`attach --start`),
`launchd` or `systemd`, or empty when it was run by hand. The app's
connection pop-up shows it, with `user` and how long ago `started_ms` was.

A `quiet` execute stores no history and doesn't stop the queue on an error,
for code the user didn't write (the variable listing). `by` names an agent
that ran the code ("Claude", "Codex"), for the Console to mark it.

`session_runs` returns a session's latest runs that weren't quiet, oldest
first (100 unless `limit` says, at most 1000), with their outputs: what the
app's Console shows for the session, whatever ran the code (the Console's
prompt, a script line, a notebook cell, an agent). Each run is `{run, cell?,
by?, code, started_ms, finished_ms?, status?, execution_count?, outputs}`,
outputs as in `file_outputs`. `seq` is where to subscribe from.

`session_plots` returns the images a session's runs showed, oldest first (the
latest 100 unless `limit` says, at most 500), leaving out quiet runs: every
`display_data` and `execute_result` with a PNG, JPEG, GIF or SVG, whatever ran
the code. Each is `{run, seq, ts_ms, cell?, by?, code, data, metadata}`:
`data` is the output's MIME bundle, with images as blob references (an SVG
under 64 KB stays inline), and `ts_ms` is when the kernel sent it. `seq` is
where to subscribe from.

`session_pages` returns the web pages a session showed, oldest first (the
latest 20 unless `limit` says, at most 100), in the same form: outputs of
runs from the Console, agents and scripts that are pages (see *The Viewer*
below), and apps (`{url}` pages) from anywhere. Notebooks' and Quarto
documents' other pages stay under their cells.

`redraw_plot` draws one of a run's plots again, in the session that drew it,
at another size. The plot is the image holding the blob `image`, else the
`index`-th image the run showed. Ark keeps a recording of each plot it shows
(named by the output's `transient.display_id`), and an IRkernel session keeps
the recordings of its last 10 plots and a Python session its last 10
matplotlib figures: code run when the session starts tags each plot's image
with `sandhill_plot` in the output's metadata. Plain images, plots no longer
kept and ended sessions can't be drawn again; `{can: false, reason}` says why,
as in "only plots R drew can be drawn again". Without
`file`, the call only asks. With it, the session draws the plot into `file`
(a path, or `~/...`; its folder is made) as `format` (`png`, `svg` or `pdf`),
`width` × `height` inches (0.5 to 100) at `dpi` (36 to 2400, for PNG), out of
sight, so nothing shows in the Console or the Plots pane. It fails while the
session is busy. With `show: true` it draws a PNG to show instead (`file` and
`format` are ignored) and answers it in `png`, in base64, keeping nothing: the
Plots pane asks for one at its own size.

`list_variables` runs a quiet snippet in the session and returns `Variables`:
`{session, path, variables: [{name, kind, class, size?, children, table,
summary, accessor?}], more}`. `kind` is `data`, `value` or `function`; `size` is in bytes
when the language can tell; `children` says whether the variable has parts;
`table` whether it opens in the Data Explorer (a data frame or a matrix). With an
empty `path` it lists the session's global variables; with a `path` (a
variable's name, then the names of parts within it) it lists the parts: a data
frame's columns, a list's items, an AnnData's `X`, `obs`, `var`, `obsm`,
`layers` and `uns` and the columns inside `obs` and `var`, a Seurat object's
assays and metadata. At most 200 come back; `more` counts the rest.
`delete_variables` removes variables from the session's globals and collects
memory (R: `rm()` and `gc()`; Python: `del`, IPython's output history, and
`gc.collect()`). Both refuse while the session runs other code, and give up
after 30 seconds.

`accessor` is executable code generated by the daemon's walker, not by the
client. It is optional for compatibility with older daemons. Assay5 layers
use `SeuratObject::LayerData(seu@assays$RNA, layer = "counts")`: calling the
namespaced method on the already-walked assay preserves its native layer
matrix and dimnames, without requiring Seurat to be attached or reconstructing
the parent object. Older Assay slots remain `seu@assays$RNA@counts`.
AnnData matrix columns use `adata.obsm["X_pca"][:, 0]` (path component
`[:,0]`); DataFrame-valued mappings use their column labels. `varm`, layers
and nested `uns` dictionaries follow the same walker.

`find_variables` searches names and the first 200 characters of summaries,
case-insensitively, beneath `root` (empty means visible global variables).
It runs as one silent kernel request, with no history or journaled run,
like `list_variables`. The traversal visits at most 20,000 nodes, at most
six edges below each root, and stops at a two-second deadline. R also uses
its evaluator's elapsed-time limit. `complete: false` means a depth, node,
or time bound prevented a full search; clients must label partial results.
Matches contain full paths, so clients can expand their ancestors. Each match
also carries `children` (has expandable parts) and `table` (opens in the table
viewer), with the same meaning as `Variable`; both default to false when
reading responses from older daemons.

```json
{"id": 20, "method": "find_variables", "params": {"session": "s", "query": "leiden", "root": ["adata"]}}
{"type": "response", "id": 20, "result": {"session": "s", "matches": [{"path": ["adata", "obs", "leiden"], "name": "leiden", "class": "Series", "summary": "3 values, category: 0, 1, 0", "accessor": "adata.obs['leiden']", "children": false, "table": false}], "complete": true}}
```

The `table_*` calls read a data frame or matrix by the same `path` as
`list_variables` (`["seu", "meta.data"]`, `["adata", "obs"]`), in a quiet
snippet that filters and sorts in the session, so only the rows asked for
leave it. They refuse while the session runs other code, as listing does.
Columns are named by position from 0. `sort` is `[{column, descending?}]`,
first key first, with missing values last. `filters` is `[{column, op,
values}]`, all of which a row must pass: `<`, `<=`, `>`, `>=`, `=` and `!=`
take one value, `between` two, `in` and `not in` any number, `contains` one
(ignoring case), and `missing` and `present` none. Values are text, read as
numbers for a number column; a row whose value is missing passes no filter
but `missing`.

All three calls accept optional `search` (a string, empty by default). It is
a case-insensitive **literal substring**, not a regular expression: a row
matches when any nonmissing column value, converted to its language's text,
contains the keyword. Search checks full values, not the display's truncated
or formatted text, and all original columns, including hidden/unrequested
ones. It is ANDed with `filters` before sorting, row paging, column
projection, summaries or CSV export, in the same live session query.
An empty keyword leaves the filter result unchanged; missing values do not
match text such as `NA`, `None` or `nan`. Copy as code uses the same search
semantics and applies hidden-column selection only afterward.

`table_rows` returns `{session, path, code, matrix, columns, column_indices?,
column_offset, width, total, rows, offset, labels, values}`: `code` is the table as code in
the session's language (`seu@meta.data`, `adata.obs`); `matrix` says it is a
matrix (or a NumPy array) rather than a data frame; `columns` is `[{name,
kind, type}]` for all `width` columns (empty with `cells_only`), `kind` being
`number`, `text`, `category`, `logical`, `date` or `other` and `type` the
language's own (`dbl`, `fct`, `float64`); `total` counts the table's rows and
`rows` those that pass the filters and search. Then from `offset`, `limit` rows (200
unless asked, at most 2000): each row's name or index label in `labels`, and
its cells in `values`, numbers in number columns, text otherwise, `null` where
missing. A row's cells are those of `column_limit` columns (100 unless asked,
at most 1000) from `column_offset`, so a wide table is read a block of
columns at a time, as it is a page of rows; `cells_only` leaves out the
`columns` of a table already known.

Optional `columns: [2, 0]` projects original zero-based positions in that
order, overriding `column_offset`/`column_limit`. With projection, the
response's `columns` contains only projected headers (or is empty for
`cells_only`), and `column_indices: [2, 0]` maps each cell/header back to
the original column. `width` remains the original width. Omitting `columns`
preserves existing contiguous-page behavior and omits `column_indices`.
An empty projection returns no columns; out-of-range positions and more than
1000 requested columns return clear errors. Sorting/filtering still uses
original positions and runs before projection. Old daemons may ignore
`columns`; clients detect the absent echo and hide/project locally.

```json
{"id": 21, "method": "table_rows", "params": {"session": "s", "path": ["df"], "columns": [2, 0], "limit": 1}}
{"type": "response", "id": 21, "result": {"session": "s", "path": ["df"], "code": "df", "matrix": false, "columns": [{"name": "score", "kind": "number", "type": "int64"}, {"name": "gene", "kind": "text", "type": "object"}], "column_indices": [2, 0], "column_offset": 0, "width": 3, "total": 3, "rows": 3, "offset": 0, "labels": ["0"], "values": [[42, "TP53"]]}}
```

`table_summary` returns `{session, path, rows, columns}` for the rows that
pass the filters and search, a summary per column, for `column_limit` columns (100 unless
asked, at most 1000) from `column_offset`: `{column, missing, distinct?, min?,
max?, mean?, median?, histogram, top}`, with a 20-bin `histogram` from `min` to
`max` for numbers, and for other columns of at most 1000 distinct values the
10 commonest as `top: [{value, count}]` (a factor's in level order when it has
at most 10).

`table_export` writes the rows that pass the filters and search, sorted, to a CSV `file`
(made with its folders if need be; R's `write.csv`, pandas' `to_csv`), with
the row names or index first when the table has them, and returns the file's `{path, mtime_ms}`.

`help_topics`, `help_page`, `help_source`, `list_packages` and
`package_updates` also run quiet snippets, so they fail while the session runs
other code.

`help_topics` returns `{session, topics, more}`, each topic `{topic, package,
title}`, at most `limit` of them (50 unless asked, at most 500), and how many
more matched. In R the topics are every installed package's help aliases (read
from each package's `Meta/Rd.rds` once, then kept in the session in
`options("sandhill.help")` until a library folder changes), leaving out
internal ones and S4 methods' aliases; the title is searched too. In Python
they are the public functions, classes and modules of the modules already
imported, each by its shortest public name (`pandas.read_csv`), with `package`
its top-level module. An exact name comes first, then names that start with
the query, then names (and in R titles) that contain it; packages the user
attached or imported come first among equals. `package` keeps one package, or
those its name starts with when none has that exact name; without `query` it
lists that package's topics. In R, `vignettes` lists up to 30 of the packages'
vignettes too (`package`'s, or those whose name or title has `query`), each
`{name, package, title, file, code?}`: `file` is the built HTML or PDF and
`code` its R code, on the session's machine, for the app to read with
`read_file` or `read_bytes`. Only vignettes whose built file is installed are
listed.

`help_page` returns `{session, topic, package, title, version, ...}`. For R,
`html` is the page from `tools::Rd2HTML` without its `<head>`, its links to
other topics as `../../{package}/help/{topic}` and to its examples as
`../Example/{topic}`; without `package`, the first attached, then loaded, then
installed package with the topic is used, and `alternatives` lists the others
as topics. For Python, `topic` is a dotted name, which may start with the
user's own name for a module (`sc.pp.neighbors`, shown as
`scanpy.pp.neighbors`); a module named that way is imported if need be, as
`pydoc` does. The page has `kind` (`module`, `class`, `function`, `method`,
`property` or `value`), `signature` from `inspect.signature`, `text` (the
docstring) and `members`, a module's or class's public names as `{name, kind,
title}`.

`help_source` gives a function's source: R's deparsed, Python's from its file
(`inspect.getsource`) with the `file` and the `line` it starts on.

`list_packages` returns `{session, packages, libraries, cran, bioconductor,
conda}`. Packages, by name, are `{name, version, title, library, attached,
loaded, base, modules?, installer?}`: in R, from `installed.packages()` with
`attached` meaning `library()`'d and `base` R's own; in Python, the
environment's distributions, `attached` meaning the user's code imported one
of its `modules`, and `installer` what installed it (`pip`, `conda`).
`libraries` are R's `.libPaths()` or the Python environment's prefix; `cran`
says R has a CRAN mirror set, `bioconductor` that BiocManager is installed, and
`conda` that the Python environment is a conda one. Installing and removing
aren't calls: the app runs the code in the Console, where its output shows.

`package_updates` asks R's repositories which installed packages have a newer
version (`old.packages()`): CRAN as `options("repos")` names it
(cloud.r-project.org when it's unset) and, when BiocManager is installed, the
repositories of the Bioconductor release that goes with this R
(`BiocManager::repositories()`). It returns `{session, updates, repos,
unreached}`, each update `{name, installed, available, library,
bioconductor}`, and `unreached` the repositories that didn't answer; when none
did, the call fails ("couldn't reach ...") rather than saying nothing is new. The session
downloads the repositories' indexes, so it can take a while (at most three
minutes). In Python, a conda environment's own packages (installer `conda`)
are asked of its channels with a dry run of `conda update --all` (or mamba's),
so only versions that fit the environment count; the others, unless installed
from a path or URL, are asked of PyPI's JSON API (`SANDHILL_PYPI_URL` in the
kernel's environment names another index). `library` is where the package is,
and `repos` lists "conda's channels" and the index as they were asked.

`suggest_cell` asks a coding agent for a notebook's next cell: `cells` are the
cells up to and including the one just run (the last twelve are sent), and
`output` the text it showed. `agent` (`claude` or `codex`) is asked when it
is installed, else Claude Code, else Codex. The agent runs once as its own
CLI in `cwd`, with no tools and nothing saved as a conversation (`claude -p
--model haiku --tools ""`, `codex exec --sandbox read-only`), and answers with
one fenced block, which the call returns as `code`. It can take up to two
minutes; the app asks only after a cell the user ran ends well, and only
while its "Suggest" switch is on.

The `git_*` calls run the `git` program on the daemon's machine (a command
line git, which on macOS is the Xcode one only when its tools are installed),
in the root of the repository that `path` is in. Git runs without a
terminal; when it (or ssh) needs a username, a password or token, or a key's
passphrase that no credential helper or ssh agent has, it runs `sandhilld`
as its askpass program (`GIT_ASKPASS`, and `SSH_ASKPASS` with
`SSH_ASKPASS_REQUIRE=force`), which calls `git_credential` with the run's
token and git's prompt. The daemon sends the window that called `git_run` a
`git_credential` request, `{prompt, secret}` (`secret` unless it asks for a
username or a yes/no), and gives git the text the window answers; an error
answer, or none within five minutes, makes git give up. Nothing is kept: a
credential helper the user has (the macOS keychain) stores what worked.
Reads time out after a minute, changes after five.

`git_status` returns `{repo, branch?, head?, upstream?, ahead, behind, files,
more, stashes, state?, merge_message?, refs, branches, remote_branches, tags,
remotes, user_name, user_email}`; `repo` is empty when `path` isn't in a repository. `branch` is
missing on a detached HEAD and `head` before the first commit. Files are
`{path, from?, staged?, unstaged?, conflict, staged_lines?, lines?, size?}`:
`staged` and `unstaged` are git's letters (`M`, `A`, `D`, `R`, `C`, `T`, and
`?` for a new file), `from` a renamed file's old path, `staged_lines` and
`lines` `{added, removed, binary}`, and `size` a new file's bytes. A new folder
is one entry ending in `/`. At most 2000 files are listed; `more` counts the
rest. `state` is `rebasing`, `merging`, `cherry-picking` or `reverting` while
git is stopped in one, and while merging `merge_message` is the message git
has ready for the merge's commit (`MERGE_MSG` without its comments). `refs` changes whenever a branch, tag or HEAD moves, so
a client reads the log again only then. `tags` are the newest 500 tags, the
newest first. `user_name` and `user_email` are what a commit would be signed
with.

`git_log` lists the commits of every branch, remote branch and tag, and HEAD,
in date order: `limit` of them (200 unless asked, at most 2000) after the
first `skip`. A commit is `{sha, parents, author, email, time_ms, subject,
refs, coauthors?}`, each ref `{kind, name}` with `kind` `head` (the branch
checked out, or `HEAD` when detached), `branch`, `remote` or `tag`, and
`coauthors` from `Co-authored-by:` lines. `git_show` gives one commit's
`message` and its `files` as `{path, from?, status, lines?}`.

`git_diff` with `sha` is what that commit changed in `file`; without it, the
file's staged changes when `staged`, else the ones not staged (all of a new
file). The text is git's unified diff from the first `@@`, cut at 2 MB.
`lines` says whether its lines can be staged, unstaged or thrown away a few at
a time: a changed file git already tracks, not a new, deleted, renamed or
conflicted one, with all of its diff there.

`git_run`'s `action` has an `op`: `stage`, `unstage` and `discard` take
`files` (discarding a new file deletes it); `stage_lines`, `unstage_lines`
and `discard_lines` take a `file`, the `diff` text `git_diff` gave for it, and
`lines`, places among that text's lines (from 0), and change only those lines
(`git apply` of a patch made from them, which git refuses if the file no
longer is as that diff showed it); `commit` takes `message` and
`amend`; `fetch` fetches every remote; `pull` merges unless `pull.rebase` or
`pull.ff` says otherwise, or with `rebase: true` rebases the branch's own
commits onto what it pulled (stopping on conflicts as a rebase does); `push` sets the branch's upstream the first time,
on `origin` or the only remote; `undo_commit` moves the branch back one commit
and keeps its changes staged; `switch` takes a `branch` (a remote one makes or
checks out the local branch that tracks it); `create_branch` takes a `name`
and switches to it; `merge` takes a `branch`, local or remote, and merges it
into the one checked out (`merge --no-edit`; stopping on conflicts isn't a
failure: `state` becomes `merging` and the files show as conflicts);
`rebase` takes a `branch`, local or remote, and makes the branch checked
out's own commits again on top of it (stopping on conflicts with `state`
`rebasing`); `continue` goes on with a rebase, cherry-pick or revert once its
conflicts are staged (with git's messages, no editor), or commits a merge;
`rewrite` takes the commits `git_own_commits` gave (each `{sha, time_ms,
message}`, oldest first: those on no remote branch, back to the last merge, at
most 50, on top of `base`) as `{sha, squash?, message?}` in a new order, oldest
first, with that `base`, and makes them again so: `squash` folds a commit into
the one before it, and `message` gives the commit it makes (with those folded
into it) that message. It is an interactive rebase (`--autostash`, so changes
not committed are put aside and back) whose list of commits the daemon writes;
it fails when the commits changed since they were listed, and stops on
conflicts as `rebase` does. `note` says why older commits aren't offered;
`resolve` takes a conflicted `file` and a `side`, `ours` (the branch checked
out, or while rebasing the branch's own commit) or `theirs`, and stages that
side's version of all of it (deleting the file when that side did);
`create_tag` makes an annotated tag `name` (its message the name) on HEAD or
the commit `sha`; `delete_tag` deletes the tag `name` here; `push_tag` pushes
it to the upstream's remote, else `origin` or the only one; `stash` (with an optional `message`, new files included)
and `stash_pop`; `abort` stops a merge, rebase, cherry-pick or revert; `init`
makes `path` a repository; and `set_user` sets the global `user.name` and
`user.email`. A failure's message is git's own.

`git_write_message` asks Claude Code (`claude -p` with the small model and no
tools) or else Codex (`codex exec` in a read-only sandbox), or the `agent`
named, for a message for the staged changes. It is given the recent commit
subjects and the staged diff, at most 4000 bytes of each file and 40,000 in
all, and returns the summary line, the rest as `body`, and which agent wrote
it. It runs the agent as its CLI, signed in as the user is in a terminal.

`list_kernelspecs` lists the kernels in Jupyter's standard folders and also
those inside conda environments, pyenv versions and the environment of the
first Python on `PATH`, which need no `ipykernel install`. Kernels found in a
conda environment are named `conda-env-<env>-py` (or `-r`), as
nb_conda_kernels names them, and run with the environment activated. `env`
names the environment a kernel runs in. `list_project_kernelspecs` lists
the environments inside a project's folder first: `.venv`, `venv`, `env` and
`.conda` as `project-venv-py` and so on, and pixi's as
`project-pixi-<env>-py` (or `-r`).

## Completion and language servers

Kernel completion uses Jupyter `complete_request` on the existing shell channel:

```json
{"id":80,"method":"complete","params":{"session":"kernel-id","code":"import os; os.pa","cursor_pos":16}}
{"type":"response","id":80,"result":{"matches":["os.path"],"cursor_start":11,"cursor_end":16,"metadata":{}}}
```

The reply above illustrates the shape; matches and replacement boundaries come
from the kernel. Offsets count Unicode scalar values, **not UTF-8 bytes or UTF-16
units**. This is separate from LSP positions below. Completion never creates a
run, changes execution count/history, or journals output. A busy/queued shell
returns `kernel_busy: kernel busy`; a completion that does not answer within
two seconds returns a bounded error rather than queuing more typing behind it.
Ark, IRkernel and ipykernel use the same standard request.
The currently pinned Ark implements that shell request with an empty completion
list (upstream limitation); do not invent runtime matches or execute code as a
fallback. Ark's LSP still supplies static/document completion. Runtime-sensitive
Ark suggestions require an upstream kernel implementation.

Kernel introspection uses Jupyter `inspect_request` the same way, with the
same scalar offsets, busy rule and two-second bound; the app asks it for a
call's signature where no language server answers (the Python Console):

```json
{"id":86,"method":"inspect","params":{"session":"kernel-id","code":"np.mean(a, ","cursor_pos":7}}
{"type":"response","id":86,"result":{"found":true,"data":{"text/plain":"Signature: np.mean(a, axis=None, …)\n…"}}}
```

Language servers live on the daemon's machine, reached through the same wire
over Unix sockets or SSH; no client TCP connections or extra remote ports:

```json
{"id":81,"method":"lsp_start","params":{"project":"/data/project","language":"python","document_kind":"notebook","session":"kernel-id"}}
{"type":"response","id":81,"result":{"lsp":"language-id","capabilities":{"capabilities":{"positionEncoding":"utf-16","textDocumentSync":{"openClose":true,"change":1,"save":{"includeText":true}}}}}}
{"id":82,"method":"lsp_send","params":{"lsp":"language-id","message":{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{"general":{"positionEncodings":["utf-16"]}}}}}}
{"type":"lsp_message","lsp":"language-id","message":{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"positionEncoding":"utf-16"}}}}
{"id":83,"method":"lsp_send","params":{"lsp":"language-id","message":{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///data/project/analysis.ipynb.sandhill.python.py","languageId":"python","version":1,"text":"import os\nos.pa\n"}}}}}
{"id":84,"method":"lsp_send","params":{"lsp":"language-id","message":{"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///data/project/analysis.ipynb.sandhill.python.py"},"position":{"line":1,"character":5}}}}}
{"id":85,"method":"lsp_stop","params":{"lsp":"language-id"}}
```

Capabilities in these examples are abbreviated: use the full InitializeResult
returned by `lsp_start` or the synthetic `initialize` reply. The daemon initializes
the underlying servers once with workspace root, snippet support disabled and
**UTF-16 only**. Clients may use that result directly, or send standard
`initialize`/`initialized`; initialization params do not reconfigure the shared
servers. All LSP positions/ranges are zero-based UTF-16, including non-BMP text.
Send `didOpen`, full-text `didChange` (`contentChanges: [{"text":"..."}]`), `didSave`,
and `didClose`. Formatting and client configuration changes are not enabled.
Inner request IDs are private to each connection and may overlap; replies go
only to the requesting connection. Diagnostics/notifications go to subscribers.
`lsp_send` acknowledges forwarding, **not** the eventual LSP result.

One host exists per normalized `(project, language)`; `document_kind` is a
client hint and does not create another server. `lsp_start` subscribes the
calling connection. `lsp_stop` and disconnect detach it and release its open
documents; only the last subscriber's close reaches the server. Shared document
versions are normalized by the host. After all clients detach, a 60-second idle
grace shuts down the host. A crash sends
`{"type":"lsp_message","lsp":"...","message":{"jsonrpc":"2.0","method":"sandhill/lspExited","params":{"code":"lsp_exited","message":"..."}}}`
and fails outstanding requests; the next `lsp_start` creates a new host/id.
After reconnect/restart, start again and synchronize every open document's
**full text**; no replay or journal is available for language messages.

Python merges `ty server` (completion, hover, navigation) and `ruff server`
(diagnostics) into one language ID. ty diagnostics are off. Ruff selects
`E9,F63,F7,F82`: syntax/runtime-invalid constructs, invalid comparisons/control
flow, undefined names and unbound locals; no F401 unused-import or style noise.
The editor-only low-noise configuration deliberately avoids inheriting a
project's broad lint/style policy. Server requests for configuration and
capability registration are handled by the daemon, not by windows.
Discovery is `.venv/bin`, `venv/bin`, the selected project kernel's interpreter
directory (including conda), daemon/login PATH, then
`~/.local/share/sandhill/bin/{ty,ruff}`. The kernel interpreter, or the project's
Python environment when no live kernel exists, is passed to ty's
`initializationOptions.configuration.environment.python` (the `server`
subcommand has no `--python` flag). User environments are never modified.

The payload builder pins [ty 0.0.102](https://github.com/astral-sh/ty/releases/tag/0.0.102)
and [Ruff 0.17.5](https://github.com/astral-sh/ruff/releases/tag/0.17.5), with official
release archive SHA-256 values recorded in `app/build-server-daemons.sh`.
Linux x86_64/aarch64 musl and Mac aarch64 payloads are verified before extraction.
SSH and local Mac attachment install into Sandhill's own bin directory with the
same per-component provenance/checksum ownership rules as Ark; uninstall
preserves changed/unowned programs and legacy backups.

R connects only to an existing Ark kernel in the project. Supply `session`
to select the document's kernel; otherwise the newest live matching session
is selected. Missing sessions return `ark_session_required` (UI: “Start the R
session for language features”); IRkernel returns an explicit unsupported error.
The daemon opens Ark's MIT `positron.lsp` comm with
`{"ip_address":"127.0.0.1"}`, receives
`{"msg_type":"server_started","content":{"port":12345}}`, and relays standard
Content-Length framed JSON-RPC over loopback TCP. Python uses identical framing
over stdio. Neither transport is exposed directly to the Mac.

For notebooks and mixed-language Quarto, the Mac concatenates matching-language
cells in document order, separated by two newlines, and keeps the cell/virtual-line
mapping. YAML, prose and fences are excluded; `#|` options remain code comments.
Use stable absolute **file URIs on the daemon's
machine**, e.g. `file:///data/project/report.qmd.sandhill.r.R` and
`file:///data/project/report.qmd.sandhill.python.py`, with `languageId` `r` or
`python`. These are in-memory documents, never files written by the daemon.
Avoid fragments (`#r`) and custom schemes: servers need a real project-relative
file path and a language extension for environment/import resolution. Percent
encode path bytes in URIs. Map diagnostics, completion edits and definitions
back to cells; discard stale asynchronous results. Diagnostics display waits
until save or 500 ms without typing; the server transport itself is immediate.
Versioned diagnostic notifications are rejected when their shared server version
is obsolete; the daemon translates accepted versions back to each connection's
document version, which the Mac checks before mapping ranges. Notebook editors
share one hover/diagnostic bubble owner, dismiss bubbles on scroll, and prefer
the diagnostic bubble over a symbol hover at the same squiggle.

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
  - `sandhill.run_started` `{code, cell, quiet?, by?}`: always the first
    event of a run. `quiet` and `by` are as given to `execute`.
  - `sandhill.run_finished` `{status, execution_count?}`: `status` is the
    kernel's `ok`, `error` or `aborted`, or `unknown` if the run ended while
    no daemon was connected to hear the reply.
  - `sandhill.session_ended` `{reason}`.
  - `sandhill.view` `{title}`: the run called `View(x)` (R) or `%view x`
    (Python), and `title` is what it was given, as typed (`seu@meta.data`).
    Ark's `View()` opens a `positron.dataExplorer` comm, which the daemon
    reads and closes; IRkernel and IPython sessions get a `View` and a
    `%view` when they start, which send a display_data of type
    `application/vnd.sandhill.view+json`. Neither is journaled as output.

## Terminals and agents

The daemon keeps terminals: a shell (`kind` `shell`, the user's login shell)
or a coding agent (`claude` for Claude Code, `codex` for Codex,
`antigravity` for Antigravity's `agy`, `posit` for Posit Assistant's `pa`,
`openscience` for OpenScience, `omp` for Oh My Pi, `pi` for pi), each
running unmodified under a terminal
holder on the daemon's machine.
`list_agents` says which agent CLIs are installed, and how to install the
others.

`TerminalInfo` is `{id, kind, title, cwd, project?, created_ms, started_ms,
running, exit?: {code?, signal?, ended_ms, idle?}, end, options?, agent?,
outdated?, name?, job?, output_paths?}`. `started_ms` is when its program last started. `end`
is the number of output bytes so far. `options` is what the agent was started
with: `{model?, effort?, permission?, documents?, code?, pin?, web?, refs?, account?, fork?}`,
where `permission` is `supervised`, `edits`, `auto`, `full`, `plan`,
`restricted` or `ask`, `code: "console"` sends the agent's code to the
Console (below), and the rest are the message box's choices (below).
`name` is what the user named it with `rename_terminal` (a blank `name`
takes it away). `outdated` is true for an agent that
another build of sandhilld started, such as before an update: it keeps that
build's MCP server, hooks and instructions until it is started again, so the
app suggests a new session. Without `cwd`, a terminal starts in the `project`
folder, else the home folder. An agent's `prompt` is its first message, which
it answers as it starts.
For Codex, `options.fork` is the source terminal ID for a new conversation.
The bridge reads its `agent.thread`, copies its chat entries through the new
terminal host (preserving timestamps and `revert` markers, assigning sequential
entry numbers), then calls `thread/fork`. The new conversation gets its own
thread; restarting it resumes that thread rather than forking again. A source
without a thread produces a visible chat error.


`restart_terminal` starts a terminal's program again in the same terminal,
ending it first if it still runs (an exit with `ended_ms` before the new
`started_ms` is the old program's). Its output and conversation go on where
they were. An agent resumes its conversation: Claude Code the session its
hooks last reported (`agent.session_id`), Codex its thread, OpenScience,
Oh My Pi and pi their session (`agent.thread`); of the old `agent` report,
only those stay (and the
guard, below, of the mode it starts in). `options`
replace the ones it started with, so a Claude Code conversation can go on in
a mode it can't switch to while running, such as `full`. `prompt` is a
message to go on with.

`options.account` selects an ID from `list_agent_accounts`; omitted means
the agent's default login. To switch accounts, wait for the current turn to
finish or ask the user to stop it, then call `restart_terminal` with the
replacement options and account ID. This is always a user action, never
automatic quota rotation. The terminal ID, retained output and conversation
stay the same. Claude resumes its session, Codex its thread, and Antigravity
its conversation. An account change writes a journaled chat entry of kind
`account`, displayed as "Switched to <account>", so reconnecting clients
replay the same divider.

The daemon stops a resumable agent after 30 minutes with no input, output
or agent report, only while its reported state is `idle`. Busy agents and
agents waiting for an answer are never stopped this way. Shells, jobs,
Posit Assistant (which cannot resume), and agents without a reported
`session_id` (Claude Code) or `thread` (other agents) are excluded.
The stop uses the same gentle hang-up as `stop_terminal`, keeps the
conversation, and sends the usual terminal exit to followers with
`exit.idle: true`. This flag is omitted when false, including manual stops.
The next message resumes the ended conversation through `restart_terminal`;
the new run does not inherit the idle exit.

For integration tests only, `SANDHILL_TEST_AGENT_IDLE_MS` on the daemon
process shortens the inactivity limit in milliseconds (positive integers;
invalid/zero values use 30 minutes). The daemon reads it once when serving
starts and checks every quarter of that limit, capped at once a minute.
Activity is measured in the terminal host, including while clients or the
daemon are disconnected; no extra filesystem polling is needed for it.

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

Historical output is marked `replay: true` (omitted/false for live data).
Render it without sending terminal protocol replies back to the process.
An escape sequence begun in replay remains historical through its terminating
byte, even when the rest arrives in a live chunk; later live queries still
receive their normal replies. The daemon snapshots the host's exact output
boundary before reading retained log bytes, including for old terminal hosts
that have not restarted. Older daemons do not send this marker and need an
upgrade to provide replay-safe terminal emulation.


Agents report through hooks that run `sandhilld terminal-event`: `agent` is
`{state: "busy" | "waiting" | "idle", message?, mode?, session_id?, status?, ts_ms}`.
`waiting` means it asks the user something (a permission prompt, a question).
`mode` is Claude Code's permission mode as its hooks last reported it
(`default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`...), so a mode
changed in the terminal shows.
`status` comes from Claude Code's status line: `{model?, model_id?, context?,
context_size?, effort?, rate_limits?, cost_usd?, version?, last?, total?,
duration_ms?, api_duration_ms?, lines_added?, lines_removed?}`. `context` is
the percentage of the context window in use, `context_size` is tokens, and
`rate_limits` is the plan's 5-hour and weekly use. `last` has token counts
`{input?, cache_read?, cache_write?, output?}` from the current request;
`total` has conversation token counts `{input?, output?}`. Cost is USD at API
prices, durations are milliseconds (wall time and API time respectively), and
line counts are conversation additions/removals. Missing measurements are
omitted, including `last` when Claude reports no current usage.

Claude Code's hooks also record its conversation, so the app can show it as
a chat while the agent runs unmodified in its terminal. With `chat_from`,
`subscribe_terminal` also sends the conversation from entry `chat_from` on (0
for all of it), then each new entry, as terminal messages with `chat`:
`{seq, event}`. Each `event` has a `kind` and `at_ms`:

- `prompt {turn, text, place?}`: the user's message, with where they were.
- `text {turn, message, index, text, final}`: part of a reply.
- `step {turn, id, tool, input}`, then `step_done {turn, id, output, ms}` or
  `step_failed {turn, id, error, interrupted}`.
- `ask {turn, id, tool, input, remember?, scopes?, step?, options?}`: the agent waits
  for permission, or for answers to `AskUserQuestion`, until an
  `answered {id, answer}` entry. `remember` is what allowing it for the chat
  or the project would also allow from then on (`Bash(npm test:*)`,
  `files in /data`); `scopes` says which of `chat` and `project` it can be
  allowed for, when not both. `step` is the id of the step it is about, when
  the agent says (Codex); otherwise it is the latest step of that tool with
  the same input.
  `options`, when supplied, is the agent's explicit permission choices:
  `[{optionId, name, kind}]`, with ACP kinds `allow_once`, `allow_always`,
  `reject_once` or `reject_always`. Preserve the IDs, labels and order;
  do not substitute Sandhill's usual once/chat/project buttons.
- `stop {turn}`, and `session {source, model?}` when a session starts,
  resumes, is cleared or compacted.
- `error {turn?, text}`: something went wrong, such as a turn Codex couldn't
  start or finish; without `turn` it is about the conversation.
- `revert {turn}`: Codex undid this exchange. Remove every entry with that
  turn ID, including the user's `prompt`, when applying or replaying the log.
  The append-only log retains those entries and this marker.
- `account {account, name}`: the daemon's account-switch divider, where
  `account` is the selected account ID and `name` its display name. It is
  retained across terminal restarts and replayed with the conversation.

`answer_terminal` answers an `ask`: `answer` is `{behavior: "allow" |
"deny", scope?: "once" | "chat" | "project", answers?: {question: answer}}`.
For an `ask` with explicit `options`, send `{option_id: "<optionId>"}` instead;
the selected ID must be one of the offered options.

The Restricted and Ask modes (`permission` `restricted` and `ask`) are
Sandhill's own: `agent.guard` says which is on. It is set from the mode an
agent starts in, and by `tell_terminal` with `{kind: "set", permission}`
for any agent (Codex also takes the mode itself). For Claude Code, which
runs in its default mode underneath, the hook before each step enforces it:
in Restricted mode it asks the user about every step (an `ask` with `step`
and `scopes: []`, so it is allowed once at a time), and in Ask mode it blocks
steps that change something, with a `step_failed` entry saying so. Codex
runs Restricted with its `untrusted` approval policy and Ask in a read-only
sandbox that never asks. In Ask mode, each message tells the agent it is
read-only, and Sandhill's MCP tools that change something refuse.

Codex runs differently: `start_terminal` with kind `codex` runs Sandhill's
bridge, `sandhilld codex-chat`, in the terminal, and the bridge runs
`codex app-server` and writes the conversation and the agent's reports
itself, from what Codex sends. In Chat mode the terminal shows a transcript. Codex's
steps take the shapes above where there is one (`Bash`, `TodoWrite`,
`mcp__sandhill__add_cell`); its own are `apply_patch` (`{changes: [{path,
kind: "add" | "delete" | "update", diff, move_path?}]}`, `diff` being the
whole file when added or deleted) and `reasoning` (`{text}`, its summary of
what it is thinking). `permissions` asks for more access
(`{reason, permissions}`). What the user does in the chat goes to the bridge
with `tell_terminal`; `command` is one of `{kind: "say", text}`,
`{kind: "interrupt"}`, `{kind: "set", model?, effort?, permission?}` (from
the next turn on; `model: null` is Codex's default), `{kind: "compact"}`,
`{kind: "undo"}` and `{kind: "login", method: "browser" | "code"}`.
Undo calls `thread/revert` before the most recently ended durable turn and
appends `revert` with that turn's ID on success. It refuses while a turn runs
or when there is no exchange, and reports failures as chat errors. It changes
conversation history only, not local files. Commands sent while the bridge
isn't connected wait for it. Its `agent` reports also have `thread` (Codex's
thread id), `permission` (the mode it is in, named as in `options`), `models` (`[{id, label, efforts?,
effort?, default?}]`), `account` (`{type, email?, plan?}`, or null when it
isn't signed in), `auth_required` and `login` (`{url, code?}` while signing
in, or `{error}`).
Codex's `agent.status` also carries `context` (percent), `context_size` (tokens),
`last: {input?, cache_read?, cache_write?, output?, reasoning?}` and
`total: {input?, output?, reasoning?}` (all token counts). Last-request `input`
is `max(0, inputTokens - cachedInputTokens)`; total input includes cache hits.
Reasoning is the agent's reported reasoning output, not extra output to add
to `output`. Counts remain available even without a known context window.
`instructions`, when supplied, is an array of absolute instruction-source
paths from thread start/resume/fork; it survives subsequent status updates.
Undo leaves usage to the next Codex token-usage notification rather than
estimating a replacement.


Antigravity runs the same way, with its own bridge, `sandhilld agy-chat`,
which runs `agy` in its headless mode once per message and goes on with
agy's conversation. It takes `say`, `interrupt` and `set` (permission
`supervised`, `full` or `plan`), and reports `thread` (agy's conversation
id, from the message's first `init` event on), `permission` and `models` as
Codex's bridge does. Its steps take the shapes above for commands (`Bash`),
new files (`Write`) and Sandhill's tools; agy's other tools keep agy's names
and parameters. Headless `agy` runs in a
session of its own, without the conversation's terminal as its controlling
terminal (over SSH, Linux `agy` otherwise probes that terminal and, from a
background process group, is stopped by `SIGTTOU`). A message whose `agy`
prints no event within 30 seconds is stopped and fails with an `error`
entry quoting what it printed on stderr.

Both bridges also accept `{kind: "view", mode: "terminal" | "chat"}`.
Terminal mode hands the existing controlling PTY to the unmodified native
CLI (`codex resume <thread>` or `agy --conversation <thread>`), retaining
the conversation's working directory, model, permission settings and Sandhill
MCP configuration. The bridge releases the conversation before starting the
native CLI; chat commands received during this handoff wait until Chat mode
resumes. `terminal_input` then reaches the native interface directly.
Returning to Chat requests the native CLI's exit, with signals as a bounded
fallback, waits for it to finish, then resumes the same conversation and
imports new turns from the agent's history API, never from terminal output.
A native CLI exit also returns to Chat. Neither agent drives the conversation
concurrently with its bridge. Claude Code and Posit Assistant are unchanged.
Before any message has started an agy conversation, Terminal mode first
starts one by running headless `agy` with no message, which names it in its
`init` event without a model turn. A failed agy handoff is reported once,
as `agent.view_error` in the report that returns `view_mode` to `chat`
(every other agy report has `view_error: null`); clients show it as a
dismissible notice rather than a conversation entry.

The bridges report `agent.view_mode`: `switching_terminal`, `terminal`,
`switching_chat` or `chat`. Clients display the switching states briefly,
give the terminal keyboard focus in Terminal mode and withhold the chat
composer until the bridge has resumed. `agent.thread` remains the native
conversation ID across both modes. No new wire call is required.

OpenScience is chat only. `start_terminal` with kind `openscience` starts
the generic bridge:

```text
sandhilld --socket SOCKET acp-chat --agent openscience --program PATH --project PROJECT --options JSON [--prompt TEXT] [--session ID]
```

The bridge runs `PATH acp` in the project's directory and speaks ACP v1
JSON-RPC over newline-delimited JSON on stdio. It writes the same journaled
chat events and agent reports as the other bridges. `agent.thread` is the
ACP session ID, `models` uses Sandhill's normal `{id, label, ...}` entries,
`status.model` and `status.model_id` report the selected model, and
`permission` and `state` retain their existing meanings. `view_mode` is
always `chat`; there is no chat/terminal switch for OpenScience.
`tell_terminal` accepts `say`, `interrupt` and `set` for the model and
permission choices. Interrupt sends ACP `session/cancel`.

After `initialize`, create with `session/new` or resume with `session/load`,
passing `cwd` and an empty `mcpServers`. OpenScience 2.0 offers MCP tools only
from servers in its own configuration (servers added through ACP are filtered
out), and it starts local MCP servers in a Seatbelt/bubblewrap sandbox that
denies every socket, so a stdio `sandhilld mcp` could not reach the daemon.
The bridge therefore serves Sandhill's tools itself, as MCP Streamable HTTP
on `http://127.0.0.1:PORT/mcp` (JSON replies only; `GET` answers 405; every
request needs the per-conversation bearer token, otherwise 401), with the
terminal's guards (`DIR` is the terminal's state directory). Only the child
process receives `OPENSCIENCE_CONFIG_CONTENT`:

```json
{"permission":{"python":"deny","r":"deny","mcp":{"sandhill_list_sessions":"allow","sandhill_run_code":"ask","…":"…"}},
 "instructions":["<absolute path under DIR>"],
 "mcp":{"sandhill":{"type":"remote","url":"http://127.0.0.1:PORT/mcp","headers":{"Authorization":"Bearer TOKEN"},"oauth":false,"enabled":true}},
 "model":"<selected model, when chosen>"}
```

`permission.mcp` lets read-only Sandhill tools run and makes the rest ask in
a supervised conversation; other permissions allow them, because Sandhill's
own guard refuses changes in Ask mode. OpenScience's per-project access mode
can still ask for every MCP call ("Ask always"). The instructions file
directs code execution to Sandhill's kernels. OpenScience merges these
settings with its configuration; Sandhill does not replace the user's files
or read credentials. The override is not a sandbox or a promise to
supersede administrator policy.

ACP `agent_message_chunk` becomes reply `text`; `agent_thought_chunk` and
`plan` become thought and plan step rows. `tool_call` and `tool_call_update`
become `step` and its completion/failure events, keyed by `toolCallId`.
Tool input retains ACP's `title`, `kind`, `locations`, `rawInput` and
`content`; output retains returned content and raw output. Updates may be
partial and use `pending`, `in_progress`, `completed` or `failed`.
`session/request_permission` becomes an `ask` linked to that tool step and
carrying the exact ACP `options`. Answering `{option_id: ID}` returns
`{"outcome":{"outcome":"selected","optionId":"ID"}}` to ACP; cancellation
uses ACP's cancelled outcome, not an invented permission grant.

Session responses provide `models: {currentModelId, availableModels:
[{modelId, name}]}`. Preserve the full model ID, including a returned variant
suffix. OpenScience supports the legacy/unstable `session/set_model`
extension with `{sessionId, modelId}`; newer versions also expose a model
`configOptions` selector through `session/set_config_option`. Native modes
come from `modes: {currentModeId, availableModes: [{id, name, description?}]}`;
switch only to an advertised ID with `session/set_mode` and honor
`current_mode_update` notifications. These are agent modes, not Sandhill's
MCP guard modes. Do not assume `build` or `plan` is advertised, or that
`set_mode` returns a mode update. `session/load` replays history through
`session/update` before returning, including `user_message_chunk`, replies,
thoughts, tools and plans; replay must not duplicate retained chat entries.
OpenScience keeps its native session history, and the report stores its
session ID for subsequent loads. No subscription quota is reported.

The OpenScience-specific wire details above come from its
[ACP implementation](https://github.com/synthetic-sciences/openscience/blob/8e3341a5a4d14541623305bb8e602a4fc265ff49/backend/cli/src/acp/agent.ts)
and [stdio command](https://github.com/synthetic-sciences/openscience/blob/8e3341a5a4d14541623305bb8e602a4fc265ff49/backend/cli/src/cli/cmd/acp.ts),
not a real model-prompt verification.

Oh My Pi uses the same bridge, with chat and command line:

```text
sandhilld --socket SOCKET acp-chat --agent omp --program PATH --project PROJECT --options JSON [--prompt TEXT] [--session ID]
```

The bridge runs `PATH acp` with `PI_CONFIG_FILES` naming a per-run overlay
(`DIR/omp-config.yml`, `tools.approvalMode: always-ask`), so omp asks before
steps that write or run; the user's own files are not edited. Sandhill's
loopback MCP server goes in the session request instead of a configuration:
`mcpServers: [{"type":"http","name":"sandhill","url":"http://127.0.0.1:PORT/mcp","headers":[{"name":"Authorization","value":"Bearer TOKEN"}]}]`.
omp has no system prompt over ACP, so a new session's first `session/prompt`
carries Sandhill's instructions as an extra text part. Models and thinking
effort come from the session's `configOptions` (the `model` and
`thought_level` selects); `set` with `model` or `effort` becomes
`session/set_config_option` with `{sessionId, configId, value}`, and
`config_option_update` notifications refresh them. The report adds
`status.effort`, and each model lists the current model's levels as
`efforts`. Permission requests in Full access are answered with the first
`allow_once` (else `allow_always`) option, edit tools likewise in Edit mode;
the rest become `ask` entries.

`tell_terminal` `{kind: "view", mode: "terminal"}` hands the conversation to
omp's own screen in the same terminal: the bridge reports
`switching_terminal`, cancels a running turn, ends `omp acp` (one process
owns an omp session), and runs
`omp --resume ID --append-system-prompt TEXT --extension DIR/omp-sandhill.js
--approval-mode MODE [--allow-home] [--model M] [--thinking LEVEL]` in the
foreground, reporting `terminal`. The extension offers Sandhill's tools from
the bridge's MCP server (`SANDHILL_MCP_URL`, `SANDHILL_MCP_TOKEN` in its
environment), since omp's command line reads MCP servers only from its
`mcp.json` files. `MODE` is `yolo` in Full access, `write` in Edit mode and
`always-ask` otherwise. `{kind: "view", mode: "chat"}` quits it (Ctrl-C,
then `/exit`); the bridge reports `switching_chat`, starts `omp acp` again,
`session/load`s the session (importing the turns taken on the command line)
and reports `chat`. Commands sent meanwhile apply afterwards.

pi has no ACP. `start_terminal` with kind `pi` starts its own bridge over
`pi --mode rpc`:

```text
sandhilld --socket SOCKET pi-chat --program PATH --project PROJECT --options JSON [--prompt TEXT] [--session ID]
```

It writes the same chat events and reports as the other bridges, with
`agent.thread` the pi session ID. Sandhill's tools and chat-mode approvals
come through a per-run pi extension (`pi -e`) the bridge writes into `DIR`,
so no pi configuration file is edited. `view` commands hand the session to
pi's own screen (`pi --session ID`) and back, as for Oh My Pi. Neither Oh
My Pi nor pi reports subscription quota.

Posit Assistant runs as `pa`'s own screen, with no bridge, chat entries or
reports. `start_terminal` and `restart_terminal` ignore `prompt` for it (a
prompt makes `pa` answer once and quit): the app types the first message
into the terminal.

Each agent gets Sandhill's MCP server, `sandhilld mcp --project <folder>`,
which it starts itself (Antigravity's from its settings, as `sandhilld mcp`
with the project in `SANDHILL_PROJECT` and the socket in `SANDHILL_SOCKET`;
Posit Assistant's from its administrator defaults,
`POSIT_ASSISTANT_SETTINGS_DEFAULT`). Its tools reach the user's live sessions through this
same protocol: `list_sessions`, `list_variables`, `run_code` and
`get_outputs`, `get_console`, `get_plots`, `view_table`, `get_viewer`, `get_help` and
`list_packages`; and the user's documents (below): `get_focus`, `add_cell`,
`edit_cell` and `delete_cell`. Claude Code's prompt hook (`terminal-event prompt`) adds what
the user added with @ and # to each prompt, never where the user is; the agent
asks `get_focus` for that.

`view_table` accepts `view: "current" | "original"`. When the requested
path/session matches `Focus.table`, the default is `current`: use its sort,
filters and hidden columns, and say `filtered N of M rows` in the result.
The focused session is also the default for that object. `original` ignores
those view settings. Without a matching focused table the original is read.

`get_viewer {session?}` reads the latest journaled Viewer page of that session
(an unambiguous session ID prefix is accepted), or the project's latest
page when omitted. It is read-only and allowed in Ask/Plan. Its text item
contains JSON `{title, widget_types, visible_text, tables, plot_series,
limitation, truncated}`. Widget types include `DT`, `plotly`, `leaflet` and
other htmlwidgets names. HTML/DT tables return up to 20 rows, 100 columns,
and 20 tables; plotly returns up to 100 traces with `{type, name, point_count,
axis_titles}`. Visible text is at most 16 KiB, individual cells/names 1 KiB,
and the entire serialized MCP result is at most 64 KiB, reducing tables,
series and text further as needed. Script source is never returned and
JavaScript is never executed. This is saved page content, not a live browser
snapshot: in-page plotly zoom, DT filtering/sorting, leaflet navigation,
and dynamic content without declarative widget data are unavailable.

### Bioinformatics discovery and evidence

The same MCP server also exposes `get_bio_guide`, `begin_analysis`,
`check_analysis` and `get_analysis`. The first tool is compile-time embedded
discovery, not another wire request: optional `id` selects one of
`bio-study-design`, `scrna-scanpy`, `scrna-seurat`, `bio-results-review`;
optional `resource` must exactly match a resource listed for that ID.
Catalog metadata includes guide/checker digests and explicit human-review status.
No discovery call starts a kernel, downloads data or installs software.

The other three tools forward the same-named wire requests. Their project
must match the MCP connection; the daemon also verifies record/session
ownership. `spec` binds workflow, goal, current guide/checker digests, declared
design and parameters, source files and visible code documents. `scrna`
additionally requires an existing session, a plain object variable name,
species/ID type and an explicit raw-count layer/assay with provenance.
`study_design` needs no session. Read the selected guide's
`references/checkpoints.md` for the exact request and table schemas.

`checkpoint` is `design`, or `input`, `qc`, `result` for scRNA. Result details
include a saved `.h5ad`/`.rds`, marker and annotation tables, parameters and
at most 32 actual successful `execution_run_ids` from that same session.
MCP execution replies and `get_console` expose these IDs. The daemon reads
their code/status from its own journal rather than trusting caller-supplied
proof. Fixed checker code uses the existing actor's atomic idle-only path;
busy sessions are not interrupted, queued behind user work or replaced.
Timeouts retain the execution and report incomplete evidence, never rerun
the analysis automatically.

`analyses` and `analysis_checks` share the existing journal. A measured input
baseline remains private; bounded responses separate declarations from facts.
Readable exports live in `.sandhill/analysis/<id>/`, with immutable per-check
JSON and a generated report; final results normally live in `results/<id>/`.
Managed exports reject symlink/hardlink aliases, input collisions and
unowned overwrite. Export failure is returned explicitly; journal authority
does not depend on a report being editable or successfully saved.
Mutable report replacement rechecks directory descriptors, file identity and
metadata immediately before atomic rename. This does not exclude an external
process writing in the final syscall window; journal/check records, not report
contents, remain authoritative. These tools are not an operating-system sandbox.
Explicit history cleanup removes authoritative analysis/run evidence
together, preserving project exports as historical files only.

Per-item states are `pass`, `warning`, `fail`, `not_checked`. A missing required
check cannot pass. `validity.current` only describes a fresh successful check
at that request; reads are historical, and changed files/code/session or
guide/checker versions require rechecking. Scientific validity and initial
guide review remain awaiting human review, not a correctness badge.
Stale MCP digests require restarting the agent with the matching daemon;
older servers report the feature unavailable. Ask and Plan permit only
reading guides/history among these four tools. Existing unrelated Plan tools
are unchanged; the CLI's independent shell/file tools are not sandboxed.


### Skills

Skills are device-wide folders with a `SKILL.md`, installed only after the
user confirms the preview. `list_skills` returns installed skills by name,
the pinned Suggested catalog (with installation status), and the four
built-in bioinformatics guides.

`preview_skills` accepts a tagged `fetch`: `git {url, commit?}`,
`suggested {id}`, `update {name}`, `zip {label, base64}`, or
`files {label, files, source?}`. Each file is `{path, base64, executable}`.
Git accepts `owner/repo`, GitHub repository and `/tree/<ref>/<folder>`
addresses, or another git URL. A shallow fetch records the exact commit;
Suggested uses its catalog pin, while Update fetches the repository's
current HEAD and restricts discovery to the installed skill's folder.
Uploads cannot be checked for updates. Export followed by Files with
`source` keeps the original Git source when copying between devices.

Previews live in `skills/.incoming/<token>/` under the daemon's state
folder. Tokens expire after one hour; each preview removes old incoming
folders. Install consumes its token, including on failure. It installs
only the selected names, replacing an existing folder when requested.
`skills/<name>/` keeps the files and executable bits; `skills/index.json`
records description, source, enabled state, install time, counts, scripts,
licence and a SHA-256 digest. Index writes use a temporary file and rename;
the daemon serializes skill operations. Replacement preserves the on/off
state.

Discovery looks for `SKILL.md` up to six folders deep, skipping `.git`.
Each candidate includes its whole folder tree and needs frontmatter
`name` and `description`. Names are lowercased, unsafe characters become
hyphens, outer dots and hyphens are trimmed, and the result is at most
64 characters. Empty names are refused. Candidates with symlinks, escaping
paths, more than 20 MiB or 2,048 files are refused. Duplicate names in a
preview are refused after the first. Git trees and uploads are limited to
100 MiB; zip archives and folder uploads to 10,000 entries. Zip paths and
symlinks are checked before extraction. Mac metadata files are ignored.
Script paths are shown in the preview. A nearest ancestor licence inside
the fetched tree is copied as `LICENSE` when the skill has none.
`replaces` signals a name already installed; `changed` compares file bytes,
relative paths and executable bits to the installed digest.

Every agent gets the read-only MCP tool `get_skill`. With no name it lists
enabled skills as `{skills: [{name, description}]}`. A name returns
`{name, description, path, files, text}`, with `SKILL.md` as the text.
`file` reads another relative UTF-8 text file inside the skill, at most
256 KiB, with no `..` components or symlinks. Disabled or missing skills
are not readable through this tool. Enabled skills also appear after the
four guides in `agent_commands`, as `kind: "guide"` picks that instruct
the agent to read the skill first. Sandhill does not write to agents'
own skill folders or run installed scripts on their behalf.

### Connectors

Connectors are MCP servers added per device: a web address or a program.
`list_connectors` reads the index without connecting. `add_connector` takes
`suggested {id}` or `custom {title, spec, secret?}`; a spec is
`http {url, header?}` or `command {command, args, env}`. Add initializes
the server and lists its tools before saving anything. Suggested entries
use their ids; custom names are lowercased safe title slugs with numeric
suffixes for duplicates. Only Suggested entries marked read-only in
Sandhill's catalog may be queried without approval.

The state folder's `connectors/index.json` records enabled state, source,
cached tool summaries and the last error. `connectors/secrets.json` keeps
header values by name, mode 0600 from creation. Both use atomic writes;
mutations are serialized. Replies, agent tool results, errors and exports
never include secret values. Export returns a Suggested id or a Custom
definition without its secret, for Add on another device.

The daemon speaks MCP 2025-06-18, 2025-03-26 and 2024-11-05 over stdio or
Streamable HTTP (JSON and SSE, session ids, protocol-version headers and
one reinitialization on an expired session). Initialization precedes the
initialized notification and paginated tool discovery. Calls time out at
60 seconds, including initialization; tool results over 1 MiB of JSON
are replaced with a truncation notice. Program connectors share one child
across conversations, use the user's login PATH, and stop after ten
minutes unused, when disabled or removed, and with their daemon.

Suggested BioMCP is downloaded only on Add, checked against the catalog's
pinned SHA-256 and extracted into `connectors/bin/biomcp-<version>/`.
BioContextAI requires uv on the device; Open Targets uses its web address.
No OAuth sign-in is implemented in this phase.

Agents get `list_connectors {connector?}`: without a name, cached
`{connectors: [{name, title, description, read_only, tools: [{name,
description}]}]}` for enabled connectors; with one, live `{name, title,
read_only, tools: [{name, description, inputSchema}]}` with full tool
descriptions. `query_connector {connector, tool, arguments}` is read-only
and refuses custom connectors. `call_connector` takes the same arguments
but is not read-only and is refused in Plan mode. Both preserve the
server's text/image content and tool-error flag. Missing or disabled
connectors are unavailable to agents; call failures update the row's
error. No connector is written into an agent's own settings.

### Agent accounts

Accounts belong to the device running the daemon, not to a project or to
the app. `AgentAccount` is
`{id, kind, name, method, default, signed_in?, identity?, usage?}`.
`kind` is `claude`, `codex` or `antigravity`; `method` is `login` or
`api_key`. `signed_in` is omitted when unknown, rather than reporting a
failed query as signed out. `identity` is the email or name the official CLI
reports. `usage` holds the last reading as
`{rate_limits: {<window>: {used_percentage, resets_at?}}, observed_at_ms}`.
Clients retain its timestamp and do not treat a missing reading as zero use.

`list_agent_accounts` synthesizes a "Default" account for each supported
agent, with `id` equal to its kind and `default: true`. These represent the
agent's existing login and cannot be renamed or removed. Added accounts
have stable IDs `<kind>:<slug>` and `default: false`; display names are
independent of IDs. Without `refresh`, listing uses the last known state.
With `refresh: true`, it asks the official CLIs about login and usage;
Sandhill does not extract OAuth credentials. The older `agent_accounts`
and `agent_accounts_query` calls remain read-only provider snapshots, not
the account registry.

For Claude and Codex, `add_agent_account` creates
`~/.local/share/sandhill/accounts/<kind>/<slug>/` with Sandhill metadata
in `account.json`. Runs receive that folder as `CLAUDE_CONFIG_DIR` or
`CODEX_HOME`. Extra Codex accounts use
`cli_auth_credentials_store="file"` for that run, keeping their login out
of a shared keyring.

The account folder links to the user's existing agent folder:

| Agent | Shared links |
|---|---|
| Claude | `projects`, `settings.json`, `CLAUDE.md`, `agents`, `commands`, `skills`, `plugins`, `todos` |
| Codex | `sessions`, `archived_sessions`, `config.toml`, `AGENTS.md`, `prompts`, `skills` |

Claude's `.credentials.json` and `.claude.json`, Codex's `auth.json`,
and Codex's SQLite databases are not linked. Links are created only inside
Sandhill's account folder. If missing, Sandhill creates only the empty
default `projects` (Claude) or `sessions` (Codex) directory so the shared
history link resolves. Optional links are made only for existing targets;
Sandhill does not create their files or directories or change credentials.
Removing an added account deletes its Sandhill-owned folder and login,
not the shared targets or the default login. Removal fails while a running
conversation or sign-in terminal uses that account.

Antigravity's added accounts require `api_key` in `add_agent_account`.
On macOS Sandhill keeps these in its own Keychain service,
`dev.sandhill.agent-account`, separate from Antigravity's Google login.
The selected key reaches only that run as `GEMINI_API_KEY`; keys are never
command-line arguments, returned account fields, or chat entries.
Antigravity's key authentication setting is a per-run `--settings`
override, not an edit of the user's settings.

`sign_in_agent_account` opens a terminal of kind `login` running the
agent's own interactive sign-in with that account's environment. Clients
display and subscribe to it as a terminal; Sandhill does not fill in,
read or copy login tokens. API-key accounts do not need interactive
sign-in. Signing in to Antigravity's default account changes its Google
login for the whole device, affecting every default-account Antigravity
conversation; clients must warn the user before offering this action.

Codex accounts' `usage.reset_credits` is `{available, expires_at, next_id,
observed_at_ms}`, read from Codex's `account/rateLimits/read`
(`rateLimitResetCredits`): how many banked resets are left, when the
soonest one expires (Unix seconds, null when it does not or details are
absent) and its id. `use_agent_reset` spends one through Codex's own
`account/rateLimitResetCredit/consume`, in the account's home, with the
`credit` asked for (else Codex picks) and a fresh idempotency key, then reads
the account again. `outcome` is Codex's: `reset`, `nothingToReset` (no limit
is used enough for a reset yet), `noCredit` or `alreadyRedeemed`. Claude Code
has no official way to report its limit resets, so Claude accounts never
carry `reset_credits`.

### Conversations kept with the project

A project keeps its agent conversations in its folder unless its settings
say `chat_history = machine`, so they go with the folder to another
computer (a portable drive, a synced copy). Each conversation still runs in
a terminal on the machine; every few seconds the daemon copies what changed
to `.sandhill/agents/<terminal id>/`, which `.sandhill/.gitignore` lists:

- `conversation.json`: its title, agent, options, `resume` (the agent's
  session, thread or conversation), `session_file`, `machine` (the one it
  last ran on) and, while it runs, `running_ms`, renewed every minute.
- `chat.jsonl`: the chat entries, as the terminal keeps them.
- `session.jsonl`: the agent's own copy of the conversation, from the
  `transcript` path the agent reports (Claude Code's transcript, Codex's
  rollout).

Both files only grow, so the longer copy is the newer, and the daemon copies
what one side has beyond the other: this machine's news to the folder, or
another machine's news into an ended terminal here. Only the machine with
news, else the one that wrote last, writes `conversation.json`.

`open_project` makes an ended terminal (with the project's path on this
machine as its `project` and `cwd`) for each conversation in the folder
that this machine doesn't have; `list_terminals` does again for the projects
opened since the daemon started. `restart_terminal` first puts
`session.jsonl` back where the agent looks (Claude Code's
`~/.claude/projects/<the path, every character but letters and digits made
a dash>/<session>.jsonl`, or under Codex's `~/.codex/sessions/`) and resumes
it. Without that copy (or for Antigravity, on another machine than it ran
on) the agent starts afresh and its first message says to read
`.sandhill/agents/<id>/earlier.md`, the chat as text. It fails while
`conversation.json` says another machine runs the conversation.
`close_terminal` removes the folder's copy too, all but a
`conversation.json` with `removed_ms` and the machine that removed it (no
title or chat). The other machines then remove theirs at their next copy, and
`open_project` doesn't bring it back; `restart_terminal` fails with "This
conversation was removed on ...". One still running on another machine stays
there and goes back to the folder, so nothing in use is lost. Logins never
move.

### Jobs

`start_job` runs a long script or pipeline in the background, in a terminal of
kind `job`, so it goes on when the app disconnects or the daemon restarts, and
its output is its log (`subscribe_terminal`, as above). `job` in its
`TerminalInfo` is what it runs, `{path?, command?}`. It runs in the user's
login shell, started as an interactive one so that conda and modules are set up
as in their Terminal, in `cwd`, else the project folder, else the script's
folder, with the project's env files. `path` is a script, which runs with the
program for its kind:

- `.R`: `Rscript`, and `.py`: `python`, each from the environment of the
  project's kernel for the language (its settings, else what its files point
  to), with that environment's `bin` first on `PATH`; without one, the shell's
  `Rscript` or `python3`;
- `.sh`: `bash`; `.nf`: `nextflow run`; `.smk` or `Snakefile`: `snakemake
  --snakefile` (with `--cores all` when nothing else is said);
- `.qmd`/`.Rmd`: `quarto render <absolute-file>`, using the same Quarto
  discovery as previews. `.md` is not treated as a render job. Rendering
  starts in the document's directory even when a different `cwd` is given;
  Quarto discovers the containing project and applies its own project rules.
- any other file runs itself, if it is executable.

`command` is the arguments after the script, or without `path`, a command line
for the shell. A job's `title` is the script's name or the command line.
`restart_terminal` runs a job again as it was started, its log going on;
`stop_terminal` ends it (and works for any terminal), keeping it and its log;
`close_terminal` removes it. While it runs, `system_stats` says how much CPU and
memory it uses.

Quarto rendering uses a fresh process, not live kernel variables. Missing
Quarto is an error with installation advice. On a successful render, the
independent terminal host parses Quarto's `Output created: ...` lines
(including chunked output and terminal escape sequences), resolves existing
output files relative to the document directory, and persists their canonical
paths. They appear as `output_paths: ["/data/project/report.html"]` in
`TerminalInfo` and the terminal finish message alongside `exit`. This uses
Quarto's actual outputs, including format/project output choices, rather than
guessing extensions. Failed renders and restarted jobs clear previous outputs.
The paths remain available after app close or daemon restart. Clients can
open HTML with `serve_page`, or PDFs with the existing file-preview calls.

```json
{"id": 22, "method": "start_job", "params": {"path": "/data/project/report.qmd", "project": "/data/project"}}
{"type": "terminal", "terminal": {"terminal": "job-id", "exit": {"code": 0, "ended_ms": 42}, "output_paths": ["/data/project/report.html"]}}
```

## Windows and agents' edits

A project window tells the daemon where the user is with `set_focus`:
`Focus` is `{file?, cell?, line?, selection?, open, table?, ts_ms}`, the document in
front, its current cell (the chunk with the cursor, or the notebook cell last
in focus), the cursor's line, the selected text, and every document open in
the window. The daemon keeps the latest place per project, also after the
window has gone (`ts_ms` says when); `get_focus` returns it.

The window sends `table` only while a table viewer is focused, and clears it
when focus leaves the table. Its shape is `{session, path, name, sort,
filters, search?, hidden_columns, selected_column?, selected_cells?, shown_rows,
total_rows}`. Sort/filter values reuse the table request types, and `search`
defaults to empty with the same semantics as table requests; column
positions are always original zero-based indices. Each selected cell is
`{row, column}`, with `row` zero-based in the filtered/sorted view.
`shown_rows` is the full filtered row count, not the current page length.
MCP `view_table` uses this search along with the current filters, sorting
and hidden columns, unless `view: "original"` is requested.
`get_focus` mentions this state even without a document
in front. The daemon supplies `ts_ms`; old clients can omit `table`.

```json
{"id": 23, "method": "set_focus", "params": {"project": "/data/project", "focus": {"open": [], "table": {"session": "s", "path": ["adata", "obs"], "name": "adata.obs", "sort": [{"column": 2, "descending": true}], "filters": [{"column": 1, "op": "=", "values": ["T"]}], "hidden_columns": [0], "selected_column": 2, "selected_cells": [{"row": 0, "column": 2}], "shown_rows": 12, "total_rows": 30}}}}
```

`add_cell` puts code into a document as a new cell after cell `after`, or
after the last cell (the request to a window then has no `after`): a chunk in a `.qmd`/`.Rmd`, a code cell in a notebook,
or code at the end of an `.R`/`.py` script. `edit_cell` replaces a cell's
code. `delete_cell` removes a notebook cell (only `.ipynb`; a window can bring
it back). `by` names the agent asking, for the window to show. The result is `CellEdited`: `{file, cell_id, language, in_window,
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

An agent started with `code: "console"` (in `agent.code`, switched with
`tell_terminal` `{kind: "set", code: "console" | null}`) doesn't add cells:
its `add_cell` runs the code in the document's session instead, with `by`
naming the agent, and leaves the document as it is. Its `run_code` always
runs that way.

The message box's choices are kept in `agent` too, from the options an
agent starts with and `tell_terminal` `{kind: "set", pin?, web?, refs?}` for
either agent:

- Each message works in a document: the first notebook or Quarto file it
  names in `refs`, else `pin`; none otherwise, not the one in front of the
  user. It records it as `agent.document` (null when there is none), so a
  working agent keeps the file it started in and the next message starts
  over. `add_cell` writes into the `file` it is given, else
  `agent.document`, else `pin`, else the first of `documents`; with none of
  them it runs the code in the Console. The app no longer sets `pin` or
  `documents`: conversations are about the whole project, and the user names
  a file with @ (and a notebook's cell with #, a `chunk` ref).
- `web: false` turns web search off: each message says so, and for Claude
  Code the hook before each step blocks `WebSearch` and `WebFetch` (a
  `step_failed` entry). Codex is told only.
- `refs` is what the user added to their next message with the @ menu:
  `[{kind: "file" | "chunk" | "variable" | "plot" | "attach", name, path?,
  cell?, session?, language?}]`. The message takes them (`agent.refs`
  becomes null), the agent is told of each, and the chat's `prompt` entry
  lists their names in `place.refs`. Codex's `say` command can carry `refs`
  itself.

## The Viewer: pages and tunnels

An output is a **page** when it has `application/vnd.sandhill.page+json` or
a plotly figure (`application/vnd.plotly.v1+json`), or `text/html` that runs
scripts, says it is `isolated` (IRkernel's metadata), or is kept as a blob.
Other HTML, such as a data frame's table, stays in the Console. The rule is
`outputs::is_page` here and `isPage` in the app; `tests/fixtures/pages.json`
keeps the two in step.

R sessions get a `viewer` option when they start (IRkernel sets none). It
prints the page's file, or a web address, between `ESC ]sandhill-page;` and
`BEL`, to stdout in IRkernel and to stderr in Ark (which holds back stdout
while it prints a value, and printing a `shinyApp()` runs the app); IRkernel
sends a web address as a `display_data` through its display function. The
daemon takes that out of the stream and sends a `display_data` in its place:

```json
{"data": {"application/vnd.sandhill.page+json": {"path": "/tmp/RtmpX/viewhtml1/index.html", "title": "datatables"},
          "text/html": "<!DOCTYPE html>…", "text/plain": "datatables (in the Viewer)"},
 "metadata": {"text/html": {"isolated": true}}}
```

`text/html` is the page with the scripts, styles and pictures beside it put
inside it, so it outlives R's temporary folder; a notebook shows it under
its cell. An address gives `{url, title}` and no HTML.

The `browser` and `shiny.launch.browser` options go the same way, so
`browseURL()` and a Shiny app (`shiny::runApp()`, or a printed
`shinyApp()`) say where they are. An address on the daemon's machine
(`localhost`, `127.0.0.1`), such as an app's, gives `{url, title}`, which the
Viewer opens; any other gives `{url, title, "browser": true}` and the text
"Opening … in the browser", which the app opens in the browser and the
Viewer leaves out. IRkernel sends what R prints only once the code is done,
and an app's code never is, so there R shows an address as a `display_data`
of `{url}` itself, which the daemon completes in the same way. Stopping an
app is interrupting its run.

A page a kernel shows
itself (IRkernel's htmlwidgets, a plotly figure's HTML) gets the type too,
with only its `title`, when it has one.

`serve_page` serves a file and the folder it is in over HTTP on
`127.0.0.1`, at a port the daemon picks: `http://127.0.0.1:{port}{path}`,
where `path` is signed for the folder, so only that folder's files are
served. A folder's address serves its `index.html`. `serve_html` serves HTML
(the text, or the blob holding it) from a folder of the daemon's own, which
keeps the latest 50.

`preview_document` runs `quarto preview <path> --no-browser --port <port>`
in a terminal of kind `preview`, in the document's folder, with the
project's env files' variables and, when the project names a Python kernel,
its Python as `QUARTO_PYTHON`. Quarto renders the document again each time
it is saved, and its page reloads itself. The call answers once the page
answers, with `url` (`http://127.0.0.1:{port}/`, or the page Quarto says to
browse, for a document in a Quarto project) and the terminal, which
`close_terminal` ends; a preview of the same file still running is used
again. It fails with the end of Quarto's output if Quarto stops first, and
says how to install Quarto when it isn't found (on the `PATH`, at
`$QUARTO_PATH`, in a conda environment, or bundled with RStudio or
Positron).

A **tunnel** carries a TCP connection to a port on the daemon's machine over
the client's own connection, as `ssh -L` would, so the app reaches a
server's page server and apps without a second login. The client numbers
each tunnel; `open_tunnel` answers once connected, or with an error when
nothing answers on the port. Bytes go as `tunnel_write` and come back as

```json
{"type": "tunnel", "tunnel": {"id": 3, "data": "SFRUUC8xLjEgMjAw…"}}
{"type": "tunnel", "tunnel": {"id": 3, "closed": true}}
```

in order; `closed` says the daemon's side ended. A tunnel ends with
`close_tunnel`, or with the client's connection.

PDF/image tabs reuse the same page server and tunnel for streamed original
bytes, not full-file base64 RPC. The Mac creates a private temporary cache and
uses system decoders; SVG is rasterized without scripts or external resources.
Document selections persist separately in project state
`documents[absolute_file].kernel`, not in shared notebook metadata.

## Blobs

Binary MIME values (images other than SVG, PDF) and any value over 64 KiB are
stored once, by SHA-256, and replaced in the event with
`{"sandhill_blob": "<hash>", "size": <bytes>}`. Fetch them with `get_blob`.
The data is the original JSON string (base64 for images).

## Files and notebook outputs

Paths are absolute on the daemon's machine (a leading `~` is expanded), so
the app edits files the same way locally and on a server.

- `write_file` replaces the file atomically, making its folder if need be.
  Pass the `mtime_ms` you read to refuse the write, with an error starting
  `conflict:`, if the file changed on disk in the meantime. With
  `base64: true`, `text` is the file's bytes in base64, for a saved plot.
- With `create: true`, `write_file` makes a new file and fails, with an
  error starting `exists:`, if anything is already at `path`. `make_dir`,
  `move_path` and `copy_path` fail the same way rather than replace
  anything, and make the folders `path` or `to` need. `move_path` may change
  only the case of a name. Links are copied as links.
- `delete_path` removes a file, or a folder and everything in it. With
  `trash: true` it moves it to the machine's Trash instead: `~/.Trash` on
  macOS, the freedesktop.org trash (`$XDG_DATA_HOME/Trash`, with an `info/`
  file saying where it came from) on Linux, so Finder or the file manager
  can put it back. Names already in the Trash get a number: `notes 2.md`.
- `list_dir` with `sizes: true` gives each folder the sum of its files'
  sizes, for sorting by size. It looks at 200,000 entries at most, so very
  large folders show a partial size.
- `read_bytes` reads a file in parts for downloading it to the Mac; ask
  again from `offset + ` the bytes returned until `size` is reached.
  The client's bounded UTF-8 reader uses this established API rather than a
  separate RPC: metadata first, chunks no larger than 4 MiB, an EOF probe and
  final metadata. Path, offset, size and modification time must agree; short,
  oversized or malformed chunks fail rather than produce partial text.
  Decode UTF-8 after joining bytes, allowing a codepoint across a chunk boundary.
  CSV/TSV text opening allows exactly 5,000,000 bytes and rejects larger files
  before fetching content. It never falls back to an unbounded `read_file`.
  The historical protocol cannot detect a same-size edit that preserves its
  millisecond modification time.
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
