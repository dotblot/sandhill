//! The rows, the selection, the query, and the open menu of a select.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    ListAlignment, ListState, Pixels, SharedString, Subscription, Task, Window,
    base::{
        DeferredPopover, GlobalState, Placement,
        input::{InputEvent, InputState},
    },
    px,
};

use super::item::{self, SelectEntry, SelectItem, SelectValue};
use crate::{ActiveTheme as _, MenuState, menu::search_style};

/// What a [`SelectState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectEvent<V> {
    /// The selection changed. The payload is every selected value, in
    /// the order chosen; one or none for a single select.
    Changed(Vec<V>),
    /// The menu opened or closed.
    OpenChanged(bool),
}

/// A search the application runs itself: given the query, it returns the
/// rows to show, when it has them. See
/// [`SelectState::with_search_handler`].
pub type SearchHandler<V> =
    Rc<dyn Fn(SharedString, &mut Context<SelectState<V>>) -> Task<Vec<SelectEntry<V>>>>;

/// The search field at the top of the menu, and the handler that answers
/// its query when the application does rather than the local filter.
struct SearchField<V: SelectValue> {
    input: Entity<InputState>,
    handler: Option<SearchHandler<V>>,
    /// The query the rows answer to. The field reports a change for an
    /// edit that leaves its text as it was, such as an Enter; the rows
    /// answer only to a query that differs.
    query: SharedString,
    /// The field's right-click menu. The select's own popup blocks base's
    /// hook for it, so the menu opens from the search row.
    menu: Entity<MenuState>,
}

/// The rows, the selection, and the open menu of a [`Select`](super::Select).
///
/// Owned by the view that shows the select, which observes it so a change
/// renders. Edit the rows and the selection through its methods; the
/// menu's own state (the query, the highlighted row, the scroll position)
/// is the state's to keep.
///
/// The selection is kept as items, not values alone, so the trigger can
/// show a chosen item's label after a search has replaced the rows.
pub struct SelectState<V: SelectValue> {
    entries: Vec<SelectEntry<V>>,
    selected: Vec<SelectItem<V>>,
    multiple: bool,
    open: bool,
    /// The row the keyboard is on: an index into `visible`.
    highlighted: Option<usize>,
    /// The rows that show for the query: indices into `entries`.
    visible: Vec<usize>,
    search: Option<SearchField<V>>,
    searching: bool,
    /// The running search. Dropping it cancels it, so a new query cancels
    /// the search before it and no stale rows arrive.
    search_task: Option<Task<()>>,
    list: ListState,
    /// The height every row is given, when one is fixed; the rows are
    /// otherwise as tall as their content, with the plain row's height as
    /// the hint for rows not yet measured.
    row_height: Option<Pixels>,
    /// Which side of the trigger the menu opened on, decided once per
    /// opening from the room the menu's full height needs, so a search
    /// that shortens the rows does not swing the menu to the other side.
    menu_placement: Option<Placement>,
    trigger_focus: FocusHandle,
    content_focus: FocusHandle,
    /// Where the trigger was last laid out, which the menu opens from.
    trigger_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Held while open, so the rest of the application knows a popup is
    /// up, and released with the state if it is dropped open.
    deferred: Option<DeferredPopover>,
    focus_watch: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl<V: SelectValue> EventEmitter<SelectEvent<V>> for SelectState<V> {}

/// The trigger's focus handle: focusing the state focuses the trigger.
impl<V: SelectValue> Focusable for SelectState<V> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}

impl<V: SelectValue> SelectState<V> {
    /// A closed, single select over `entries` with nothing selected.
    pub fn new(
        entries: impl IntoIterator<Item = impl Into<SelectEntry<V>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_multiple(false, entries, cx)
    }

    /// A closed select over `entries` in which more than one item can be
    /// selected: the menu stays open while items are toggled, and the
    /// trigger counts the selection.
    pub fn multiple(
        entries: impl IntoIterator<Item = impl Into<SelectEntry<V>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_multiple(true, entries, cx)
    }

