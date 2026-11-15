//! The panels of an open menu, one per level, and their rows: the part a
//! dropdown menu, a context menu, and a text field's menu share.

use gpui_kit::{
    Action, Anchor, AnyElement, App, Bounds, ElementId, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeyContext, Keystroke, Length, ParentElement as _,
    Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled as _, Window,
    accesskit::Toggled,
    assets::IconName,
    base::{
        Align, ElementExt as _, POPUP_PRIORITY, Placement, Positioner, TestSupportExt as _,
        actions::{
            Cancel, Confirm, SelectDown, SelectFirst, SelectLast, SelectLeft, SelectRight, SelectUp,
        },
        v_flex,
    },
    deferred, div, point,
    prelude::FluentBuilder as _,
    px,
};

use super::{
    CONTEXT, MenuAnchor, MenuEntry, MenuState,
    entry::{Indicator, MenuRowState, RowRenderer},
    look::{MenuLook, MenuMotion, label_block, line_slot, row_frame, row_line, separator},
};
use crate::{ActiveTheme as _, Icon, ScrollArea, Theme};

/// The panels of `state`'s open menu, deferred over the page. Nothing
/// while it is closed.
///
/// Put it inside the element the menu belongs to, so the menu's actions
/// reach that element's handlers. It takes no room in the layout.
#[derive(IntoElement)]
pub(crate) struct MenuPanels {
    id: ElementId,
    state: Entity<MenuState>,
    width: Option<Length>,
}

impl MenuPanels {
    pub(crate) fn new(id: impl Into<ElementId>, state: &Entity<MenuState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            width: None,
        }
    }

    /// A fixed width for the root panel, in place of the width of its rows
    /// between the menu's minimum and maximum.
    pub(crate) fn width(mut self, width: Option<Length>) -> Self {
        self.width = width;
        self
    }
}

impl RenderOnce for MenuPanels {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let holder = div().absolute().size_0();
        let Some((anchor, placement)) = self.state.read(cx).anchor() else {
            return holder;
        };
        let look = MenuLook::of(cx.theme(), window.rem_size());
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let depths = self.state.read(cx).levels().len();
        let mut panels = Vec::with_capacity(depths);
        for depth in 0..depths {
            let Some(panel_id) = panel_id(&self.id, &self.state, depth, cx) else {
                continue;
            };
            // Only the root of a trigger menu slides from its trigger; a
            // menu at the pointer and a submenu beside its row fade.
            let slides = depth == 0 && matches!(anchor, MenuAnchor::Trigger(..));
            let Some(motion) = MenuMotion::sample(
                ElementId::NamedChild(panel_id.clone().into(), "presence".into()),
                true,
                slides,
                &look,
                window,
                cx,
            ) else {
                continue;
            };
            let positioner = if depth == 0 {
                match anchor {
                    MenuAnchor::Trigger(bounds, align) => {
                        let (corner_anchor, position) = corner(placement, align, bounds, look.gap);
                        Positioner::corner(corner_anchor, position)
                    }
                    MenuAnchor::Point(position) => Positioner::corner(Anchor::TopLeft, position),
                }
            } else {
                // The submenu's first row sits inside its border and
                // padding, so the panel starts that far above the row that
                // opened it, to put the two rows level.
                let inset = look.padding + px(1.);
                let mut row = self.state.read(cx).levels()[depth].anchor();
                row.origin.y -= inset;
                row.size.height += inset * 2.;
                Positioner::side(row)
                    .placement(Placement::Right)
                    .align(Align::Start)
                    .offset(look.gap)
            };
            let panel = Panel {
                width: if depth == 0 { self.width } else { None },
                id: self.id.clone(),
                panel_id,
                state: self.state.clone(),
                depth,
                look: look.clone(),
                motion,
                pointer_cursors,
            }
            .render(window, cx);
            panels.push(
                deferred(positioner.occlude().child(panel))
                    .with_priority(POPUP_PRIORITY + 1 + depth),
            );
        }
        holder.children(panels)
    }
}

/// The id of the panel at `depth`: the menu's for the root, the submenu
/// row's for a submenu, so each is keyed by its domain.
fn panel_id(
    id: &ElementId,
    state: &Entity<MenuState>,
    depth: usize,
    cx: &App,
) -> Option<ElementId> {
    if depth == 0 {
        return Some(ElementId::NamedChild(id.clone().into(), "menu".into()));
    }
    let state = state.read(cx);
    let row = state.open_child_row(depth - 1)?;
    let key = state.entries_at(depth - 1)?.get(row)?.key()?.clone();
    Some(ElementId::NamedChild(
        ElementId::NamedChild(id.clone().into(), key).into(),
        "menu".into(),
    ))
}

