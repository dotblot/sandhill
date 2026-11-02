# Working on Sandhill

Sandhill is a Mac-native IDE for R/Python bioinformatics. Read `docs/design.md`
before making architectural changes; it records the decisions and why.

## Decisions that are settled
- Mac app: GPUI (Rust, in `gpui/`), Sandhill.app with bundle id
  `dev.sandhill.ide`. It replaced the Tauri 2 app (TypeScript UI in `app/`,
  decided 2026-11-01, retired 2026-11-02; it is in the git history). It keeps
  the Tauri app's look, with every font size 2px larger. No Electron.
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
- `gpui/` is its own Cargo project (`[workspace]` of its own, own
  `Cargo.lock`, pinned `gpui-kit`/`gpui-webview` versions), so a root
  `cargo build` doesn't build it. Check it with `cargo fmt`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` inside `gpui/`. On Linux it
  needs `libxkbcommon-dev libxkbcommon-x11-dev libwebkit2gtk-4.1-dev` and X11.
  The terminal links Ghostty's libghostty-vt, built from source by
  `ghostty-vt-sys`: it needs Zig 0.17 (`ZIG=...` or `zig` on PATH) and
  access to tip.files.ghostty.org, deps.files.ghostty.org and codeberg.org.
  Where those hosts are blocked, `DOCS_RS=1 cargo clippy` still checks the
  code (it skips building the library, so nothing links).
- Rust workspace at the repo root; crates live in `crates/`. The app is not
  part of it (see `gpui/` above).
- Keep crates small and focused; prefer adding a module over growing `main.rs`.
- Errors: `anyhow` in binaries, `thiserror` in libraries.
- Every daemon feature gets an integration test that uses a real kernel.
  Tests skip (with a message) when a kernel isn't installed.
- Output building (`crates/daemon/src/outputs.rs`) and finding Quarto chunks
  and their ids (`crates/daemon/src/qmd.rs`) live in the daemon crate, and
  the app uses them from there. A change gets a case in
  `tests/fixtures/outputs.json` or `tests/fixtures/chunks.json`.
- Icons are drawn once, in `docs/design/ui/icons/build.py`, which writes the
  SVGs and registry (the app embeds them) and the mockup's icon block. The
  crane and the app icon sources come from `docs/design/ui/brand/build.py`;
  then `node gpui/assets/icon/make.mjs`.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` before committing, and the `gpui/` checks above
  when the app changed.
