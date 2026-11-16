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
- Commit as work is done, but push to GitHub only when Huan asks or a
  batch of changes has built up, not after each small change: every push
  runs about 45 minutes of checks (Huan, 2026-11-14).

## Design docs
- `docs/design.md`: architecture decisions and why (read before changing it).
- `docs/design/bioinfo-ide-framework.md`: the original design for the whole
  IDE: goals, server and kernels, Projects, panes.
- `docs/design/agents-v2-plan.md`: the AI agents plan (chat pane, Claude
  Code and Codex, @ and # mentions), modeled on Posit Assistant.
- `docs/design/gpui-plan.md`: the plan for the move to GPUI.
- `docs/design/ui/`: the UI mockup, spec, icons and brand.
- `docs/design/done/`: plans that have been built, kept as history.
These plans are history: where they differ from this file or
`docs/decisions/`, those win.

## Decisions that are settled
Huan's settled decisions are in `docs/decisions/`, one file per subject
(moved there from this file on 2026-11-15). Before changing something,
read the file for its subject; a change that goes against one needs
Huan's approval, and a new decision is written into the right file with
its date.
- `docs/decisions/look.md`: glass, colours, tabs and capsules, popups and
  dropdowns, icons, the crane, fonts, agent marks.
- `docs/decisions/window.md`: the three columns, pane sizes, the title
  bar, windows, Welcome, choosing folders.
- `docs/decisions/editor-and-kernels.md`: the editor, notebook cells and
  outputs, kernels and the Console, Plots, completion, documentation and
  Help, the data explorer.
- `docs/decisions/files.md`: notebook formats, opening every kind of file,
  previews, compress and extract, rendered Markdown.
- `docs/decisions/agents.md`: conversations, the model and effort
  pickers, the agents, accounts and quota.
- `docs/decisions/devices.md`: the daemon, quitting and closing, device
  management, folder trust, journaling.
- `docs/decisions/licences.md`: Sandhill's licence, code from elsewhere,
  notices.

## Conventions
- `app/` is its own Cargo project (`[workspace]` of its own, own
  `Cargo.lock`, pinned `gpui-kit`/`gpui-webview` versions), so a root
  `cargo build` doesn't build it. Check it with `cargo fmt`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` inside `app/`. On Linux it
  needs `libxkbcommon-dev libxkbcommon-x11-dev libwebkit2gtk-4.1-dev` and X11.
  The terminal links Ghostty's libghostty-vt, built from source by
  `ghostty-vt-sys`: it needs Zig 0.17 (`ZIG=...` or `zig` on PATH) and
  access to tip.files.ghostty.org, deps.files.ghostty.org and codeberg.org.
  Where those hosts are blocked, `DOCS_RS=1 cargo clippy` still checks the
  code (it skips building the library, so nothing links).
  Current stack: Kit/Base/Component/WebView 0.9.1, Fast 0.1.15, Pre 0.3.12.
  Fast stays at 0.1.15 with Kit 0.9: 0.1.16 belongs to Kit 0.10 and doesn't
  build with 0.9 (checked 2026-11-16). After a plain `cargo update` in
  `app/`, put every `gpui-fast*` package back to 0.1.15 (`cargo update -p
  <name>@0.1.16 --precise 0.1.15`, repeated until each one takes).
  `app/vendor/gpui-cn` retains the latest terminal library release (0.8.0),
  adapted to Kit 0.9; keep its upstream licences and recorded provenance.
  `app/vendor/gpui-component` is Component 0.9.1 through `[patch.crates-io]`
  with one change (caret menus open above the caret when they don't fit
  below; Huan, 2026-11-13; `SANDHILL.md`); drop it when upstream has it.
  `tree-sitter` stays at 0.26, Kit's own version, until Kit moves (Huan,
  2026-11-13): Cargo allows only one copy of it.
- Rust workspace at the repo root; crates live in `crates/`. The app is not
  part of it (see `app/` above).
- Keep crates small and focused; prefer adding a module over growing `main.rs`.
- Errors: `anyhow` in binaries, `thiserror` in libraries.
- Every daemon feature gets an integration test that uses a real kernel.
  Tests skip (with a message) when a kernel isn't installed.
  Their temporary folders come from `tests/common` (`common::tempdir()`),
  which kills the kernels and test daemons started from a folder before
  deleting it: kernels outlive their daemon by design, and plain
  `tempfile` folders left 183 of them running (Huan, 2026-11-14).
- Output building (`crates/daemon/src/outputs.rs`) and finding Quarto chunks
  and their ids (`crates/daemon/src/qmd.rs`) live in the daemon crate, and
  the app uses them from there. A change gets a case in
  `tests/fixtures/outputs.json` or `tests/fixtures/chunks.json`.
- Icons are generated by `docs/design/ui/icons/build.py` from `catalog.json`,
  the two source maps and pinned original SVGs under `vendor/`. Change those
  inputs, not generated SVGs; the app embeds the output, and the mockup uses
  the same art. The crane and app icon sources remain in
  `docs/design/ui/brand/build.py`; then `node app/assets/icon/make.mjs`.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` before committing, and the `app/` checks above
  when the app changed.
- The version stays 0.1.0 until Huan judges Sandhill stable, with no
  obvious bugs (2026-11-13). It is written in `Cargo.toml`,
  `app/Cargo.toml` and the bundle `Info.plist` in
  `.github/workflows/mac-app.yml`; change all three together.