/// The side of a trigger a menu opens on: below when its full height
/// fits there or below has the more room, else above. Decided once as
/// the menu opens, so a list that shortens never flips it.
pub(crate) fn placement(
    trigger: Bounds<Pixels>,
    viewport_height: Pixels,
    max_height: Pixels,
    gap: Pixels,
) -> Placement {
    let below = viewport_height - trigger.bottom();
    let above = trigger.top();
    if below >= max_height + gap || below >= above {
        Placement::Bottom
    } else {
        Placement::Top
    }
}

/// The menu's corner that touches a trigger, and where it goes: on the
/// side the menu is on, at the edge it lines up with, a gap away.
pub(crate) fn corner(
    placement: Placement,
    align: Align,
    trigger: Bounds<Pixels>,
    gap: Pixels,
) -> (Anchor, gpui_kit::Point<Pixels>) {
    let x = match align {
        Align::Start => trigger.left(),
        Align::Center => trigger.center().x,
        Align::End => trigger.right(),
    };
    match placement {
        Placement::Top => {
            let anchor = match align {
                Align::Start => Anchor::BottomLeft,
                Align::Center => Anchor::BottomCenter,
                Align::End => Anchor::BottomRight,
            };
            (anchor, point(x, trigger.top() - gap))
        }
        _ => {
            let anchor = match align {
                Align::Start => Anchor::TopLeft,
                Align::Center => Anchor::TopCenter,
                Align::End => Anchor::TopRight,
            };
            (anchor, point(x, trigger.bottom() + gap))
        }
    }
}

/// One level of the menu: its panel and its rows.
struct Panel {
    width: Option<Length>,
    id: ElementId,
    panel_id: ElementId,
    state: Entity<MenuState>,
    depth: usize,
    look: MenuLook,
    motion: MenuMotion,
    pointer_cursors: bool,
}

impl Panel {
    fn render(self, _: &mut Window, cx: &mut App) -> AnyElement {
        let Panel {
            width,
            id,
            panel_id,
            state,
            depth,
            look,
            motion,
            pointer_cursors,
        } = self;
        let (list, bounds, rows, focus) = {
            let menu = state.read(cx);
            let level = &menu.levels()[depth];
            let entries = menu.entries_at(depth).unwrap_or_default();
            let leading = entries.iter().any(|entry| match entry {
                MenuEntry::Item(item) => {
                    item.leading_icon().is_some() || item.indicator() != Indicator::None
                }
                MenuEntry::Submenu(submenu) => submenu.leading_icon().is_some(),
                MenuEntry::Separator | MenuEntry::Label(_) => false,
            });
            (
                level.list().clone(),
                level.bounds().clone(),
                Rows {
                    id: id.clone(),
                    state: state.clone(),
                    depth,
                    look: look.clone(),
                    leading,
                    action_context: menu.action_context().clone(),
                    pointer_cursors,
                },
                (depth == 0).then(|| menu.focus_handle(cx)),
            )
        };
        let rows_id = ElementId::NamedChild(panel_id.clone().into(), "rows".into());
        v_flex()
            .id(panel_id)
            .test_support()
            .role(Role::Menu)
            .when_some(focus, |this, focus| keyboard(this, &focus, &state))
            .map(|this| match width {
                Some(width) => this.w(width),
                None => this.min_w(look.min_width).max_w(look.max_width),
            })
            // The fade sits on the panel itself, since GPUI shapes the
            // shadow from the element that carries it.
            .relative()
            .top(motion.offset)
            .opacity(motion.opacity)
            .p(look.padding)
            .rounded(look.radius)
            .bg(look.surface)
            .border_1()
            .border_color(look.border)
            .shadow(look.shadow(motion.shadow_strength))
            .text_size(look.text_size)
            .line_height(look.line_height)
            .text_color(look.foreground)
            // The recorder spans the padding box; the hairline is outside it.
            .on_prepaint(move |panel, _, _| bounds.set(panel.dilate(px(1.))))
            .child(
                ScrollArea::list(rows_id, &list, look.row_height, move |row, window, cx| {
                    rows.render(row, window, cx)
                })
                .max_h(look.rows_max_height(false)),
            )
            .into_any_element()
    }
}

