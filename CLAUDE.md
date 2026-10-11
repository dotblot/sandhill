# Working on Sandhill

Sandhill is a Mac-native IDE for R/Python bioinformatics. Read `docs/design.md`
before making architectural changes; it records the decisions and why.

## Decisions that are settled
- Mac app: Tauri 2 with a TypeScript UI. No Electron.
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
- Rust workspace at the repo root; crates live in `crates/`.
- Keep crates small and focused; prefer adding a module over growing `main.rs`.
- Errors: `anyhow` in binaries, `thiserror` in libraries.
- Every daemon feature gets an integration test that uses a real kernel.
  Tests skip (with a message) when a kernel isn't installed.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` before committing.
