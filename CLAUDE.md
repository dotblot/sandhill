# Working on Sandhill

Sandhill is a Mac-native IDE for R/Python bioinformatics. Read `docs/design.md`
before making architectural changes; it records the decisions and why.

## Who you work for, and how
Huan owns Sandhill and decides what gets built. Huan is new to GitHub words
like PR and CI.
- Talk plainly, with no jargon; explain a term when you must use one.
  Reply in the language Huan writes in (sometimes Chinese).
- No builds of the Mac app unless Huan asks for one.
- No tests or experiments that weren't asked for. When Huan is only asking
  or exploring, answer from what you know instead of running things.
- Before a big change, write a plan and wait for Huan to approve it (Huan
  may change it). Small fixes Huan asked for need no plan.
- Design and building are separate steps; design answers go into the
  design docs below.

## Design docs
- `docs/design.md`: architecture decisions and why (read before changing it).
- `docs/design/bioinfo-ide-framework.md`: the original design for the whole
  IDE: goals, server and kernels, Projects, panes.
- `docs/design/agents-v2-plan.md`: the AI agents plan (chat pane, Claude
  Code and Codex, @ and # mentions), modeled on Posit Assistant.
- `docs/design/gpui-plan.md`: the plan for the move to GPUI.
- `docs/design/ui/`: the UI mockup, spec, icons and brand.
These plans are history: where they differ from this file, this file wins.

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
- Window layout (Huan, 2026-11-02): three columns, 1:2:1. Left: Variables,
  Help, Packages, Kernels in one group; Files, Git, Plots, Viewer in the
  other (the two groups split 1:1). Middle: the editor, with Console,
  Terminal and Jobs under it. Right: the AI pane. Resources show in the
  status bar. The AI pane and the right side pane are shown by default.
- Popups and dropdowns all use one small text size: 11px in the Tauri
  design, so 13px in the app after the +2px rule.
- AI conversations belong to the whole project, not to one file. `@file`
  attaches a file to that one message only; `#` picks a notebook cell
  (and brings its output). No @ symbol is shown in the composer box.
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
