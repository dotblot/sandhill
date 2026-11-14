# Sandhill

A fast, low-memory IDE with AI coding agents for R and Python bioinformatics,
built for Apple silicon Macs that work against a remote Ubuntu server (or locally).

Think "Positron without Electron": inline outputs in Jupyter and Quarto that are
saved and come back when you reopen a file, R and Python sessions that keep
running on the server after you close the app, and Claude Code, Codex or open
and local models (through omp) working directly in your live sessions.

> Status: early development. The Mac app opens projects of Quarto and Jupyter
> notebooks and R and Python scripts, runs them on this Mac or on a Linux
> server over ssh, and runs Claude Code and Codex next to them. See the
> roadmap below.

## How it fits together

```
Mac app (Rust, GPUI)           ──ssh or local socket──▶  sandhilld (Rust daemon)
                                                            ├─ Python / R kernels (Jupyter protocol)
                                                            ├─ output journal (SQLite + blobs)
                                                            ├─ jobs, terminals, language servers
                                                            └─ AI agents + MCP tools for live sessions
```

The full design, including the reasoning behind each choice, is in
[docs/design.md](docs/design.md).

## Repository layout

| Path | What |
|---|---|
| `gpui/` | The Mac app, drawn with GPUI (its own Cargo project; see [gpui/README.md](gpui/README.md)) |
| `crates/daemon` | `sandhilld`: kernel sessions, output journal, socket server |
| `crates/proto` | Wire protocol types and the async client the app and CLI share |
| `crates/link` | How the app reaches sandhilld: on this Mac, or over ssh (installing it there) |
| `crates/cli` | `sandhill`: terminal client, useful for scripting and testing |
| `packaging/` | systemd (server) and launchd (Mac local mode) service files |
| `docs/` | [Design](docs/design.md) and [protocol](docs/protocol.md) |

## Try the Mac app

You need Rust and Jupyter kernels for the languages you want: ipykernel in
your Python environment (conda environments are found on their own; others
need `python -m ipykernel install --user`), and for R,
[Ark](https://github.com/posit-dev/ark) or IRkernel.

```sh
cargo build -p sandhill-daemon        # at the repo root, once
cd gpui
cargo run -- /path/to/project         # the folder is optional
```

Every push to `main` (and every pull request that touches the app) builds
Sandhill.app, with sandhilld inside and Linux builds of it and Ark to install
on servers: open the **Mac app** run on GitHub's Actions tab and download
**Sandhill-macos-arm64** under Artifacts (to build one without a push, pick
**Mac app** there, then **Run workflow**). The app isn't notarized, so after
unzipping run `xattr -dr com.apple.quarantine Sandhill.app` once, or macOS
will refuse to open it. `.github/workflows/gpui-mac.yml` shows how the bundle
is put together.

The app opens on a start window: open a folder with your analysis as a
project, and it gets its own window. Projects open at Quit come back at the
next launch with their tabs, and each document reattaches to the kernels that
last ran it. Code keeps running after you quit; reopening a file shows its
outputs again, including output printed while the app was closed. Quarto and
R Markdown outputs are saved next to the file in `.<name>.out/`.

### Work on a server

On the start window, click **+ Server…** and type the server as you would
after `ssh`: a `Host` from `~/.ssh/config`, or `user@host`. Pick it, open a
folder there, and the project window works as it does on the Mac.

- The app runs your Mac's own `ssh`, so `~/.ssh/config`, keys, the ssh agent
  and jump hosts work as they do in Terminal. It can't type a password for
  you: `ssh yourserver` must log in without asking for one (set up a key
  with `ssh-copy-id yourserver`).
- The first time, the app installs sandhilld in `~/.local/bin` on the
  server, and replaces it when a newer app brings another build. It needs
  no root and opens no ports, and runs on any 64-bit Linux, Intel or ARM.
- Kernels start with the `PATH` your login shell sets up, so conda and the
  tools your shell startup files add are found. Python runs in any conda,
  mamba or virtual environment that has `ipykernel`.
- When the server has R, the app also installs Ark, Posit's R kernel, in
  `~/.local/share/sandhill/bin`, so R chunks run with no setup. R kernels
  you installed yourself, such as IRkernel, are listed too.
- Code keeps running on the server when the app quits or the network drops.
  The app reconnects on its own and shows what was printed in the meantime.

### Agents

The chat button in the toolbar (⌘I) opens the Agents pane. Pick Claude Code
or Codex, a model, how much it may do without asking, and the files it should
start from, then **Start session**. The agent is the real CLI, running in a
terminal in the project's folder on the machine the project is on (the server,
for a server project), so it works exactly as it does in Terminal: its own
login, settings, slash commands and keys.

- The agent must be installed there. If it isn't, the pane offers to install
  it in the Console's Terminal tab. The first session asks you to sign in,
  as it would in Terminal; Sandhill never sees the login.
