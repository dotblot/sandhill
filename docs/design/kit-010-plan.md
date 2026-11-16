# Moving the app to GPUI Kit 0.10

Huan, 2026-11-16: approved planning the move after the dependency check.
Status: waiting for Huan's approval.

## What moves

| Package | Now | New |
|---|---|---|
| gpui-kit, gpui-base, gpui-component, gpui-webview, gpui-kit-assets | 0.9.1 | 0.10.0 |
| gpui-fast (all its parts) | 0.1.15 | 0.1.16 |
| gpui-pre | 0.3.12 | 0.3.12 (Kit 0.10 still requires exactly it) |
| tree-sitter | 0.26 | 0.26 (Kit 0.10 still uses it, so Huan's rule holds) |

All released 2026-11-16. Size, from comparing the published sources:
gpui-base changed in 67 files (about 5,500 lines), gpui-component in about
110 files, spread over many components (input, theme, sidebar, button,
table, menu, dialog, dock...).

## Sandhill's own copies

**`app/vendor/gpui-component`** (0.9.1 with our changes, `SANDHILL.md`):

| File | Our change | Upstream 0.10 |
|---|---|---|
| `input/popovers/caret_anchored.rs` | menus open above the caret when they don't fit; stay clear of the title and status bars; documentation panel placed beside the menu | now opens above the caret by itself (our first change, 2026-11-13); the bars and the side panel are still ours |
| `input/popovers/completion_menu.rs` | lettered kind badges, fixed-size documentation panel that can be resized, docs asked on selection, 3 s delay | 4 lines changed |
| `input/popovers/mod.rs` | documentation drawn like the AI pane's replies, 13/12px | unchanged |
| `input/popovers/hover_popover.rs` | interface font, 13px, resizable | 1 line changed |
| `input/mod.rs` | exports for the above | unchanged |

So the copy stays, rebased on 0.10: take 0.10's files, then put our
changes back on top, using upstream's new "open above" code instead of
ours where they overlap. `SANDHILL.md` and AGENTS.md are corrected to list
every change (AGENTS.md still says there is one).

**`app/vendor/gpui-cn`** (the terminal library, 0.8.0 adapted to Kit 0.9):
adapted again to Kit 0.10, keeping its licences and provenance note.

## Steps

1. A branch. Raise the versions in `app/Cargo.toml`; rebase the
   gpui-component copy as above; adapt gpui-cn.
2. Build and fix what Kit 0.10 renamed or changed in the app's own code
   (unknown until it builds; base's 5,500 changed lines suggest some).
3. `cargo fmt`, clippy and the app's tests; the workspace checks are
   unaffected (the daemon doesn't use Kit).
4. Click through the app in a test copy, with screenshots: Welcome and
   its settings, the three columns and pane tabs, the editor with
   completion (badges, documentation panel, opening above the caret at the
   Console), hover popups, notebooks, the terminal, the Viewer (WebView),
   the AI pane (message box, menus, context panel), the status bar, light
   and dark.
5. Update AGENTS.md (stack line, the Fast note goes) and `app/README.md`;
   commit; Huan decides when to push (the Mac build check runs then).

If 0.10 turns out to break something we can't fix well, the branch is
dropped and we stay on 0.9.1; nothing on `main` changes until step 5.

## Questions for Huan

1. Go ahead on a branch as above?
