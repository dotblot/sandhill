# Sandhill

A fast, low-memory IDE with AI coding agents for R and Python bioinformatics,
built for Apple silicon Macs that work against a remote Ubuntu server (or locally).

Think "Positron without Electron": inline outputs in Jupyter and Quarto that are
saved and come back when you reopen a file, R and Python sessions that keep
running on the server after you close the app, and Claude Code, Codex or open
and local models (through omp) working directly in your live sessions.

> Status: early development. The Mac app opens projects of Quarto and Jupyter
> notebooks and runs them on this Mac or on a Linux server over ssh; agents
> are next. See the roadmap below.

## How it fits together

```
Mac app (Tauri, TypeScript UI)  ──ssh or local socket──▶  sandhilld (Rust daemon)
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
| `app/` | The Mac app: TypeScript UI in `src/`, Tauri shell in `src-tauri/` |
| `crates/daemon` | `sandhilld`: kernel sessions, output journal, socket server |
| `crates/proto` | Wire protocol types and the async client the app and CLI share |
| `crates/link` | How the app reaches sandhilld: on this Mac, or over ssh (installing it there) |
| `crates/cli` | `sandhill`: terminal client, useful for scripting and testing |
| `packaging/` | systemd (server) and launchd (Mac local mode) service files |
| `docs/` | [Design](docs/design.md) and [protocol](docs/protocol.md) |

## Try the Mac app

You need Rust, Node 22 and Jupyter kernels for the languages you want:
ipykernel in your Python environment (conda environments are found on their
own; others need `python -m ipykernel install --user`), and for R,
[Ark](https://github.com/posit-dev/ark) or IRkernel.

```sh
cd app
npm install
npm run bundle        # builds target/release/bundle/macos/Sandhill.app, with sandhilld inside
open ../target/release/bundle/macos/Sandhill.app
```

For the app to install sandhilld on servers, the bundle needs Linux builds
of it, which `npm run bundle` makes when cargo-zigbuild is installed (CI
builds always have them):
`rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl`,
`cargo install cargo-zigbuild` and `brew install zig`.

Every CI run also builds the app: open the run on GitHub's Actions tab and
download **Sandhill-macos-arm64** under Artifacts. The app isn't notarized,
so after unzipping run `xattr -dr com.apple.quarantine Sandhill.app` once,
or macOS will refuse to open it.

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

To work on the UI, `cargo build -p sandhill-daemon` once, then
`npm run tauri dev` in `app/` reloads the window as you edit. Without the
app, `npm run bridge` and `npm run dev` serve the same UI to a browser; the
page is the start window, and `?project=<folder>` makes it a project window.

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

Run the tests with `cargo test --workspace` (on Linux the app needs the
WebKitGTK development packages; see `.github/workflows/ci.yml`) and
`npm test` in `app/`. Kernel tests skip when a kernel isn't installed.

## Roadmap

1. **Server core** (done): `sandhilld` starts Python and R kernels, runs code, and records every output so a client can disconnect and replay later.
2. **First Mac app loop** (done): open `.qmd`/`.ipynb`, run a chunk, see inline output, close and reopen with outputs intact.
3. **Server and projects** (done): RStudio-style projects, each in its own window; connect to an Ubuntu server over ssh, with work that keeps running when the app closes.
4. **Agents**: Claude Code and Codex in a terminal pane, MCP tools for live sessions, omp chat for open and local models.
5. **Positron-style panes**: Variables, Data Explorer, Plots, Help, Viewer.
