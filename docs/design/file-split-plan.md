# Splitting the three largest app files

Plan (Huan asked for it on 2026-11-14; not built until approved).

## Why
`gpui/src/workbench.rs` (3,944 lines), `explorer.rs` (3,315) and
`start_screen.rs` (3,023) each hold several unrelated things. They are the
files most often changed, so they keep growing, and every small change means
reading thousands of lines. `CLAUDE.md` asks for small, focused files.

## Rules for this change
- **Moving only.** No behaviour, text, layout or name changes. A function's
  body is the same before and after; only the file it lives in changes.
- **Child modules, as `workbench.rs` already does with `console.rs`.** A file
  under `explorer/` is a child of `explorer.rs`, so it can use the parent's
  private fields and functions. Nothing has to be made more public just to
  be moved.
- Tests move with the code they test.
- One commit per file split, each after `cargo fmt`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` in `gpui/`.
- After all three: open the app once and click through Welcome, a notebook
  run, and the data explorer, in case a move broke something tests miss.

## `explorer.rs` → about 1,150 lines
Keeps: the types, `Explorer`'s state, loading, selection, sorting, saved
views, `Render for Explorer`, the toolbar and the pop-out window.

| New file | Holds | About |
|---|---|---|
| `explorer/grid.rs` | `render_grid`, `column_resizer`, `render_column_menu`, `render_settings_menu`, `menu_item` | 800 |
| `explorer/filters.rs` | `open_filter` … `filter_to`, `render_filter_popup` | 450 |
| `explorer/summary.rs` | `read_summary`, value colours, `render_side`, `spark`, `render_stats`, `render_bar`, `missing_share` and its test | 500 |
| `explorer/export.rs` | `copy_selection`, `copy_code`, `copy_filter_code`, the export functions, `render_export` | 400 |

## `start_screen.rs` → about 1,000 lines
Keeps: `Start` and its state, `render_start`, the sidebar, the title bar,
the small shared helpers and their tests.

| New file | Holds | About |
|---|---|---|
| `start_screen/devices.rs` | connecting and disconnecting, add/remove server, the device menu, renaming, `render_start_dialog` | 750 |
| `start_screen/projects.rs` | `projects_page`, `host_projects`, project rows and their ⋯ menu, stop-kernels and remove questions, `recent_row`, opening from Welcome, the folder chooser's preparation | 1,050 |
| `start_screen/trusted.rs` | the Trusted Folders page and the agent accounts page | 250 |

## `workbench.rs` → about 1,250 lines
Keeps: the types (`Session`, `Run`, `Open`, `DocKind`…), the `Workbench`
struct, `new`, connecting, opening and closing a project, the file tree.

| New file | Holds | About |
|---|---|---|
| `workbench/tabs.rs` | `open_at` … `save_file`: opening, showing and closing document tabs, editors and wrapping, Quarto view, saving; the opening tests | 1,100 |
| `workbench/runs.rs` | `active_session` … `on_event`: which kernel, choosing and remembering it, running cells, statements and scripts, starting, restarting and stopping sessions, kernel events | 950 |
| `workbench/panels.rs` | `refresh_variables` … `fetch_image`: variables, viewer pages, the data explorer's opening and pop-out, the terminal, plots | 550 |

`console.rs` stays where it is (it is already a child module).

## Not in this change
`view.rs`, `plots.rs`, `git_pane.rs`, `crates/proto/src/lib.rs` and
`crates/daemon/src/server.rs` are also large. They are split when next
changed in a big way, by the same rules.

## Risk
A moved function that used a `use` from the top of the old file needs that
`use` in the new one; the compiler finds every such case. Nothing here can
change what the app does without failing to build, except a mistaken edit
during the move, which the per-file checks and the final click-through are
for.
