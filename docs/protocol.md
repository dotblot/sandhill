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
| `execute` | `session`, `code`, `cell?: {file, cell_id}`, `quiet?`, `by?` | `{run, msg_id}` |
| `session_runs` | `session`, `limit?` | `{session, runs, seq}`: the session's latest runs, for its Console |
| `session_plots` | `session`, `limit?` | `{session, plots, seq}`: the images the session's runs showed, for the Plots pane |
| `redraw_plot` | `session`, `run`, `image?`, `index?`, `file?`, `format?`, `width?`, `height?`, `dpi?` | `{can, reason?}`: whether the session can draw the plot again, or draws it into `file` |
| `list_variables` | `session`, `path?` | `Variables` |
| `delete_variables` | `session`, `names` | `null` once they are gone and memory is collected |
| `table_rows` | `session`, `path`, `offset?`, `limit?`, `sort?`, `filters?` | `TableRows`: a page of a data frame or matrix, for the Data Explorer |
| `table_summary` | `session`, `path`, `filters?` | `TableSummary`: each column's counts, range and commonest values |
| `table_export` | `session`, `path`, `sort?`, `filters?`, `file` | `{path, mtime_ms}` once the CSV is written |
| `help_topics` | `session`, `query?`, `package?`, `limit?` | `HelpTopics`: the help topics that match, for the Help pane |
| `help_page` | `session`, `topic`, `package?` | `HelpPage`: R's page as HTML, or Python's signature, docstring and members |
| `help_source` | `session`, `topic`, `package?` | `{session, topic, package, text, file?, line?}`: a function's source |
| `list_packages` | `session` | `Packages`: what's installed for the session's language |
| `package_updates` | `session` | `PackageUpdates`: R packages with newer versions on CRAN or Bioconductor |
| `git_status` | `path` | `GitStatus`: the branch, the changed files and the refs of the repository `path` is in |
| `git_log` | `path`, `skip?`, `limit?` | `{commits, more}`: a page of every branch's commits, newest first |
| `git_show` | `path`, `sha` | `GitCommitInfo`: a commit's whole message and the files it changed |
| `git_diff` | `path`, `file`, `staged?`, `sha?` | `{file, text, truncated, binary}`: a file's unified diff |
| `git_run` | `path`, `action` | `{output}` once git has done it: stage, commit, push and the rest |
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
| `read_bytes` | `path`, `offset?`, `limit?` | `{path, offset, data, size, mtime_ms}`: up to 4 MiB of the file, `data` in base64 |
| `list_files` | `path`, `limit?` | `[path]`: the files under the folder, relative to it, nearest first; hidden files and package folders (`node_modules`, `renv`, …) left out; at most 5000 unless `limit` |
| `file_outputs` | `file` | `{file, cells, seq}` |
| `open_project` | `path` | `ProjectInfo` with its saved `state` |
| `list_recent_projects` | | `[ProjectInfo]`, most recent first, `state` null |
| `save_project_state` | `path`, `state` | `null` |
| `forget_project` | `path` | `null` |
| `get_project_settings` | `path` | `ProjectSetup`: the project's settings and the environments its files point to |
| `save_project_settings` | `path`, `settings` | `ProjectSetup` once `.sandhill/project.toml` is written |
| `list_project_kernelspecs` | `path` | `[{name, display_name, language, env?}]`: `list_kernelspecs` and the kernels of the environments inside the project's folder |
| `system_stats` | `path?` | `SystemStats` for the daemon's machine |
| `read_log` | `bytes?` | `{path, text, size}`: whole lines from the end of the daemon's log, 64 KiB unless `bytes` says |
| `stop_daemon` | | `null`, then the daemon stops; kernels and terminals keep running for the next one |
| `stat_files` | `paths` | `[{path, mtime_ms?, size}]`; no `mtime_ms` when a file is missing |
| `list_agents` | | `[{kind, name, path?, version?, install}]` |
| `start_terminal` | `kind`, `cwd?`, `project?`, `cols?`, `rows?`, `options?`, `prompt?` | `TerminalInfo` |
| `restart_terminal` | `terminal`, `options?`, `cols?`, `rows?`, `prompt?` | `TerminalInfo` |
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

