//! A menu bar in the window: a row of titles, each opening a menu under
//! it, on the same state and panels as every other menu.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, ElementId, Entity, EventEmitter, FocusHandle,
    InteractiveElement as _, IntoElement, MouseButton, OwnedMenu, ParentElement as _, Pixels,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window,
    base::{
        Align, Disableable, ElementExt as _, GlobalState, Selectable as _, TestSupportExt as _,
        actions::{SelectDown, SelectLeft, SelectRight},
        h_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{BAR_CONTEXT, MenuAnchor, MenuEntry, MenuEvent, MenuState, panel::MenuPanels};
use crate::{ActiveTheme as _, Button, ButtonSize};

/// One menu of a [`MenuBar`]: its title and its rows.
#[derive(Clone)]
#[non_exhaustive]
pub struct MenuBarMenu {
    key: SharedString,
    label: SharedString,
    disabled: bool,
    entries: Vec<MenuEntry>,
}

impl MenuBarMenu {
    /// A menu with a stable key and the title it shows.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            disabled: false,
            entries: Vec::new(),
        }
    }

    /// The menu's rows.
    pub fn entries(mut self, entries: impl IntoIterator<Item = impl Into<MenuEntry>>) -> Self {
        self.entries.extend(entries.into_iter().map(Into::into));
        self
    }

    /// The menu's key.
    pub fn key(&self) -> &SharedString {
        &self.key
    }

    /// The title the bar shows.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Whether the title ignores input.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

impl Disableable for MenuBarMenu {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A menu of GPUI's application menu data, keyed by its name, its rows
/// keyed as [`MenuEntry::from_app_menu_item`] keys them, unique within
/// the menu. Items the system owns are left out.
impl From<OwnedMenu> for MenuBarMenu {
    fn from(menu: OwnedMenu) -> Self {
        let mut used = HashSet::new();
        let entries: Vec<MenuEntry> = menu
            .items
            .into_iter()
            .filter_map(|item| MenuEntry::from_app_item_keyed(item, &mut used))
            .collect();
        Self::new(menu.name.clone(), menu.name)
            .disabled(menu.disabled)
            .entries(entries)
    }
}

/// The menus of a [`MenuBar`] and the one open among them.
///
/// Owned by the view that shows the bar, which observes it so a change
/// renders. The open menu is a [`MenuState`], so its keyboard, dismissal,
/// and focus are every menu's: Left and Right at its root move to the
/// neighbouring menu. Items' actions go to the element that had focus
/// before the bar opened, so Edit > Copy reaches the focused field.
pub struct MenuBarState {
    menus: Vec<MenuBarMenu>,
    /// The menu that is open, by index, while the menu state is open.
    open: Option<usize>,
    menu: Entity<MenuState>,
    /// The element focused before the bar opened, which its items act on.
    context: Option<FocusHandle>,
    /// Where each title was last laid out, by key.
    titles: Rc<RefCell<HashMap<SharedString, Bounds<Pixels>>>>,
    /// Each title's focus. Tab reaches the highlighted title only, and the
    /// arrows move focus between titles.
    title_focus: Vec<FocusHandle>,
    /// The title Tab reaches and the arrows move from.
    highlighted: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<MenuEvent> for MenuBarState {}

impl MenuBarState {
    /// A bar over `menus`, all closed.
    pub fn new(
        menus: impl IntoIterator<Item = impl Into<MenuBarMenu>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let menu = cx.new(MenuState::new);
        let this = cx.weak_entity();
        menu.update(cx, |menu, _| {
            menu.set_step_handler(Rc::new(move |delta, window, cx| {
                this.update(cx, |bar, cx| bar.step(delta, window, cx)).ok();
            }))
        });
        let subscriptions = vec![
            cx.observe(&menu, |_, _, cx| cx.notify()),
            cx.subscribe(&menu, |this, menu, event: &MenuEvent, cx| {
                // A switch closes one menu and opens the next in one turn,
                // so the close only counts when the menu stayed closed.
                if *event == MenuEvent::Closed && !menu.read(cx).is_open() {
                    this.open = None;
                    this.context = None;
                }
                cx.emit(event.clone());
                cx.notify();
            }),
        ];
        let menus: Vec<MenuBarMenu> = menus.into_iter().map(Into::into).collect();
        let title_focus = menus.iter().map(|_| cx.focus_handle()).collect();
        Self {
            menus,
            open: None,
            menu,
            context: None,
            titles: Rc::default(),
            title_focus,
            highlighted: 0,
            _subscriptions: subscriptions,
        }
    }

    /// A bar over the application's menus, as given to `App::set_menus`;
    /// empty where the platform keeps none.
    pub fn from_app_menus(cx: &mut Context<Self>) -> Self {
        let menus = cx.get_menus().unwrap_or_default();
        Self::new(menus, cx)
    }

