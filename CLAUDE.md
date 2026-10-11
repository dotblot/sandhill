# Working on Sandhill

Sandhill is a Mac-native IDE for R/Python bioinformatics. Read `docs/design.md`
before making architectural changes; it records the decisions and why.

## Decisions that are settled
- Mac app: Tauri 2 with a TypeScript UI (SolidJS, CodeMirror 6). No Electron.
- The app reaches the daemon only through the wire protocol, by running
  `sandhilld attach --start` (later through ssh). It never opens notebooks
  or kernels itself, so local and remote work the same way.
- Daemon (`sandhilld`): Rust, tokio. Runs on Linux (systemd user service) and
  macOS (launchd agent). Kernels speak the Jupyter protocol (ipykernel for
  Python, Ark for R; IRkernel also works).
- Notebook formats: Jupyter `.ipynb` and Quarto `.qmd`/`.Rmd`. No marimo.
- Every kernel output is journaled by the daemon so clients can disconnect and
  replay. Never make output delivery depend on a client being connected.
- Kernels are started in their own process group and must outlive the daemon.
- Agents: Claude Code and Codex run as unmodified CLIs. Do not read or reuse
  their OAuth tokens. Open/local models go through omp over ACP.
- Positron and Kallichore are Elastic License 2.0: do not copy their code.
  Ark and Air are MIT and may be used.

## Conventions
- Rust workspace at the repo root; crates live in `crates/`. The exception is
  the Tauri app crate, `app/src-tauri`, which sits next to the UI in `app/src`
  as Tauri expects. A plain `cargo build` skips it (it needs WebKitGTK on
  Linux); `--workspace` includes it.
- Keep crates small and focused; prefer adding a module over growing `main.rs`.
- Errors: `anyhow` in binaries, `thiserror` in libraries.
- Every daemon feature gets an integration test that uses a real kernel.
  Tests skip (with a message) when a kernel isn't installed.
- Output building exists twice, in the daemon (`crates/daemon/src/outputs.rs`)
  and the UI (`app/src/model/outputs.ts`). Change both together, with a case
  in `tests/fixtures/outputs.json`, which both test suites read.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` before committing, and `npm test` and
  `npm run build` in `app/` when the UI changed.