A project's shared settings live in `.sandhill/project.toml`, which can be
committed. `ProjectSettings` is `{r?, python?, documents, env_files}`: the
kernel each language runs in (none: detect it), where notebook and Quarto
chunks run (`document`, their own folder, or `project`), and the env files,
relative to the project, whose `NAME=value` lines its kernels start with
(`[".env"]` unless set). `save_project_settings` makes `.sandhill/` the first
time, with a `.gitignore` for `outputs/`, and keeps the rest of an existing
file, comments included. `ProjectSetup` is `{settings, saved, problem?,
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

`redraw_plot` draws one of a run's plots again, in the session that drew it,
at another size. The plot is the image holding the blob `image`, else the
`index`-th image the run showed. Ark keeps a recording of each plot it shows
(named by the output's `transient.display_id`), and a Python session keeps its
last 10 matplotlib figures: code run when the session starts tags each
figure's image with `sandhill_plot` in the output's metadata. IRkernel's
plots, plain images and ended sessions can't be drawn again; `{can: false,
reason}` says why, as in "only Ark keeps R plots to draw them again". Without
`file`, the call only asks. With it, the session draws the plot into `file`
(a path, or `~/...`; its folder is made) as `format` (`png`, `svg` or `pdf`),
`width` × `height` inches (0.5 to 100) at `dpi` (36 to 2400, for PNG), out of
sight, so nothing shows in the Console or the Plots pane. It fails while the
session is busy.

`list_variables` runs a quiet snippet in the session and returns `Variables`:
`{session, path, variables: [{name, kind, class, size?, children, table,
summary}], more}`. `kind` is `data`, `value` or `function`; `size` is in bytes
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

`table_rows` returns `{session, path, code, matrix, columns, width, total,
rows, offset, labels, values}`: `code` is the table as code in the session's
language (`seu@meta.data`, `adata.obs`); `matrix` says it is a matrix (or a
NumPy array) rather than a data frame; `columns` is `[{name, kind, type}]` for
the first 500 columns of the `width`, `kind` being `number`, `text`,
`category`, `logical`, `date` or `other` and `type` the language's own
(`dbl`, `fct`, `float64`); `total` counts the table's rows and `rows` those
that pass the filters. Then from `offset`, `limit` rows (200 unless asked, at
most 2000): each row's name or index label in `labels`, and its cells in
`values`, numbers in number columns, text otherwise, `null` where missing.

`table_summary` returns `{session, path, rows, columns}` for the rows that
pass the filters, a summary per column: `{column, missing, distinct?, min?,
max?, mean?, median?, histogram, top}`, with a 20-bin `histogram` from `min` to
`max` for numbers, and for other columns of at most 1000 distinct values the
10 commonest as `top: [{value, count}]` (a factor's in level order when it has
at most 10).

`table_export` writes the rows that pass the filters, sorted, to a CSV `file`
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
minutes). Python's aren't checked yet.

The `git_*` calls run the `git` program on the daemon's machine (a command
line git, which on macOS is the Xcode one only when its tools are installed),
in the root of the repository that `path` is in. Git never asks for a password
there: a remote that needs one fails with git's message, so ssh keys or a
credential helper must already work in a terminal. Reads time out after a
minute, changes after five.

`git_status` returns `{repo, branch?, head?, upstream?, ahead, behind, files,
more, stashes, state?, merge_message?, refs, branches, remote_branches,
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
a client reads the log again only then. `user_name` and `user_email` are what
a commit would be signed with.

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
`pull.ff` says otherwise; `push` sets the branch's upstream the first time,
on `origin` or the only remote; `undo_commit` moves the branch back one commit
and keeps its changes staged; `switch` takes a `branch` (a remote one makes or
checks out the local branch that tracks it); `create_branch` takes a `name`
and switches to it; `merge` takes a `branch`, local or remote, and merges it
into the one checked out (`merge --no-edit`; stopping on conflicts isn't a
failure: `state` becomes `merging` and the files show as conflicts);
`resolve` takes a conflicted `file` and a `side`, `ours` or `theirs`, and
stages that side's version of all of it (deleting the file when that side
did); `stash` (with an optional `message`, new files included)
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
`antigravity` for Antigravity's `agy`), each running
unmodified in its own pseudo-terminal on the daemon's machine. `list_agents`
says which agent CLIs are installed, and how to install the others.