    /// The menus, in bar order.
    pub fn menus(&self) -> &[MenuBarMenu] {
        &self.menus
    }

    /// Replaces the menus, closing the open one.
    pub fn set_menus(
        &mut self,
        menus: impl IntoIterator<Item = impl Into<MenuBarMenu>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
        self.menus = menus.into_iter().map(Into::into).collect();
        self.title_focus = self.menus.iter().map(|_| cx.focus_handle()).collect();
        self.highlighted = self.highlighted.min(self.menus.len().saturating_sub(1));
        cx.notify();
    }

    /// The key of the open menu.
    pub fn open_menu(&self) -> Option<&SharedString> {
        Some(self.menus.get(self.open?)?.key())
    }

    /// Whether a menu is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The open menu's state.
    pub fn menu(&self) -> &Entity<MenuState> {
        &self.menu
    }

    /// Opens the menu with `key` under its title.
    pub fn open(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.menus.iter().position(|menu| menu.key == key) {
            self.open_at(index, false, window, cx);
        }
    }

    /// Closes the open menu and gives focus back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu.update(cx, |menu, cx| menu.close(window, cx));
    }

    /// Opens the menu at `index`, with the keyboard on its first row when
    /// the keyboard opened it.
    fn open_at(
        &mut self,
        index: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = self.menus.get(index).filter(|menu| !menu.disabled) else {
            return;
        };
        if !self.menu.read(cx).is_open() {
            self.context = window.focused(cx);
        }
        let bounds = self
            .titles
            .borrow()
            .get(&menu.key)
            .copied()
            .unwrap_or_default();
        let anchor = MenuAnchor::Trigger(bounds, Align::Start);
        let entries = menu.entries.clone();
        let context = self.context.clone();
        self.menu.update(cx, |menu, cx| {
            match &context {
                Some(context) => menu.open_for(entries, anchor, context, window, cx),
                None => menu.open(entries, anchor, window, cx),
            }
            if keyboard {
                menu.highlight_edge(false, cx);
            }
        });
        self.open = Some(index);
        self.highlighted = index;
        cx.notify();
    }

    /// Moves focus to the next (or previous) title that takes input,
    /// wrapping.
    fn move_highlight(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.neighbour(self.highlighted, delta) {
            self.highlighted = index;
            self.title_focus[index].focus(window, cx);
            cx.notify();
        }
    }

    /// Opens the title the keyboard is on, with the keyboard on its first
    /// row.
    fn open_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_at(self.highlighted, true, window, cx);
    }

    /// The next title from `from` that takes input, `delta` steps at a
    /// time, wrapping; none when every other title is disabled.
    fn neighbour(&self, from: usize, delta: isize) -> Option<usize> {
        let count = self.menus.len() as isize;
        let mut index = from as isize;
        for _ in 0..count {
            index = (index + delta).rem_euclid(count);
            if !self.menus[index as usize].disabled {
                return Some(index as usize);
            }
        }
        None
    }

    /// Moves to the next (or previous) menu that takes input, wrapping.
    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open else {
            return;
        };
        if let Some(index) = self.neighbour(open, delta).filter(|index| *index != open) {
            self.open_at(index, true, window, cx);
        }
    }
}

/// A row of menu titles in the window, as a desktop application's menu
/// bar, for a platform or a layout without the native one.
///
/// A press or a tap on a title opens its menu under it and a second one
/// closes it; while a menu is open, the pointer moving onto another
/// title opens that one instead. The open menu is every menu: arrows,
/// Enter, Escape, submenus, and a press outside behave as in a
/// [`DropdownMenu`](super::DropdownMenu).
///
/// ```
/// use gpui_cn::{MenuBar, MenuBarMenu, MenuBarState, MenuItem};
/// use gpui_kit::{AppContext as _, Context, Entity, IntoElement};
///
/// fn bar(cx: &mut Context<()>) -> Entity<MenuBarState> {
///     cx.new(|cx| {
///         MenuBarState::new(
///             [MenuBarMenu::new("file", "File").entries([MenuItem::new("new", "New File")])],
///             cx,
///         )
///     })
/// }
///
/// fn render(state: &Entity<MenuBarState>) -> impl IntoElement {
///     MenuBar::new("menu-bar", state)
/// }
/// ```
///
/// The id keys the titles and the rows: a title is its menu's key under
/// the id, and the rows are under `"menus"` under the id.
#[derive(IntoElement)]
pub struct MenuBar {
    id: ElementId,
    state: Entity<MenuBarState>,
    plain: bool,
}

