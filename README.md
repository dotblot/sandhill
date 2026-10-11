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
| `docs/` | Design and protocol documents |

## Roadmap

1. **Server core**: `sandhilld` starts Python and R kernels, runs code, and records every output so a client can disconnect and replay later.
2. **First Mac app loop**: open `.qmd`/`.ipynb`, run a chunk, see inline output, close and reopen with outputs intact.
3. **Remote**: connect to an Ubuntu server over ssh; work keeps running when the app closes.
4. **Agents**: Claude Code and Codex in a terminal pane, MCP tools for live sessions, omp chat for open and local models.
5. **Positron-style panes**: Variables, Data Explorer, Plots, Help, Viewer.
