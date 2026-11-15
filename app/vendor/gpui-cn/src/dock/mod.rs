//! Panes that the user rearranges by dragging, as in Ghostty's splits.
//!
//! The layout and its edits are `gpui-base`'s dock: a [`DockArea`] owns a
//! tree of splits and tab groups, and each [`Panel`] is a pane. This
//! module adds the look and the drag through [`DockSkin`], and re-exports
//! the dock types an application needs, so it uses `gpui_cn::dock` only.
//!
//! ```no_run
//! use gpui_cn::dock::{DockLayout, DockSkin};
//! # use gpui_kit::{App, Window};
//! # fn build(left: gpui_kit::Entity<impl gpui_cn::dock::Panel>,
//! #          right: gpui_kit::Entity<impl gpui_cn::dock::Panel>,
//! #          window: &mut Window, cx: &mut App) {
//! let area = DockSkin::area("panes", window, cx);
//! area.update(cx, |area, cx| {
//!     area.set_center(
//!         DockLayout::h_split()
//!             .child(DockLayout::tabs().panel(left), None)
//!             .child(DockLayout::tabs().panel(right), None),
//!         window,
//!         cx,
//!     );
//! });
//! # }
//! ```
//!
//! A pane moves by its layout, never by a resize per frame: when the
//! layout changes, each pane is laid out at its new slot at once and
//! painted gliding from its old rect, clipped to the rect it is passing
//! through. Nothing animates once every pane rests.

pub(crate) mod motion;
mod session;
mod skin;

pub use gpui_kit::base::Placement;
pub use gpui_kit::base::dock::{
    DockArea, DockEvent, DockLayout, DockPlacement, InsertTarget, NodeId, PaneNode, PaneRef,
    PaneTree, Panel, PanelEvent, PanelId, PanelView,
};
pub use skin::DockSkin;