impl MenuBar {
    /// A menu bar on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<MenuBarState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            plain: false,
        }
    }

    /// Drops the frame (the fill, the hairline, and the inset) for a bar
    /// in a title bar or a toolbar that gives it a surface already.
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
}

impl RenderOnce for MenuBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (frame_fill, frame_border, frame_radius, touch) = (
            theme.select_trigger,
            theme.select_trigger_border,
            ButtonSize::Default.geometry(theme).radius,
            theme.touch,
        );
        // Not a ghost button's muted rest, which reads as disabled; and
        // not its selected fill, which vanishes on the dark bar.
        let (title_ink, open_fill) = (theme.foreground(), theme.popover_accent);
        let (menus, open, menu, titles, title_focus, highlighted) = {
            let state = self.state.read(cx);
            let open = state.open.filter(|_| state.menu.read(cx).is_open());
            (
                state.menus.clone(),
                open,
                state.menu.clone(),
                state.titles.clone(),
                state.title_focus.clone(),
                state.highlighted,
            )
        };
        let bar_id = self.id.clone();
        let on = |f: fn(&mut MenuBarState, &mut Window, &mut Context<MenuBarState>)| {
            let state = self.state.clone();
            move |window: &mut Window, cx: &mut App| {
                state.update(cx, |state, cx| f(state, window, cx))
            }
        };
        let (left, right, down) = (
            on(|state, window, cx| state.move_highlight(-1, window, cx)),
            on(|state, window, cx| state.move_highlight(1, window, cx)),
            on(|state, window, cx| state.open_highlighted(window, cx)),
        );
        h_flex()
            .id(self.id.clone())
            .test_support()
            .key_context(BAR_CONTEXT)
            .role(Role::MenuBar)
            .when(open.is_none(), |this| {
                this.on_action(move |_: &SelectLeft, window, cx| left(window, cx))
                    .on_action(move |_: &SelectRight, window, cx| right(window, cx))
                    .on_action(move |_: &SelectDown, window, cx| down(window, cx))
            })
            .flex_shrink_0()
            .items_center()
            .when(!self.plain, |this| {
                this.p_1()
                    .rounded(frame_radius)
                    .bg(frame_fill)
                    .border_1()
                    .border_color(frame_border)
            })
            .children(menus.into_iter().enumerate().map(|(index, entry)| {
                let is_open = open == Some(index);
                let disabled = entry.disabled;
                let key = entry.key.clone();
                let titles = titles.clone();
                let press = self.state.clone();
                let hover = self.state.clone();
                let keyboard = self.state.clone();
                // Opens on the press, so a press can drag on into the
                // menu; the click comes on release and acts only for Enter
                // and Space.
                let title = Button::new(ElementId::NamedChild(bar_id.clone().into(), key.clone()))
                    .ghost()
                    .size(ButtonSize::Default)
                    .label(entry.label.clone())
                    .accessibility_label(entry.label)
                    .with_role(Role::MenuItem)
                    .with_expanded(is_open)
                    .open(is_open)
                    .disabled(disabled)
                    .when(!disabled, |this| this.text_color(title_ink))
                    .when(is_open, |this| this.bg(open_fill))
                    .track_focus(&title_focus[index])
                    .tab_stop(index == highlighted)
                    .on_click(move |event, window, cx| {
                        if event.is_keyboard() {
                            keyboard.update(cx, |state, cx| {
                                state.highlighted = index;
                                state.open_at(index, true, window, cx)
                            });
                        }
                    })
                    .when(!disabled, |this| {
                        this.on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            // The press is the title's: focus stays where it
                            // is, so the items act on it.
                            window.prevent_default();
                            cx.stop_propagation();
                            GlobalState::suppress_text_selection(cx);
                            // An open menu's outside press closed it already;
                            // a press on its own title leaves it closed.
                            if open == Some(index) {
                                press.update(cx, |state, cx| state.close(window, cx));
                            } else {
                                press.update(cx, |state, cx| {
                                    state.open_at(index, false, window, cx)
                                });
                            }
                        })
                        .when(!touch, |this| {
                            this.on_mouse_move(move |_, window, cx| {
                                let switch = {
                                    let state = hover.read(cx);
                                    state.menu.read(cx).is_open() && state.open != Some(index)
                                };
                                if switch {
                                    hover.update(cx, |state, cx| {
                                        state.open_at(index, false, window, cx)
                                    });
                                }
                            })
                        })
                    });
                // The title's bounds, which its menu opens under, come from
                // a box around the button, the button's own size.
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(title)
                    .on_prepaint(move |bounds, _, _| {
                        titles.borrow_mut().insert(key, bounds);
                    })
            }))
            .child(MenuPanels::new(
                ElementId::NamedChild(self.id.into(), "menus".into()),
                &menu,
            ))
    }
}