`TerminalInfo` is `{id, kind, title, cwd, project?, created_ms, started_ms,
running, exit?: {code?, signal?, ended_ms}, end, options?, agent?,
outdated?, name?}`. `started_ms` is when its program last started. `end`
is the number of output bytes so far. `options` is what the agent was started
with: `{model?, effort?, permission?, documents?, code?, pin?, web?, context?,
refs?}`, where `permission` is `supervised`, `edits`, `auto`, `full`, `plan`,
`restricted` or `ask`, `code: "console"` sends the agent's code to the
Console (below), and the rest are the message box's choices (below).
`name` is what the user named it with `rename_terminal` (a blank `name`
takes it away). `outdated` is true for an agent that
another build of sandhilld started, such as before an update: it keeps that
build's MCP server, hooks and instructions until it is started again, so the
app suggests a new session. Without `cwd`, a terminal starts in the `project`
folder, else the home folder. An agent's `prompt` is its first message, which
it answers as it starts.

`restart_terminal` starts a terminal's program again in the same terminal,
ending it first if it still runs (an exit with `ended_ms` before the new
`started_ms` is the old program's). Its output and conversation go on where
they were. An agent resumes its conversation: Claude Code the session its
hooks last reported (`agent.session_id`), Codex its thread
(`agent.thread`); of the old `agent` report, only those stay (and the
guard, below, of the mode it starts in). `options`
replace the ones it started with, so a Claude Code conversation can go on in
a mode it can't switch to while running, such as `full`. `prompt` is a
message to go on with.

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
`{state: "busy" | "waiting" | "idle", message?, mode?, session_id?, status?, ts_ms}`.
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
isn't connected wait for it. Its `agent` reports also have `thread` (Codex's
thread id), `permission` (the mode it is in, named as in `options`), `models` (`[{id, label, efforts?,
effort?, default?}]`), `account` (`{type, email?, plan?}`, or null when it
isn't signed in), `auth_required` and `login` (`{url, code?}` while signing
in, or `{error}`).

Antigravity runs the same way, with its own bridge, `sandhilld agy-chat`,
which runs `agy` in its headless mode once per message and goes on with
agy's conversation. It takes `say`, `interrupt` and `set` (permission
`supervised`, `full` or `plan`), and reports `thread` (agy's conversation
id), `permission` and `models` as Codex's bridge does. Its steps take the
shapes above for commands (`Bash`), new files (`Write`) and Sandhill's tools;
agy's other tools keep agy's names and parameters.

Each agent gets Sandhill's MCP server, `sandhilld mcp --project <folder>`,
which it starts itself (Antigravity's from its settings, as `sandhilld mcp`
with the project in `SANDHILL_PROJECT` and the socket in `SANDHILL_SOCKET`). Its tools reach the user's live sessions through this
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

An agent started with `code: "console"` (in `agent.code`, switched with
`tell_terminal` `{kind: "set", code: "console" | null}`) doesn't add cells:
its `add_cell` runs the code in the document's session instead, with `by`
naming the agent, and leaves the document as it is. Its `run_code` always
runs that way.

The message box's choices are kept in `agent` too, from the options an
agent starts with and `tell_terminal` `{kind: "set", pin?, web?, context?,
refs?}` for either agent:

- `pin` (a file, or null) is the document the conversation works in.
  Unpinned, each message works in the document in front of the user, and
  records it as `agent.document`, so a working agent keeps the file it
  started in. `add_cell` writes into `agent.document`, else `pin`, else the
  first of `documents`.
- `web: false` turns web search off: each message says so, and for Claude
  Code the hook before each step blocks `WebSearch` and `WebFetch` (a
  `step_failed` entry). Codex is told only.
- `context: false` keeps where the user is out of their messages.
- `refs` is what the user added to their next message with the @ menu:
  `[{kind: "file" | "chunk" | "variable" | "plot" | "attach", name, path?,
  cell?, session?, language?}]`. The message takes them (`agent.refs`
  becomes null), the agent is told of each, and the chat's `prompt` entry
  lists their names in `place.refs`. Codex's `say` command can carry `refs`
  itself.

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
