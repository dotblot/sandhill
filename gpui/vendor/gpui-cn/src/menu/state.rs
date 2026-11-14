//! The open menu: its rows, where it opened, the levels of submenus open
//! in it, and the row the keyboard is on in each.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use gpui_kit::{
    App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState,
    Pixels, Point, SharedString, Subscription, Window,
    base::{Align, DeferredPopover, GlobalState, Placement},
};

use super::entry::{MenuEntry, PendingActivation};
use crate::ActiveTheme as _;

/// What a menu opens from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuAnchor {
    /// Under (or, without room, over) a trigger's bounds in window
    /// coordinates, lined up with the given edge of it.
    Trigger(Bounds<Pixels>, Align),
    /// At a point in window coordinates, such as the pointer of a right
    /// click. The menu's top-left corner sits on it, moved into the window
    /// when it would not fit.
    Point(Point<Pixels>),
}

/// What a [`MenuState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuEvent {
    /// The menu opened.
    Opened,
    /// The menu closed, chosen from or dismissed.
    Closed,
    /// An item was chosen. The payload is its key.
    Activated(SharedString),
}

/// One panel of an open menu: the root, or a submenu beside a row of the
/// level before it.
pub(crate) struct Level {
    /// The row of the level before that opened this one; none for the
    /// root.
    parent_row: Option<usize>,
    /// Where that row was, across the width of its panel, which this
    /// level opens beside.
    anchor: Bounds<Pixels>,
    highlighted: Option<usize>,
    list: ListState,
    /// Where the panel was last laid out, so a press on it is not taken
    /// for a press outside the menu.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where each submenu row was last laid out, which its submenu opens
    /// beside.
    submenu_rows: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
}

