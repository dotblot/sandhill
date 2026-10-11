# Sandhill

A fast, low-memory IDE with AI coding agents for R and Python bioinformatics,
built for Apple silicon Macs that work against a remote Ubuntu server (or locally).

Think "Positron without Electron": inline outputs in Jupyter and Quarto that are
saved and come back when you reopen a file, R and Python sessions that keep
running on the server after you close the app, and Claude Code, Codex or open
and local models (through omp) working directly in your live sessions.

> Status: early development. Nothing is usable yet. See the roadmap below.

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
| `crates/daemon` | `sandhilld`: kernel sessions, output journal, socket server |
| `crates/proto` | Wire protocol types and the async client the app and CLI share |
| `crates/cli` | `sandhill`: terminal client, useful for scripting and testing |
| `packaging/` | systemd (server) and launchd (Mac local mode) service files |
| `docs/` | [Design](docs/design.md) and [protocol](docs/protocol.md) |

## Try the daemon

You need Rust, and Jupyter kernels for the languages you want
(`python -m ipykernel install --user`; for R, [Ark](https://github.com/posit-dev/ark) or IRkernel).

```sh
cargo build --release
./target/release/sandhilld serve &                  # or install packaging/systemd/sandhilld.service
./target/release/sandhill kernels
id=$(./target/release/sandhill start python3)
./target/release/sandhill run "$id" 'print("hello")'
./target/release/sandhill history "$id"             # everything that ran, even while you were away
```

Against a server, copy `sandhilld` to `~/.local/bin` there and add
`--ssh yourserver --remote-daemon '~/.local/bin/sandhilld'` to any `sandhill`
command; the daemon starts on first use.

Run the tests with `cargo test`. Kernel tests skip when a kernel isn't installed.

## Roadmap

1. **Server core**: `sandhilld` starts Python and R kernels, runs code, and records every output so a client can disconnect and replay later.
2. **First Mac app loop**: open `.qmd`/`.ipynb`, run a chunk, see inline output, close and reopen with outputs intact.
3. **Remote**: connect to an Ubuntu server over ssh; work keeps running when the app closes.
4. **Agents**: Claude Code and Codex in a terminal pane, MCP tools for live sessions, omp chat for open and local models.
5. **Positron-style panes**: Variables, Data Explorer, Plots, Help, Viewer.