    fn with_multiple(
        multiple: bool,
        entries: impl IntoIterator<Item = impl Into<SelectEntry<V>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let entries: Vec<SelectEntry<V>> = entries.into_iter().map(Into::into).collect();
        let visible = item::visible_entries(&entries, "");
        // The list sizes itself from the rows it lays out, and on its
        // first frame it lays out only one screen of overdraw. With none,
        // a menu opened one row wide and widened on the next frame; with
        // a menu's height of overdraw, every row that can show measures
        // on the first frame.
        let overdraw = cx.theme().metrics.menu_max_height;
        let list = ListState::new(visible.len(), ListAlignment::Top, overdraw);
        Self {
            entries,
            selected: Vec::new(),
            multiple,
            open: false,
            highlighted: None,
            visible,
            search: None,
            searching: false,
            search_task: None,
            list,
            row_height: None,
            menu_placement: None,
            trigger_focus: cx.focus_handle(),
            content_focus: cx.focus_handle(),
            trigger_bounds: Rc::new(Cell::new(Bounds::default())),
            deferred: None,
            focus_watch: Vec::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// Adds a search field at the top of the menu, with `placeholder` in it
    /// while it is empty. Typing narrows the rows to those whose label or
    /// keywords the query matches as a fuzzy subsequence, ignoring case.
    pub fn with_search(
        self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.with_search_field(placeholder, None, window, cx)
    }

    /// Adds a search field whose query the application answers: for each
    /// change, and once when the menu opens, `handler` gets the query and
    /// returns a task with the rows to show. The task can do its work on
    /// a background thread or over the network; a newer query cancels the
    /// task before it. The rows given to the state are the ones shown
    /// until the first answer, and a chosen item stays selected when a
    /// later answer no longer lists it.
    ///
    /// The state keeps no answers: what makes one stale is the
    /// application's to know. A handler that remembers its answers can
    /// return `Task::ready` for a query it has seen, and the menu shows
    /// those rows on the next frame.
    ///
    /// ```
    /// use gpui_cn::{SelectEntry, SelectItem, SelectState};
    /// use gpui_kit::{AppContext as _, Context, Task, Window};
    ///
    /// fn cities(window: &mut Window, cx: &mut Context<SelectState<usize>>) -> SelectState<usize> {
    ///     SelectState::new([SelectItem::new(1, "Zurich")], cx).with_search_handler(
    ///         "Search cities",
    ///         |query, cx| {
    ///             cx.background_spawn(async move {
    ///                 // Any search: fuzzy, indexed, or a request.
    ///                 vec![SelectEntry::from(SelectItem::new(2, format!("{query} City")))]
    ///             })
    ///         },
    ///         window,
    ///         cx,
    ///     )
    /// }
    /// ```
    pub fn with_search_handler(
        self,
        placeholder: impl Into<SharedString>,
        handler: impl Fn(SharedString, &mut Context<Self>) -> Task<Vec<SelectEntry<V>>> + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.with_search_field(placeholder, Some(Rc::new(handler)), window, cx)
    }

    fn with_search_field(
        mut self,
        placeholder: impl Into<SharedString>,
        handler: Option<SearchHandler<V>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = search_style(cx.theme());
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(placeholder);
            input.set_editor_style(style);
            input
        });
        self._subscriptions
            .push(cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                let query = input.read(cx).value();
                if let Some(search) = &mut this.search
                    && search.query != query
                {
                    search.query = query;
                    this.query_changed(cx);
                }
            }));
        let menu = cx.new(MenuState::new);
        self._subscriptions
            .push(cx.observe(&menu, |_, _, cx| cx.notify()));
        self.search = Some(SearchField {
            input,
            handler,
            query: SharedString::default(),
            menu,
        });
        self
    }

    /// Seeds the selection. Values that are not items are dropped, and a
    /// single select keeps the first.
    pub fn with_selected(mut self, values: impl IntoIterator<Item = V>) -> Self {
        self.selected = self.items_for(values);
        self
    }

    /// Gives every item row this height. Without it a row is as tall as
    /// its content, a plain row's height at least, and the list measures
    /// rows as they come into view. With it the list knows every row's
    /// height before it is laid out, so a scroll to a far row of a long
    /// menu lands exactly; rows drawn by
    /// [`Select::render_item`](super::Select::render_item) clip to it.
    pub fn with_row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    /// The rows, in menu order.
    pub fn entries(&self) -> &[SelectEntry<V>] {
        &self.entries
    }

    /// Replaces the rows. With a local search, a selected value that is
    /// no longer an item is dropped from the selection; with a handler
    /// the rows come and go with the query, so the selection stays.
    pub fn set_entries(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<SelectEntry<V>>>,
        cx: &mut Context<Self>,
    ) {
        self.entries = entries.into_iter().map(Into::into).collect();
        if self.search_handler().is_none() {
            let selected = self.items_for(self.selected.iter().map(|item| item.value().clone()));
            if !same_values(&selected, &self.selected) {
                self.selected = selected;
                cx.emit(SelectEvent::Changed(self.selected()));
            }
        }
        self.rows_changed(cx);
    }

    /// The selected values, in the order chosen.
    pub fn selected(&self) -> Vec<V> {
        self.selected
            .iter()
            .map(|item| item.value().clone())
            .collect()
    }

    /// The selected value of a single select.
    pub fn selected_value(&self) -> Option<&V> {
        self.selected.first().map(SelectItem::value)
    }

    /// The selected items, in the order chosen.
    pub fn selected_items(&self) -> &[SelectItem<V>] {
        &self.selected
    }

    /// Whether `value` is selected.
    pub fn is_selected(&self, value: &V) -> bool {
        self.selected.iter().any(|item| item.value() == value)
    }

    /// Replaces the selection and reports the change. Values that are not
    /// items are dropped, and a single select keeps the first.
    pub fn set_selected(&mut self, values: impl IntoIterator<Item = V>, cx: &mut Context<Self>) {
        let selected = self.items_for(values);
        if same_values(&selected, &self.selected) {
            return;
        }
        self.selected = selected;
        cx.emit(SelectEvent::Changed(self.selected()));
        cx.notify();
    }

    /// Whether more than one item can be selected.
    pub fn is_multiple(&self) -> bool {
        self.multiple
    }

    /// Whether the menu has a search field.
    pub fn is_searchable(&self) -> bool {
        self.search.is_some()
    }

    /// Whether a search handler is still answering the query.
    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// Whether the menu is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The search query, or an empty string without a search field.
    pub fn query(&self, cx: &App) -> SharedString {
        self.search
            .as_ref()
            .map(|search| search.input.read(cx).value())
            .unwrap_or_default()
    }

    /// The item the keyboard is on while the menu is open.
    pub fn highlighted(&self) -> Option<&SelectItem<V>> {
        self.entry_at(self.highlighted?)?.item()
    }

    /// Opens the menu, highlights the selected item (or the first), decides
    /// which side of the trigger the menu takes, and moves focus into the
    /// menu: to the search field, else to the list. A search handler is
    /// asked for the rows of the empty query.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return;
        }
        self.open = true;
        self.deferred = Some(GlobalState::register_deferred_popover(cx));
        // Below when the menu's full height fits, else above when that
        // fits, else whichever side has more room; and kept while open,
        // so a search that shortens the rows never flips the menu.
        let (max_height, gap) = {
            let metrics = &cx.theme().metrics;
            (metrics.menu_max_height, metrics.menu_gap)
        };
        self.menu_placement = Some(crate::menu::placement(
            self.trigger_bounds.get(),
            window.viewport_size().height,
            max_height,
            gap,
        ));
        if let Some(search) = &self.search {
            let style = search_style(cx.theme());
            search
                .input
                .update(cx, |input, _| input.set_editor_style(style));
        }
        self.rows_changed(cx);
        // A scroll to the highlighted row needs the list's height, which
        // it has only once it has been laid out: on the first opening it
        // has not been, so the row is revealed again after that frame.
        if self.list.viewport_bounds().size.height <= px(0.) {
            let this = cx.weak_entity();
            window.on_next_frame(move |_, cx| {
                this.update(cx, |this, cx| {
                    this.reveal_highlighted();
                    cx.notify();
                })
                .ok();
            });
        }
        self.focus_content(window, cx);
        // Focus that leaves the menu for anywhere but the trigger closes
        // it: a Tab out, a click on a field. Focus that returns to the
        // trigger is a close already under way. The list and the search
        // field each hold focus on their own (GPUI does not see the field
        // inside the list's node), so both are watched.
        let mut watched = vec![self.content_focus.clone()];
        if let Some(search) = &self.search {
            watched.push(search.input.read(cx).focus_handle(cx));
        }
        self.focus_watch = watched
            .iter()
            .map(|handle| {
                cx.on_focus_out(handle, window, |this, _, window, cx| {
                    if this.open && !this.owns_focus(window, cx) {
                        this.close(window, cx);
                    }
                })
            })
            .collect();
        if self.search_handler().is_some() {
            self.run_search(cx);
        }
        cx.emit(SelectEvent::OpenChanged(true));
        cx.notify();
    }

    /// Closes the menu, clears the query, cancels a running search, and
    /// returns focus to the trigger if the menu had it.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.deferred = None;
        self.menu_placement = None;
        self.search_task = None;
        self.searching = false;
        self.focus_watch.clear();
        let had_focus = self.owns_focus(window, cx);
        // The query clears for the next opening, which refreshes the rows.
        if let Some(search) = &mut self.search {
            search.menu.update(cx, |menu, cx| menu.close(window, cx));
            search.query = SharedString::default();
            if !search.input.read(cx).value().is_empty() {
                search
                    .input
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
        }
        if had_focus {
            self.trigger_focus.focus(window, cx);
        }
        cx.emit(SelectEvent::OpenChanged(false));
        cx.notify();
    }

    /// Chooses `value` while the menu is open: a single select selects it
    /// and closes, a multiple select toggles it and stays open. A
    /// disabled item, or a closed menu, ignores it.
    pub fn choose(&mut self, value: &V, window: &mut Window, cx: &mut Context<Self>) {
        // A press that reaches a row of a closed menu chooses nothing.
        if !self.open {
            return;
        }
        let Some(item) = self.item(value).cloned() else {
            return;
        };
        if item.is_disabled() {
            return;
        }
        let mut selected = if self.multiple {
            self.selected.clone()
        } else {
            Vec::new()
        };
        match selected
            .iter()
            .position(|selected| selected.value() == value)
        {
            Some(position) if self.multiple => {
                selected.remove(position);
            }
            Some(_) => {}
            None => selected.push(item),
        }
        if !same_values(&selected, &self.selected) {
            self.selected = selected;
            cx.emit(SelectEvent::Changed(self.selected()));
            cx.notify();
        }
        if !self.multiple {
            self.close(window, cx);
        }
    }

    /// Chooses the highlighted item. While a handler is still answering
    /// there is no row to choose: the highlight is on rows the answer
    /// will replace.
    pub fn choose_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.searching {
            return;
        }
        if let Some(value) = self.highlighted().map(|item| item.value().clone()) {
            self.choose(&value, window, cx);
        }
    }

    /// Moves the highlight one selectable row up or down, wrapping at the
    /// ends, and scrolls it into view.
    pub fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.visible.len();
        if count == 0 {
            return;
        }
        let start = self
            .highlighted
            .map(|row| row as isize)
            .unwrap_or(if delta > 0 { -1 } else { count as isize });
        let mut row = start;
        for _ in 0..count {
            row = (row + delta).rem_euclid(count as isize);
            if self.is_selectable(row as usize) {
                self.highlighted = Some(row as usize);
                self.reveal_highlighted();
                cx.notify();
                return;
            }
        }
    }

    /// Highlights the first selectable row.
    pub fn highlight_first(&mut self, cx: &mut Context<Self>) {
        self.highlighted = self.first_selectable();
        self.reveal_highlighted();
        cx.notify();
    }

    /// Highlights the last selectable row.
    pub fn highlight_last(&mut self, cx: &mut Context<Self>) {
        self.highlighted = (0..self.visible.len())
            .rev()
            .find(|row| self.is_selectable(*row));
        self.reveal_highlighted();
        cx.notify();
    }

    /// Highlights the row at `row` from the pointer, without scrolling.
    pub(super) fn highlight_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.highlighted != Some(row) && self.is_selectable(row) {
            self.highlighted = Some(row);
            cx.notify();
        }
    }

    pub(super) fn list(&self) -> &ListState {
        &self.list
    }

    pub(super) fn row_count(&self) -> usize {
        self.visible.len()
    }

    /// The row at `row` of the rows that show.
    pub(super) fn entry_at(&self, row: usize) -> Option<&SelectEntry<V>> {
        self.entries.get(*self.visible.get(row)?)
    }

    pub(super) fn is_highlighted(&self, row: usize) -> bool {
        self.highlighted == Some(row)
    }

    pub(super) fn row_height(&self) -> Option<Pixels> {
        self.row_height
    }

    /// The height a row not yet measured is taken to have.
    pub(super) fn row_hint(&self, cx: &App) -> Pixels {
        self.row_height.unwrap_or(cx.theme().metrics.row_sm)
    }

    pub(super) fn menu_placement(&self) -> Option<Placement> {
        self.menu_placement
    }

    pub(super) fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// The menu's focus handle: the list's, which holds the rows.
    pub(super) fn content_focus(&self) -> &FocusHandle {
        &self.content_focus
    }

    pub(super) fn search_input(&self) -> Option<&Entity<InputState>> {
        self.search.as_ref().map(|search| &search.input)
    }

    /// The search field's right-click menu.
    pub(super) fn search_menu(&self) -> Option<&Entity<MenuState>> {
        self.search.as_ref().map(|search| &search.menu)
    }

    /// Where the trigger records its bounds each frame.
    pub(super) fn trigger_bounds(&self) -> &Rc<Cell<Bounds<Pixels>>> {
        &self.trigger_bounds
    }

    /// The focus handle base moves keyboard focus to while the menu is
    /// open.
    pub(super) fn content_focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.search {
            Some(search) => search.input.read(cx).focus_handle(cx),
            None => self.content_focus.clone(),
        }
    }

    fn search_handler(&self) -> Option<&SearchHandler<V>> {
        self.search.as_ref()?.handler.as_ref()
    }

    /// The query changed while the menu is open: a local search narrows
    /// the rows now, a handler is asked for new ones.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        if self.search_handler().is_some() {
            self.run_search(cx);
        } else {
            self.rows_changed(cx);
        }
    }

    /// Asks the handler for the rows of the current query, and shows them
    /// when they arrive unless a newer query has replaced the search.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let Some(handler) = self.search_handler().cloned() else {
            return;
        };
        let rows = handler(self.query(cx), cx);
        self.searching = true;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let rows = rows.await;
            this.update(cx, |this, cx| {
                this.searching = false;
                this.search_task = None;
                this.entries = rows;
                this.rows_changed(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// The rows to show changed, by a query, an answer, or a new set:
    /// filter them, size the list, and put the keyboard
    /// on the selected item while the query is empty, else on the first
    /// row that matches.
    fn rows_changed(&mut self, cx: &mut Context<Self>) {
        // A handler's rows are the answer to the query already.
        let query = if self.search_handler().is_some() {
            SharedString::default()
        } else {
            self.query(cx)
        };
        self.visible = item::visible_entries(&self.entries, &query);
        let hint = self.row_hint(cx);
        self.list
            .reset_with_uniform_height(self.visible.len(), hint);
        let selected = self
            .query(cx)
            .is_empty()
            .then(|| self.row_of(self.selected.first().map(SelectItem::value)))
            .flatten();
        self.highlighted = selected.or_else(|| self.first_selectable());
        self.reveal_highlighted();
        cx.notify();
    }

    fn item(&self, value: &V) -> Option<&SelectItem<V>> {
        self.entries
            .iter()
            .filter_map(SelectEntry::item)
            .find(|item| item.value() == value)
    }

    /// The items for `values`, in the order given, once each, dropping
    /// values that are not items; one for a single select.
    fn items_for(&self, values: impl IntoIterator<Item = V>) -> Vec<SelectItem<V>> {
        let mut selected: Vec<SelectItem<V>> = Vec::new();
        for value in values {
            if selected.iter().any(|item| item.value() == &value) {
                continue;
            }
            if let Some(item) = self.item(&value) {
                selected.push(item.clone());
            }
        }
        if !self.multiple {
            selected.truncate(1);
        }
        selected
    }

    fn is_selectable(&self, row: usize) -> bool {
        self.entry_at(row)
            .and_then(SelectEntry::item)
            .is_some_and(|item| !item.is_disabled())
    }

    fn first_selectable(&self) -> Option<usize> {
        (0..self.visible.len()).find(|row| self.is_selectable(*row))
    }

    fn row_of(&self, value: Option<&V>) -> Option<usize> {
        let value = value?;
        self.visible.iter().position(|index| {
            self.entries[*index]
                .item()
                .is_some_and(|item| item.value() == value)
        })
    }

    fn reveal_highlighted(&self) {
        if let Some(row) = self.highlighted {
            self.list.scroll_to_reveal_item(row);
        }
    }

    /// Whether the list, the search field, or the search field's menu is
    /// focused.
    fn owns_focus(&self, window: &Window, cx: &App) -> bool {
        let Some(focused) = window.focused(cx) else {
            return false;
        };
        focused == self.content_focus
            || self.search.as_ref().is_some_and(|search| {
                search.input.read(cx).focus_handle(cx) == focused || search.menu.read(cx).is_open()
            })
    }

    fn focus_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.search {
            Some(search) => search.input.update(cx, |input, cx| input.focus(window, cx)),
            None => self.content_focus.focus(window, cx),
        }
    }
}

fn same_values<V: SelectValue>(a: &[SelectItem<V>], b: &[SelectItem<V>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.value() == b.value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Kind {
        All,
        Local,
        Cloud,
        Max,
    }

    impl SelectValue for Kind {
        fn key(&self) -> SharedString {
            format!("{self:?}").to_lowercase().into()
        }
    }

    fn entries() -> Vec<SelectEntry<Kind>> {
        vec![
            SelectEntry::label("Type"),
            SelectItem::new(Kind::All, "All chats").into(),
            SelectItem::new(Kind::Local, "Local").disabled(true).into(),
            SelectItem::new(Kind::Cloud, "Cloud").into(),
            SelectEntry::Separator,
            SelectItem::new(Kind::Max, "Max").into(),
        ]
    }

    fn init(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(cx);
        });
    }

    #[gpui_kit::test]
    fn a_single_select_keeps_one_value_and_a_multiple_select_toggles(cx: &mut TestAppContext) {
        init(cx);
        let single =
            cx.new(|cx| SelectState::new(entries(), cx).with_selected([Kind::Cloud, Kind::Max]));
        single.read_with(cx, |state, _| {
            assert_eq!(state.selected(), [Kind::Cloud]);
            assert_eq!(state.selected_value(), Some(&Kind::Cloud));
            assert_eq!(state.selected_items()[0].key(), "cloud");
            assert!(!state.is_multiple());
        });
        let multiple = cx.new(|cx| {
            SelectState::multiple(entries(), cx).with_selected([Kind::Max, Kind::All, Kind::All])
        });
        multiple.read_with(cx, |state, _| {
            assert_eq!(state.selected(), [Kind::Max, Kind::All], "as chosen, once");
            assert!(state.is_selected(&Kind::Max));
            assert!(!state.is_selected(&Kind::Cloud));
            assert!(state.is_multiple());
        });
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&multiple, move |_, event: &SelectEvent<Kind>, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        });
        multiple.update(cx, |state, cx| {
            state.set_selected([Kind::Cloud], cx);
            state.set_selected([Kind::Cloud], cx);
        });
        assert_eq!(
            &*events.borrow(),
            &[SelectEvent::Changed(vec![Kind::Cloud])],
            "the same selection again reports nothing"
        );
    }

    #[gpui_kit::test]
    fn the_highlight_skips_disabled_rows_and_labels_and_wraps(cx: &mut TestAppContext) {
        init(cx);
        let state = cx.new(|cx| SelectState::new(entries(), cx));
        state.update(cx, |state, cx| {
            let highlighted = |state: &SelectState<Kind>| state.highlighted().map(|i| *i.value());
            state.highlight_first(cx);
            assert_eq!(highlighted(state), Some(Kind::All));
            state.move_highlight(1, cx);
            assert_eq!(highlighted(state), Some(Kind::Cloud), "Local is disabled");
            state.move_highlight(1, cx);
            assert_eq!(highlighted(state), Some(Kind::Max));
            state.move_highlight(1, cx);
            assert_eq!(highlighted(state), Some(Kind::All), "wraps");
            state.highlight_last(cx);
            assert_eq!(highlighted(state), Some(Kind::Max));
            state.move_highlight(-1, cx);
            assert_eq!(highlighted(state), Some(Kind::Cloud));
        });
    }

    #[gpui_kit::test]
    fn replacing_the_rows_drops_a_selection_that_is_gone(cx: &mut TestAppContext) {
        init(cx);
        let state = cx.new(|cx| SelectState::new(entries(), cx).with_selected([Kind::Max]));
        state.update(cx, |state, cx| {
            state.set_entries([SelectItem::new(Kind::All, "All chats")], cx);
            assert!(state.selected().is_empty());
            assert_eq!(state.entries().len(), 1);
        });
    }

    #[test]
    fn the_search_field_paints_in_the_menu_colors() {
        use crate::theme::{ThemeConfig, to_hex};
        use gpui_kit::base::ThemeAppearance;
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let style = search_style(&theme);
        assert_eq!(to_hex(style.foreground), "#ffffff");
        assert_eq!(style.caret, style.foreground);
        assert_eq!(style.selection, theme.selection());
        assert!(style.selection.a > 0.);
    }
}
