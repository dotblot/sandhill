//! The menu a right click opens on a text field: Cut, Copy, Paste, and
//! Select All, which the field's own actions carry out.

use std::rc::Rc;

use gpui_kit::{
    App, Entity, Pixels, Point, Window,
    base::{
        Disableable as _,
        input::{
            Copy, Cut, InputBaseState, InputContextMenuCapabilities, InputModeKind, Paste,
            SelectAll,
        },
    },
};

use super::{MenuAnchor, MenuEntry, MenuItem, MenuState};

/// Turns a text field's default menu rows into the ones it shows.
pub(crate) type TextMenuBuilder = Rc<
    dyn Fn(Vec<MenuEntry>, InputContextMenuCapabilities, &mut Window, &mut App) -> Vec<MenuEntry>,
>;

/// The rows a text field offers. Cut and Copy need a selection, and a
/// masked field offers neither, so its value never reaches the
/// clipboard. Paste needs text that can change, and text to paste.
pub(crate) fn text_entries(capabilities: InputContextMenuCapabilities, cx: &App) -> Vec<MenuEntry> {
    let editable = capabilities.is_editable();
    let copyable = capabilities.is_copyable();
    let mut entries = Vec::with_capacity(5);
    if !capabilities.is_masked() {
        entries.push(
            MenuItem::new("cut", "Cut")
                .action(Cut)
                .disabled(!(editable && copyable))
                .into(),
        );
        entries.push(
            MenuItem::new("copy", "Copy")
                .action(Copy)
                .disabled(!copyable)
                .into(),
        );
    }
    entries.push(
        MenuItem::new("paste", "Paste")
            .action(Paste)
            .disabled(!(editable && has_text_to_paste(cx)))
            .into(),
    );
    entries.push(MenuEntry::Separator);
    entries.push(
        MenuItem::new("select-all", "Select All")
            .action(SelectAll)
            .disabled(capabilities.is_disabled())
            .into(),
    );
    entries
}

/// Whether the clipboard holds text. iOS shows its paste banner on every
/// read, so there Paste stays on and an empty clipboard pastes nothing.
fn has_text_to_paste(cx: &App) -> bool {
    if cfg!(target_os = "ios") {
        return true;
    }
    cx.read_from_clipboard()
        .is_some_and(|item| item.text().is_some())
}

/// Opens `input`'s menu at `position`: the default rows, passed through
/// `builder` when the field has one. The field takes focus, so the menu
/// gives it back on close, and the rows' actions go to it.
pub(crate) fn open_text_menu<M: InputModeKind>(
    menu: &Entity<MenuState>,
    input: &Entity<InputBaseState<M>>,
    position: Point<Pixels>,
    builder: Option<&TextMenuBuilder>,
    window: &mut Window,
    cx: &mut App,
) {
    let (capabilities, focus) = {
        let input = input.read(cx);
        (
            input.context_menu_capabilities(),
            input.presentation().focus_handle().clone(),
        )
    };
    let mut entries = text_entries(capabilities, cx);
    if let Some(builder) = builder {
        entries = builder(entries, capabilities, window, cx);
    }
    if entries.is_empty() {
        return;
    }
    focus.focus(window, cx);
    menu.update(cx, |menu, cx| {
        menu.open_for(entries, MenuAnchor::Point(position), &focus, window, cx)
    });
}