- Agents can use your live R and Python sessions through MCP tools
  (`list_sessions`, `list_variables`, `run_code`, `get_outputs`), so they
  look at the objects you already have in memory instead of recomputing them.
- Files an agent edits reload in the editor. If you changed the file too,
  you choose which version to keep.
- Agent sessions and terminals keep running when the app quits, like
  kernels, and come back with their scrollback when the project reopens.
- The box under the terminal sends messages, adds files as `@path` context,
  and switches model and permissions. The status bar shows when an agent is
  waiting for you, and Claude's plan usage.

The Console also has a **Terminal** tab: a login shell in the project's folder.

### Bioinformatics guides and checks

The existing agents share four daemon-provided MCP tools: `get_bio_guide`,
`begin_analysis`, `check_analysis` and `get_analysis`. Ask the agent to use
Sandhill's bioinformatics checks for an explicitly requested study design or
exploratory single-cell analysis. Guide discovery and history are read-only;
creating a record or running a check is blocked in Ask and Plan modes.

Python analysis and plotting prefer the installed **omicverse** interfaces,
using Scanpy or other tools where needed; R uses **Seurat**. Neither discovery
nor record creation installs packages, downloads data or starts another kernel.
Analysis code stays in your document and runs in the selected existing session.
The guides describe exact inputs, QC exclusions, saved-object and marker tables.
Execution replies and Console history expose actual run IDs for result evidence.

Checks write authoritative evidence into the daemon's existing journal and
readable exports into `.sandhill/analysis/<id>/`; results normally go into
`results/<id>/`. Read the returned `analysis.id`, then request `design`, or
`input`, `qc` and `result` checkpoints. Result checks reopen the saved `.h5ad` or
`.rds`, compare it with the live object, verify cited expression summaries and
retain actual completed execution IDs. Handwritten reports are not check results.
Historical records are not current revalidation; recheck after changes.

All initial guides are **awaiting human domain review**, not reviewed default
recommendations. A technical pass is not scientific approval: experimental
design, QC decisions, annotations and conclusions still need human review.
PBMC3k is single-donor tutorial data, not evidence of treatment effects.
The agents' own shell/file tools are not sandboxed by these checks.
See the [analysis contract and limitations](docs/design/bioinformatics-assistance-plan.md).


## Try the daemon

```sh
cargo build --release                                # sandhilld and the sandhill CLI
./target/release/sandhilld serve &                  # or install packaging/systemd/sandhilld.service
./target/release/sandhill kernels
id=$(./target/release/sandhill start python3)
./target/release/sandhill run "$id" 'print("hello")'
./target/release/sandhill history "$id"             # everything that ran, even while you were away
```

Against a server, copy `sandhilld` to `~/.local/bin` there and add
`--ssh yourserver --remote-daemon '~/.local/bin/sandhilld'` to any `sandhill`
command; the daemon starts on first use.

Run the tests with `cargo test --workspace`, and `cargo test` in `gpui/` for
the app (on Linux it needs `libxkbcommon-dev libxkbcommon-x11-dev
libwebkit2gtk-4.1-dev` and X11). Kernel tests skip when a kernel isn't
installed.

## Roadmap

1. **Server core** (done): `sandhilld` starts Python and R kernels, runs code, and records every output so a client can disconnect and replay later.
2. **First Mac app loop** (done): open `.qmd`/`.ipynb`, run a chunk, see inline output, close and reopen with outputs intact.
3. **Server and projects** (done): RStudio-style projects, each in its own window; connect to an Ubuntu server over ssh, with work that keeps running when the app closes.
4. **Agents** (in progress): Claude Code and Codex in a terminal pane and MCP tools for live sessions are done; omp chat for open and local models is next.
5. **Positron-style panes**: Variables, Data Explorer, Plots, Help, Viewer.

## License

Sandhill is free and open-source software, and a non-commercial project:
there is no paid version. It is licensed under the GNU General Public License,
version 3 or (at your option) any later version; see [LICENSE](LICENSE). You
may use, change and share it, and a changed version you share must be under
the same license, with its source code.

The Maple Mono NF font the app ships is under the SIL Open Font License
([gpui/assets/fonts/OFL.txt](gpui/assets/fonts/OFL.txt)), and the libraries
Sandhill builds on keep their own licenses (MIT, Apache-2.0 and similar).
The interface and file/integration artwork comes from Gruvbox Material Icons,
Lucide and Developer Icons under their upstream MIT/ISC licenses; see the
[icon attribution notices](docs/design/ui/icons/vendor/NOTICE.txt) and
[source mapping](docs/design/ui/icons/MAPPING.md). Vendor trademarks remain
their owners' property.
The Posit Assistant and Antigravity marks use user-supplied references; their
public-release permissions are not supplied by those icon-library licenses.