/// The root panel holds the keyboard for every level, and a press outside
/// every panel closes the menu.
fn keyboard<E: StatefulInteractiveElement>(
    panel: E,
    focus: &FocusHandle,
    state: &Entity<MenuState>,
) -> E {
    let update = |f: fn(&mut MenuState, &mut gpui_kit::Context<MenuState>)| {
        let state = state.clone();
        move |cx: &mut App| state.update(cx, f)
    };
    let (up, down, first, last) = (
        update(|state, cx| state.move_highlight(-1, cx)),
        update(|state, cx| state.move_highlight(1, cx)),
        update(|state, cx| state.highlight_edge(false, cx)),
        update(|state, cx| state.highlight_edge(true, cx)),
    );
    // Right opens a submenu and Left closes one; with neither to do, a
    // menu bar moves to the next or the previous menu.
    let (right, left) = (state.clone(), state.clone());
    panel
        .track_focus(focus)
        .key_context(CONTEXT)
        .on_action(move |_: &SelectUp, _, cx| up(cx))
        .on_action(move |_: &SelectDown, _, cx| down(cx))
        .on_action(move |_: &SelectFirst, _, cx| first(cx))
        .on_action(move |_: &SelectLast, _, cx| last(cx))
        .on_action(move |_: &SelectRight, window, cx| {
            if !right.update(cx, |state, cx| state.enter_submenu(cx)) {
                MenuState::step(&right, 1, window, cx);
            }
        })
        .on_action(move |_: &SelectLeft, window, cx| {
            if !left.update(cx, |state, cx| state.leave_submenu(cx)) {
                MenuState::step(&left, -1, window, cx);
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &Confirm, window, cx| {
                MenuState::activate(
                    &state,
                    |state, window, cx| state.confirm(window, cx),
                    window,
                    cx,
                );
            }
        })
        .on_action({
            let state = state.clone();
            move |_: &Cancel, window, cx| {
                state.update(cx, |state, cx| state.dismiss(window, cx));
            }
        })
        .on_mouse_down_out({
            let state = state.clone();
            move |event, window, cx| {
                if !state.read(cx).contains(event.position) {
                    state.update(cx, |state, cx| state.close(window, cx));
                }
            }
        })
}

/// What the rows of one level are drawn from, owned by the list's row
/// builder, which outlives the render that made it.
struct Rows {
    id: ElementId,
    state: Entity<MenuState>,
    depth: usize,
    look: MenuLook,
    /// Whether a row of the level has a check or an icon, so every row
    /// keeps the leading slot.
    leading: bool,
    /// Where the rows' actions go, whose bindings their shortcuts show.
    action_context: FocusHandle,
    pointer_cursors: bool,
}

impl Rows {
    fn render(&self, row: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let look = &self.look;
        let depth = self.depth;
        let (entry, highlighted, expanded, submenu_rows) = {
            let state = self.state.read(cx);
            let Some(entry) = state
                .entries_at(depth)
                .and_then(|entries| entries.get(row))
                .cloned()
            else {
                return div().into_any_element();
            };
            let level = &state.levels()[depth];
            let expanded = state.open_child_row(depth) == Some(row);
            (
                entry,
                level.highlighted() == Some(row) || expanded,
                expanded,
                level.submenu_rows().clone(),
            )
        };
        let row_kind = match &entry {
            MenuEntry::Separator => return separator(look),
            MenuEntry::Label(text) => {
                return div()
                    .w_full()
                    .h(look.row_height)
                    .px(look.row_padding)
                    .flex()
                    .items_center()
                    .text_color(look.muted_foreground)
                    .child(text.clone())
                    .into_any_element();
            }
            MenuEntry::Item(item) => RowKind {
                key: item.key().clone(),
                label: item.label().clone(),
                description: item.description_text().cloned(),
                icon: item.leading_icon().cloned(),
                disabled: item.is_disabled(),
                indicator: item.indicator(),
                destructive: item.is_destructive(),
                link: item.shows_link_icon(),
                renderer: item.renderer().cloned(),
                submenu: false,
            },
            MenuEntry::Submenu(submenu) => RowKind {
                key: submenu.key().clone(),
                label: submenu.label().clone(),
                description: None,
                icon: submenu.leading_icon().cloned(),
                disabled: submenu.is_disabled(),
                indicator: Indicator::None,
                destructive: false,
                link: false,
                renderer: None,
                submenu: true,
            },
        };
        let RowKind {
            key,
            label,
            description,
            icon,
            disabled,
            indicator,
            destructive,
            link,
            renderer,
            submenu: is_submenu,
        } = row_kind;
        let checked = match indicator {
            Indicator::None => None,
            Indicator::Check(on) | Indicator::Radio(on) => Some(on),
        };
        let shortcut = match &entry {
            MenuEntry::Item(item) => item.dispatched_action().and_then(|action| {
                let context = item.own_action_context().unwrap_or(&self.action_context);
                shortcut(action, context, window)
            }),
            _ => None,
        };
        let foreground = if disabled {
            look.muted_foreground
        } else if destructive {
            look.destructive
        } else {
            look.foreground
        };
        let indicator_color = if disabled {
            look.muted_foreground
        } else {
            look.indicator
        };
        let hover_state = self.state.clone();
        let choose_state = self.state.clone();
        let row_id = ElementId::NamedChild(self.id.clone().into(), key);
        let content: Vec<AnyElement> = match renderer {
            Some(render) => vec![render(
                MenuRowState {
                    highlighted,
                    disabled,
                    checked: checked == Some(true),
                },
                window,
                cx,
            )],
            None => {
                let mut parts = Vec::with_capacity(5);
                // Kept on every row once one row of the level has one, so
                // labels line up.
                if self.leading {
                    let slot = line_slot(look)
                        .id(ElementId::NamedChild(
                            row_id.clone().into(),
                            "leading".into(),
                        ))
                        .test_support()
                        .w_4()
                        .justify_center();
                    let slot = match (indicator, icon) {
                        (Indicator::Check(true), _) => slot.child(
                            Icon::from(IconName::Check)
                                .size_4()
                                .text_color(indicator_color),
                        ),
                        (Indicator::Radio(true), _) => {
                            slot.child(div().size_2().rounded_full().bg(indicator_color))
                        }
                        (_, Some(icon)) => slot.child(icon.size_4().text_color(foreground)),
                        _ => slot,
                    };
                    parts.push(slot.into_any_element());
                }
                parts.push(
                    div()
                        .id(ElementId::NamedChild(row_id.clone().into(), "label".into()))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .child(label_block(label.clone(), &[], description, look))
                        .into_any_element(),
                );
                if let Some(shortcut) = shortcut.clone() {
                    parts.push(
                        line_slot(look)
                            .ml_4()
                            .text_color(look.description)
                            .child(shortcut)
                            .into_any_element(),
                    );
                }
                if link {
                    parts.push(
                        line_slot(look)
                            .child(
                                Icon::from(IconName::ExternalLink)
                                    .size_4()
                                    .text_color(indicator_color),
                            )
                            .into_any_element(),
                    );
                }
                if is_submenu {
                    parts.push(
                        line_slot(look)
                            .child(
                                Icon::from(IconName::ChevronRight)
                                    .size_4()
                                    .text_color(look.indicator),
                            )
                            .into_any_element(),
                    );
                }
                vec![row_line().children(parts).into_any_element()]
            }
        };
        row_frame(look, highlighted && !disabled)
            .id(row_id.clone())
            .test_support()
            .role(match indicator {
                Indicator::None => Role::MenuItem,
                Indicator::Check(_) => Role::MenuItemCheckBox,
                Indicator::Radio(_) => Role::MenuItemRadio,
            })
            .aria_label(label)
            .when_some(shortcut, |this, shortcut| this.aria_keyshortcuts(shortcut))
            .when_some(checked, |this, checked| {
                this.aria_toggled(if checked {
                    Toggled::True
                } else {
                    Toggled::False
                })
            })
            .when(is_submenu, |this| this.aria_expanded(expanded))
            .when(highlighted && !disabled, |this| {
                this.aria_active_descendant()
            })
            .text_color(foreground)
            .map(|this| {
                if !disabled && self.pointer_cursors {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .children(content)
            .when(is_submenu, |this| {
                this.on_prepaint(move |bounds, _, _| {
                    submenu_rows.borrow_mut().insert(row, bounds);
                })
            })
            // A pointer that moves over a row highlights it and opens its
            // submenu; a row that scrolls under a resting pointer does not.
            // A finger has no hover: its tap opens and chooses on its own.
            .when(!look.touch, |this| {
                this.on_mouse_move(move |_, _, cx| {
                    hover_state.update(cx, |state, cx| state.hover_row(depth, row, cx));
                })
            })
            .on_click(move |_, window, cx| {
                MenuState::activate(
                    &choose_state,
                    |state, window, cx| state.choose(depth, row, window, cx),
                    window,
                    cx,
                );
            })
            .into_any_element()
    }
}

/// What one item or submenu row shows.
struct RowKind {
    key: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    icon: Option<Icon>,
    disabled: bool,
    indicator: Indicator,
    destructive: bool,
    link: bool,
    renderer: Option<RowRenderer>,
    submenu: bool,
}

/// The shortcut of `action` as the platform writes it, from the bindings
/// that reach `context`, else from those that apply everywhere.
pub(crate) fn shortcut(
    action: &dyn Action,
    context: &FocusHandle,
    window: &Window,
) -> Option<SharedString> {
    let binding = window
        .highest_precedence_binding_for_action_in(action, context)
        .or_else(|| {
            window.highest_precedence_binding_for_action_in_context(action, KeyContext::default())
        })?;
    let text: Vec<String> = binding
        .keystrokes()
        .iter()
        .map(|keystroke| keystroke_text(keystroke.inner()))
        .collect();
    (!text.is_empty()).then(|| text.join(" ").into())
}

/// A keystroke as the platform's menus write it: macOS symbols in the
/// order Control, Option, Shift, Command, and `Ctrl+Shift+X` elsewhere.
fn keystroke_text(keystroke: &Keystroke) -> String {
    let modifiers = &keystroke.modifiers;
    let mut parts: Vec<String> = Vec::with_capacity(5);
    let mac = cfg!(any(target_os = "macos", target_os = "ios"));
    for (on, symbol, name) in [
        (modifiers.control, "\u{2303}", "Ctrl"),
        (modifiers.alt, "\u{2325}", "Alt"),
        (modifiers.shift, "\u{21e7}", "Shift"),
        (
            modifiers.platform,
            "\u{2318}",
            if cfg!(target_os = "windows") {
                "Win"
            } else {
                "Super"
            },
        ),
    ] {
        if on {
            parts.push(if mac { symbol } else { name }.to_string());
        }
    }
    parts.push(key_text(&keystroke.key, mac));
    parts.join(if mac { "" } else { "+" })
}

fn key_text(key: &str, mac: bool) -> String {
    let symbol = match key {
        "enter" => Some(("\u{21a9}", "Enter")),
        "escape" => Some(("\u{238b}", "Esc")),
        "backspace" => Some(("\u{232b}", "Backspace")),
        "delete" => Some(("\u{2326}", "Delete")),
        "tab" => Some(("\u{21e5}", "Tab")),
        "left" => Some(("\u{2190}", "Left")),
        "up" => Some(("\u{2191}", "Up")),
        "right" => Some(("\u{2192}", "Right")),
        "down" => Some(("\u{2193}", "Down")),
        "space" => Some(("Space", "Space")),
        "pageup" => Some(("Page Up", "Page Up")),
        "pagedown" => Some(("Page Down", "Page Down")),
        _ => None,
    };
    if let Some((mac_text, text)) = symbol {
        return if mac { mac_text } else { text }.to_string();
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keystroke_reads_as_the_platform_writes_it() {
        let text = keystroke_text(&Keystroke::parse("cmd-shift-z").unwrap());
        if cfg!(target_os = "macos") {
            assert_eq!(text, "\u{21e7}\u{2318}Z");
        } else if cfg!(target_os = "windows") {
            assert_eq!(text, "Shift+Win+Z");
        } else {
            assert_eq!(text, "Shift+Super+Z");
        }
        let text = keystroke_text(&Keystroke::parse("ctrl-enter").unwrap());
        if cfg!(target_os = "macos") {
            assert_eq!(text, "\u{2303}\u{21a9}");
        } else {
            assert_eq!(text, "Ctrl+Enter");
        }
        assert_eq!(key_text("f5", false), "F5");
    }

    #[test]
    fn the_menu_corner_touches_the_trigger_on_the_chosen_side() {
        let trigger = Bounds::new(point(px(100.), px(50.)), gpui_kit::size(px(80.), px(28.)));
        let (anchor, position) = corner(Placement::Bottom, Align::End, trigger, px(2.));
        assert_eq!(anchor, Anchor::TopRight);
        assert_eq!(position, point(px(180.), px(80.)));
        let (anchor, position) = corner(Placement::Top, Align::Start, trigger, px(2.));
        assert_eq!(anchor, Anchor::BottomLeft);
        assert_eq!(position, point(px(100.), px(48.)));
        let (anchor, position) = corner(Placement::Bottom, Align::Center, trigger, px(2.));
        assert_eq!(anchor, Anchor::TopCenter);
        assert_eq!(position.x, px(140.));
    }
}