impl Level {
    fn new(
        parent_row: Option<usize>,
        anchor: Bounds<Pixels>,
        rows: usize,
        overdraw: Pixels,
    ) -> Self {
        Self {
            parent_row,
            anchor,
            highlighted: None,
            // The list lays out a menu's height of rows on its first frame,
            // so every row that shows measures before the panel widens.
            list: ListState::new(rows, ListAlignment::Top, overdraw),
            bounds: Rc::new(Cell::new(Bounds::default())),
            submenu_rows: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub(crate) fn list(&self) -> &ListState {
        &self.list
    }

    pub(crate) fn anchor(&self) -> Bounds<Pixels> {
        self.anchor
    }

    pub(crate) fn bounds(&self) -> &Rc<Cell<Bounds<Pixels>>> {
        &self.bounds
    }

    pub(crate) fn submenu_rows(&self) -> &Rc<RefCell<HashMap<usize, Bounds<Pixels>>>> {
        &self.submenu_rows
    }

    pub(crate) fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }
}

/// A menu that is open: nothing here exists while it is closed, so a
/// closed menu cannot keep a highlight or a stale level.
struct OpenMenu {
    entries: Vec<MenuEntry>,
    anchor: MenuAnchor,
    /// The side of the trigger a trigger menu opened on, decided once
    /// from the room its full height needs.
    placement: Placement,
    /// The root, then each open submenu. Never empty.
    levels: Vec<Level>,
    restore_focus: Option<FocusHandle>,
    action_context: Option<FocusHandle>,
    /// Held while open, so the rest of the application knows a popup is
    /// up, and released with the state if it is dropped open.
    _deferred: DeferredPopover,
    _focus_watch: Subscription,
}

/// The open menu of a [`DropdownMenu`](super::DropdownMenu), a
/// [`ContextMenu`](super::ContextMenu), or a text field.
///
/// Owned by the view that shows the menu, which observes it so a change
/// renders. The rows are given when the menu opens, so what they show
/// (whether Paste has something to paste) is read at that moment.
///
/// While open the menu holds focus. It closes on an outside press, on
/// Escape at its root, when focus leaves it, and when an item is chosen,
/// and then gives focus back to the element that had it before.
pub struct MenuState {
    open: Option<OpenMenu>,
    focus: FocusHandle,
    /// Where a dropdown's trigger was last laid out, which it opens from.
    trigger_bounds: Rc<Cell<Bounds<Pixels>>>,
    step_handler: Option<StepHandler>,
}

/// What Left or Right does when there is no submenu to act on.
pub(crate) type StepHandler = Rc<dyn Fn(isize, &mut Window, &mut App)>;

impl EventEmitter<MenuEvent> for MenuState {}

/// The menu's own focus handle, which holds the keyboard while it is open.
impl Focusable for MenuState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MenuState {
    /// A closed menu.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            open: None,
            focus: cx.focus_handle(),
            trigger_bounds: Rc::new(Cell::new(Bounds::default())),
            step_handler: None,
        }
    }

    /// Whether the menu is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The key of the row the keyboard or the pointer is on, in the
    /// innermost level that has one.
    pub fn highlighted(&self) -> Option<SharedString> {
        let open = self.open.as_ref()?;
        let depth = open.active_depth();
        let row = open.levels[depth].highlighted?;
        open.entries_at(depth)?.get(row)?.key().cloned()
    }

    /// Opens the menu with `entries` at `anchor` and moves focus into it.
    /// An action an item dispatches goes to the element that had focus
    /// before, as its key binding would, and its shortcut is read there;
    /// with nothing focused it goes up from the element the menu is drawn
    /// in. A menu that is open already opens again in the new place.
    pub fn open(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<MenuEntry>>,
        anchor: MenuAnchor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let context = self.restore_target(window, cx);
        if context.is_none() {
            // GPUI reads key bindings from the frame drawn last, which the
            // menu is not in yet; one more frame shows its shortcuts.
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                this.update(cx, |_, cx| cx.notify()).ok();
            });
        }
        self.open_with_context(entries, anchor, context, window, cx);
    }

    /// Replaces the rows of the open menu, as an answer that arrives
    /// after it opened. Submenus close; the keyboard stays on the row
    /// with the same key when there is one. A closed menu ignores it.
    pub fn set_entries(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<MenuEntry>>,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.open else {
            return;
        };
        let highlighted = open.levels[0]
            .highlighted
            .and_then(|row| open.entries.get(row))
            .and_then(|entry| entry.key().cloned());
        open.entries = normalize(entries.into_iter().map(Into::into).collect());
        open.levels.truncate(1);
        let root = &mut open.levels[0];
        root.list.reset(open.entries.len());
        root.highlighted = highlighted.and_then(|key| {
            open.entries
                .iter()
                .position(|entry| entry.key() == Some(&key) && entry.is_selectable())
        });
        if let Some(row) = root.highlighted {
            root.list.scroll_to_reveal_item(row);
        }
        cx.notify();
    }

    /// Opens the menu as [`open`](Self::open) does, with the actions its
    /// items dispatch going to `action_context`, and their shortcuts read
    /// from it.
    pub fn open_for(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<MenuEntry>>,
        anchor: MenuAnchor,
        action_context: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_with_context(entries, anchor, Some(action_context.clone()), window, cx);
    }

    /// Closes the whole menu and gives focus back.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        if let Some(focus) = open.restore_focus {
            focus.focus(window, cx);
        }
        cx.emit(MenuEvent::Closed);
        cx.notify();
    }

    /// The element to give focus back to: the one focused now, unless the
    /// menu is open and has it, when the target stays the one it had.
    fn restore_target(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        match &self.open {
            Some(open) => open.restore_focus.clone(),
            None => window.focused(cx),
        }
    }

    fn open_with_context(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<MenuEntry>>,
        anchor: MenuAnchor,
        action_context: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let restore_focus = self.restore_target(window, cx);
        let entries = normalize(entries.into_iter().map(Into::into).collect());
        let (max_height, gap) = {
            let metrics = &cx.theme().metrics;
            (metrics.menu_max_height, metrics.menu_gap)
        };
        let placement = match anchor {
            MenuAnchor::Trigger(bounds, _) => {
                super::placement(bounds, window.viewport_size().height, max_height, gap)
            }
            MenuAnchor::Point(_) => Placement::Bottom,
        };
        let root = Level::new(None, Bounds::default(), entries.len(), max_height);
        let focus_watch = cx.on_focus_out(&self.focus, window, |this, _, window, cx| {
            this.close(window, cx);
        });
        let was_open = self.open.is_some();
        self.open = Some(OpenMenu {
            entries,
            anchor,
            placement,
            levels: vec![root],
            restore_focus,
            action_context,
            _deferred: GlobalState::register_deferred_popover(cx),
            _focus_watch: focus_watch,
        });
        self.focus.focus(window, cx);
        if !was_open {
            cx.emit(MenuEvent::Opened);
        }
        cx.notify();
    }

    pub(crate) fn trigger_bounds(&self) -> &Rc<Cell<Bounds<Pixels>>> {
        &self.trigger_bounds
    }

    pub(crate) fn anchor(&self) -> Option<(MenuAnchor, Placement)> {
        let open = self.open.as_ref()?;
        Some((open.anchor, open.placement))
    }

    pub(crate) fn levels(&self) -> &[Level] {
        self.open.as_ref().map_or(&[], |open| &open.levels)
    }

    /// The rows of the level at `depth`.
    pub(crate) fn entries_at(&self, depth: usize) -> Option<&[MenuEntry]> {
        self.open.as_ref()?.entries_at(depth)
    }

    /// The row of the level at `depth` whose submenu is open beside it.
    pub(crate) fn open_child_row(&self, depth: usize) -> Option<usize> {
        self.open.as_ref()?.levels.get(depth + 1)?.parent_row
    }

    /// Where the items' actions go: the action context, else the menu
    /// itself, whose path runs up through the element it belongs to.
    pub(crate) fn action_context(&self) -> &FocusHandle {
        self.open
            .as_ref()
            .and_then(|open| open.action_context.as_ref())
            .unwrap_or(&self.focus)
    }

    /// Whether a press at `position` lands on one of the menu's panels.
    pub(crate) fn contains(&self, position: Point<Pixels>) -> bool {
        self.levels()
            .iter()
            .any(|level| level.bounds.get().contains(&position))
    }

    /// The pointer is on `row` of the level at `depth`: it takes the
    /// highlight, deeper levels close, and a submenu row opens its menu.
    pub(crate) fn hover_row(&mut self, depth: usize, row: usize, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else {
            return;
        };
        if !open.is_selectable(depth, row) {
            return;
        }
        let unchanged = open.levels.get(depth).and_then(|level| level.highlighted) == Some(row)
            && (open.levels.len() == depth + 1 || open.levels[depth + 1].parent_row == Some(row));
        if unchanged {
            return;
        }
        open.levels.truncate(depth + 1);
        open.levels[depth].highlighted = Some(row);
        open.open_submenu(depth, row, cx);
        cx.notify();
    }

    /// Moves the highlight one selectable row up or down in the active
    /// level, wrapping at the ends, and closes what was open beside it.
    pub(crate) fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else {
            return;
        };
        let depth = open.active_depth();
        let count = open.entries_at(depth).map_or(0, <[_]>::len);
        if count == 0 {
            return;
        }
        let start = open.levels[depth]
            .highlighted
            .map(|row| row as isize)
            .unwrap_or(if delta > 0 { -1 } else { count as isize });
        let mut row = start;
        for _ in 0..count {
            row = (row + delta).rem_euclid(count as isize);
            if open.is_selectable(depth, row as usize) {
                open.highlight(depth, row as usize);
                cx.notify();
                return;
            }
        }
    }

    /// Highlights the first (or last) selectable row of the active level.
    pub(crate) fn highlight_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else {
            return;
        };
        let depth = open.active_depth();
        let count = open.entries_at(depth).map_or(0, <[_]>::len);
        let mut rows: Box<dyn Iterator<Item = usize>> = if last {
            Box::new((0..count).rev())
        } else {
            Box::new(0..count)
        };
        if let Some(row) = rows.find(|row| open.is_selectable(depth, *row)) {
            open.highlight(depth, row);
            cx.notify();
        }
    }

    /// Opens the highlighted submenu of the active level and puts the
    /// keyboard on its first row.
    pub(crate) fn enter_submenu(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        let depth = open.active_depth();
        let Some(row) = open.levels[depth].highlighted else {
            return false;
        };
        let is_submenu = open
            .entries_at(depth)
            .and_then(|entries| entries.get(row))
            .is_some_and(|entry| entry.submenu().is_some() && entry.is_selectable());
        if !is_submenu {
            return false;
        }
        open.levels.truncate(depth + 1);
        open.open_submenu(depth, row, cx);
        let child = depth + 1;
        if let Some(first) = (0..open.entries_at(child).map_or(0, <[_]>::len))
            .find(|r| open.is_selectable(child, *r))
        {
            open.highlight(child, first);
        }
        cx.notify();
        true
    }

    /// Closes the innermost level the keyboard is in, keeping the root;
    /// false at the root.
    pub(crate) fn leave_submenu(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        let depth = open.active_depth();
        if depth == 0 {
            return false;
        }
        open.levels.truncate(depth);
        cx.notify();
        true
    }

    /// Hands Left or Right that has no submenu to act on to the owner
    /// that set a step handler, such as a menu bar.
    pub(crate) fn set_step_handler(&mut self, handler: StepHandler) {
        self.step_handler = Some(handler);
    }

    pub(crate) fn step(this: &Entity<Self>, delta: isize, window: &mut Window, cx: &mut App) {
        let handler = this.read(cx).step_handler.clone();
        if let Some(handler) = handler {
            handler(delta, window, cx);
        }
    }

    /// Escape: closes the innermost open level, or the whole menu at its
    /// root.
    pub(crate) fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &mut self.open {
            Some(open) if open.levels.len() > 1 => {
                open.levels.pop();
                cx.notify();
            }
            _ => self.close(window, cx),
        }
    }

    /// Chooses `row` of the level at `depth`: a submenu row opens its
    /// menu; an item closes the menu, reports its key, and hands back
    /// what it does, to run once the state is released.
    pub(crate) fn choose(
        &mut self,
        depth: usize,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PendingActivation> {
        let open = self.open.as_mut()?;
        if !open.is_selectable(depth, row) {
            return None;
        }
        let entry = open.entries_at(depth)?.get(row)?.clone();
        match entry {
            MenuEntry::Submenu(_) => {
                if open
                    .levels
                    .get(depth + 1)
                    .and_then(|level| level.parent_row)
                    != Some(row)
                {
                    open.levels.truncate(depth + 1);
                    open.levels[depth].highlighted = Some(row);
                    open.open_submenu(depth, row, cx);
                    cx.notify();
                }
                None
            }
            MenuEntry::Item(item) => {
                let pending = item.pending(
                    open.action_context
                        .clone()
                        .unwrap_or_else(|| self.focus.clone()),
                );
                cx.emit(MenuEvent::Activated(item.key().clone()));
                self.close(window, cx);
                Some(pending)
            }
            MenuEntry::Separator | MenuEntry::Label(_) => None,
        }
    }

    /// Enter or Space: chooses the highlighted row of the active level; a
    /// submenu row opens with the keyboard on its first row.
    pub(crate) fn confirm(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PendingActivation> {
        if self.enter_submenu(cx) {
            return None;
        }
        let open = self.open.as_ref()?;
        let depth = open.active_depth();
        let row = open.levels[depth].highlighted?;
        self.choose(depth, row, window, cx)
    }

    /// Chooses a row as the pointer or the keyboard does, then runs what
    /// the item does once the state is released, so a command that opens
    /// the menu again, or reads it, can.
    pub(crate) fn activate(
        this: &Entity<Self>,
        choose: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> Option<PendingActivation>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pending = this.update(cx, |state, cx| choose(state, window, cx));
        if let Some(pending) = pending {
            pending.run(window, cx);
        }
    }
}

impl OpenMenu {
    /// The rows of the level at `depth`, following each open submenu row.
    fn entries_at(&self, depth: usize) -> Option<&[MenuEntry]> {
        let mut entries = self.entries.as_slice();
        for level in self.levels.get(1..=depth)? {
            entries = entries.get(level.parent_row?)?.submenu()?.children();
        }
        Some(entries)
    }

    /// The level the keyboard is in: the innermost one with a highlight. A
    /// submenu the pointer opened has none until the keyboard enters it.
    fn active_depth(&self) -> usize {
        self.levels
            .iter()
            .rposition(|level| level.highlighted.is_some())
            .unwrap_or(0)
    }

    fn is_selectable(&self, depth: usize, row: usize) -> bool {
        self.entries_at(depth)
            .and_then(|entries| entries.get(row))
            .is_some_and(MenuEntry::is_selectable)
    }

    /// Puts the keyboard on `row` of the level at `depth`, closes what was
    /// open beside it, and scrolls the row into view.
    fn highlight(&mut self, depth: usize, row: usize) {
        self.levels.truncate(depth + 1);
        let level = &mut self.levels[depth];
        level.highlighted = Some(row);
        level.list.scroll_to_reveal_item(row);
    }

    /// Opens the submenu at `row` of the level at `depth`, when the row is
    /// one, beside where the row was last laid out.
    fn open_submenu(&mut self, depth: usize, row: usize, cx: &App) {
        let Some(count) = self
            .entries_at(depth)
            .and_then(|entries| entries.get(row))
            .and_then(MenuEntry::submenu)
            .map(|submenu| submenu.children().len())
        else {
            return;
        };
        let level = &self.levels[depth];
        let row_bounds = level
            .submenu_rows
            .borrow()
            .get(&row)
            .copied()
            .unwrap_or_default();
        let panel = level.bounds.get();
        let anchor = Bounds::from_corners(
            gpui_kit::point(panel.left(), row_bounds.top()),
            gpui_kit::point(panel.right(), row_bounds.bottom()),
        );
        let overdraw = cx.theme().metrics.menu_max_height;
        self.levels
            .push(Level::new(Some(row), anchor, count, overdraw));
    }
}

/// The rows as a menu draws them: no separator at either end, none
/// doubled, and none around a group with nothing to choose, such as a
/// lone label. Submenus are tidied the same way.
pub(crate) fn normalize(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    let mut groups: Vec<Vec<MenuEntry>> = vec![Vec::new()];
    for entry in entries {
        match entry {
            MenuEntry::Separator => groups.push(Vec::new()),
            MenuEntry::Submenu(submenu) => groups
                .last_mut()
                .expect("one group at least")
                .push(MenuEntry::Submenu(submenu.normalized())),
            entry => groups.last_mut().expect("one group at least").push(entry),
        }
    }
    let mut rows = Vec::new();
    for group in groups.into_iter().filter(|group| {
        group
            .iter()
            .any(|entry| matches!(entry, MenuEntry::Item(_) | MenuEntry::Submenu(_)))
    }) {
        if !rows.is_empty() {
            rows.push(MenuEntry::Separator);
        }
        rows.extend(group);
    }
    rows
}
