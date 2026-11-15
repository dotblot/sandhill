# Sandhill's copy of GPUI Component 0.9.1

Source: the crates.io release `gpui-component` 0.9.1
(<https://github.com/longbridge/gpui-kit>), Apache-2.0 (`LICENSE-APACHE`),
used through `[patch.crates-io]` in `gpui/Cargo.toml` (Huan, 2026-11-13).

## Sandhill changes

- `src/input/popovers/caret_anchored.rs`: caret-anchored menus (completion,
  code actions) open above the caret when they don't fit below it and there
  is more room above. The Console input sits at the bottom of the window, so
  its completion list was cut off by the window's edge. Menus also stay clear
  of the bands `set_popup_insets` names (Sandhill's title and status bars;
  2026-11-14). A side panel (the completion documentation) is placed after
  the menu, which is placed by its own size: right of it, else left, else
  under (over, when it opened upward), inside those bands.
- `src/input/popovers/completion_menu.rs` (2026-11-14): a lettered,
  coloured badge before each item says its kind. The documentation is a
  side panel of one size for every item (scrolling inside), so browsing
  never moves the list or the panel; its corner drags to resize it for the
  session. Clicks in it don't dismiss the menu. (2026-11-15) An item
  selected without documentation asks the editor's `CompletionDocs`
  (`set_completion_docs`) after 120 ms and shows the answer. The panel
  itself shows only 3 s after the last new items (typing), or at once
  where `show_completion_docs_now` (Tab) asked.
- `src/input/popovers/mod.rs` (2026-11-14): documentation renders as the AI
  pane's replies do, prose in the interface font and code blocks in the
  code font; fences that name no language take the editor's. (2026-11-15)
  Prose is 13px and code 12px.
- `src/input/popovers/hover_popover.rs` (2026-11-15): the hover popup
  sets the interface font (it inherited the editor's code font) and the
  popups' 13px, and its corner away from the text drags to resize it for
  the session.

Everything else is unchanged. Return to the crates.io release once upstream
offers the same, and remove the patch.
