//! Menus of commands: a [`DropdownMenu`] under a trigger, a
//! [`ContextMenu`] at the pointer, and the menu a text field opens on a
//! right click. All of them, and a select's menu, draw one panel and one
//! row geometry from [`MenuLook`].
//!
//! The rows are [`MenuEntry`] values given when the menu opens, and the
//! open menu lives in a [`MenuState`].

mod bar;
mod context;
mod dropdown;
mod entry;
mod look;
mod panel;
mod slot;
mod state;
mod text;

use gpui_kit::{
    App, KeyBinding,
    base::actions::{
        Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
    },
};

pub use bar::{MenuBar, MenuBarMenu, MenuBarState};
pub use context::ContextMenu;
pub use dropdown::DropdownMenu;
pub use entry::{MenuEntry, MenuItem, MenuRowState, MenuSubmenu};
pub use state::{MenuAnchor, MenuEvent, MenuState};

pub(crate) use look::{
    MenuLook, MenuMotion, check_slot, label_block, line_slot, matched_text, row_frame, row_line,
    search_row, search_style, separator,
};
pub(crate) use panel::{MenuPanels, corner, placement, shortcut};
pub(crate) use slot::OpenSlot;
pub(crate) use text::{TextMenuBuilder, open_text_menu};

/// The key context of an open menu's root panel, which holds the
/// keyboard for every level.
const CONTEXT: &str = "GpuiCnMenu";

/// The key context of a menu bar, whose titles take the arrows while no
/// menu is open.
const BAR_CONTEXT: &str = "GpuiCnMenuBar";

/// The key context around a dropdown menu's trigger.
const TRIGGER_CONTEXT: &str = "GpuiCnMenuTrigger";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", SelectFirst, Some(CONTEXT)),
        KeyBinding::new("end", SelectLast, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("space", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(TRIGGER_CONTEXT)),
        KeyBinding::new("space", Confirm { secondary: false }, Some(TRIGGER_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(TRIGGER_CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(BAR_CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(BAR_CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(BAR_CONTEXT)),
    ]);
}
