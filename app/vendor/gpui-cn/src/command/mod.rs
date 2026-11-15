//! A command palette: a search field over a filtered list of commands,
//! with groups, the commands' shortcuts, and keyboard navigation, as
//! shadcn's `Command`.
//!
//! [`CommandState`] holds the commands, the query, and the highlight;
//! [`Command`] draws them.

mod item;

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, KeyBinding, ListAlignment, ListState,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Subscription, Task, Window,
    base::{
        StyledExt as _, TestSupportExt as _,
        actions::{Cancel, Confirm, SelectDown, SelectUp},
        input::{InputEvent, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

use item::Row;
pub use item::{CommandEntry, CommandGroup, CommandItem, CommandRow};

use crate::{
    ActiveTheme as _, ScrollArea, Theme,
    menu::{
        MenuLook, label_block, line_slot, row_frame, row_line, search_row, search_style, separator,
        shortcut,
    },
};

/// What a palette shows while no command matches, as shadcn's demo.
const DEFAULT_EMPTY: &str = "No results.";

/// The key context of a palette, which takes Up, Down, Enter, and Escape
/// the search field passes on.
const CONTEXT: &str = "GpuiCnCommand";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("enter", Confirm { secondary: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

/// A search the application runs itself: given the query, it returns the
/// commands to show, when it has them. See
/// [`CommandState::with_search_handler`].
pub type CommandSearchHandler =
    Rc<dyn Fn(SharedString, &mut Context<CommandState>) -> Task<Vec<CommandEntry>>>;

/// A chosen command's action and the element it goes to.
type Pending = (Box<dyn gpui_kit::Action>, FocusHandle);

/// What a [`CommandState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandEvent {
    /// The query changed. The payload is the new query.
    QueryChanged(SharedString),
    /// A command was chosen, after its action was dispatched. The payload
    /// is its key.
    Confirmed(SharedString),
    /// Enter was pressed while no command matched the query. The payload
    /// is the query, such as a path or a URL to open.
    Submitted(SharedString),
}

/// The commands of a [`Command`] palette, its query, and its highlight.
///
/// Owned by the view that shows the palette, which observes it so a
/// change renders. The query filters the commands by label and keywords
/// as a fuzzy subsequence, ignoring case, and puts the highlight on the first match; Up and Down
/// move it, and Enter chooses it. Choosing a command, or submitting a
/// query, clears the query for the next time.
pub struct CommandState {
    input: Entity<InputState>,
    entries: Vec<CommandEntry>,
    rows: Vec<Row>,
    highlighted: Option<usize>,
    list: ListState,
    handler: Option<CommandSearchHandler>,
    /// The running search. Dropping it cancels it, so a newer query
    /// cancels the search before it and no stale rows arrive.
    search: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CommandEvent> for CommandState {}

/// The search field's focus, which holds the keyboard for the palette.
impl Focusable for CommandState {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl CommandState {
    /// A palette with no commands, whose search field shows `placeholder`.
    pub fn new(
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = search_style(cx.theme());
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(placeholder);
            input.set_editor_style(style);
            input
        });
        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let query = input.read(cx).value();
                this.query_changed(query, cx);
            }
        });
        let overdraw = cx.theme().metrics.menu_max_height;
        Self {
            input,
            entries: Vec::new(),
            rows: Vec::new(),
            highlighted: None,
            list: ListState::new(0, ListAlignment::Top, overdraw),
            handler: None,
            search: None,
            _subscriptions: vec![subscription],
        }
    }

    /// Seeds the commands.
    pub fn with_entries(
        mut self,
        entries: impl IntoIterator<Item = impl Into<CommandEntry>>,
    ) -> Self {
        self.entries = entries.into_iter().map(Into::into).collect();
        self.refilter("");
        self
    }

    /// Answers the query with `handler` in place of the local filter:
    /// for each change, and for the empty query when set, it gets the
    /// query and returns a task with the commands to show. The task can
    /// match on a background thread, fuzzily, or ask a server; a newer
    /// query cancels the task before it. The commands shown stay until
    /// the answer arrives.
    ///
    /// ```
    /// use gpui_cn::{CommandEntry, CommandItem, CommandState};
    /// use gpui_kit::{AppContext as _, Context, Window};
    ///
    /// fn files(window: &mut Window, cx: &mut Context<CommandState>) -> CommandState {
    ///     CommandState::new("Open any file", window, cx).with_search_handler(|query, cx| {
    ///         let paths = cx.background_spawn(async move {
    ///             // Any search: fuzzy, indexed, or a request.
    ///             vec![format!("src/{query}.rs")]
    ///         });
    ///         cx.spawn(async move |_, _| {
    ///             paths
    ///                 .await
    ///                 .into_iter()
    ///                 .map(|path| CommandEntry::from(CommandItem::new(path.clone(), path)))
    ///                 .collect()
    ///         })
    ///     }, cx)
    /// }
    /// ```
    pub fn with_search_handler(
        mut self,
        handler: impl Fn(SharedString, &mut Context<Self>) -> Task<Vec<CommandEntry>> + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        self.handler = Some(Rc::new(handler));
        self.run_search(SharedString::default(), cx);
        self
    }

    /// Whether a search handler is still answering the query.
    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    /// Replaces the commands, as an answer to the query. The highlight
    /// stays on the command with the same key when there is one, and a
    /// search handler's answer still on its way is dropped, so it cannot
    /// replace these commands.
    pub fn set_entries(
        &mut self,
        entries: impl IntoIterator<Item = impl Into<CommandEntry>>,
        cx: &mut Context<Self>,
    ) {
        self.search = None;
        let highlighted = self.highlighted();
        self.entries = entries.into_iter().map(Into::into).collect();
        let query = self.query(cx);
        self.refilter(&query);
        if let Some(key) = highlighted
            && let Some(row) = self.row_of(&key)
        {
            self.highlight(row);
        }
        cx.notify();
    }

    /// The search query.
    pub fn query(&self, cx: &App) -> SharedString {
        self.input.read(cx).value()
    }

    /// Replaces the search query.
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = query.into();
        if self.query(cx) == query {
            return;
        }
        self.input
            .update(cx, |input, cx| input.set_value(query.clone(), window, cx));
        // A value set from code does not report a change, so the rows
        // follow it here.
        self.query_changed(query, cx);
    }

    /// The query changed: the handler answers it, else the local filter.
    fn query_changed(&mut self, query: SharedString, cx: &mut Context<Self>) {
        if self.handler.is_some() {
            self.run_search(query.clone(), cx);
        } else {
            self.refilter(&query);
        }
        cx.emit(CommandEvent::QueryChanged(query));
        cx.notify();
    }

    /// Asks the handler for the commands of `query`, and shows them when
    /// they arrive unless a newer query has replaced the search.
    fn run_search(&mut self, query: SharedString, cx: &mut Context<Self>) {
        let Some(handler) = self.handler.clone() else {
            return;
        };
        let answer = handler(query, cx);
        // The rows answer the query before this one until the new answer
        // arrives, so Enter submits what was typed rather than choose one.
        self.highlighted = None;
        self.search = Some(cx.spawn(async move |this, cx| {
            let entries = answer.await;
            this.update(cx, |this, cx| {
                this.search = None;
                this.entries = entries;
                this.refilter("");
                cx.notify();
            })
            .ok();
        }));
    }

    /// The key of the command the keyboard or the pointer is on.
    pub fn highlighted(&self) -> Option<SharedString> {
        match self.rows.get(self.highlighted?)? {
            Row::Item(item) => Some(item.key().clone()),
            Row::Heading(_) | Row::Separator => None,
        }
    }

    /// How many commands match the query.
    pub fn matched_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, Row::Item(_)))
            .count()
    }

    /// Filters the rows for `query` and puts the highlight on the first
    /// command that can be chosen.
    fn refilter(&mut self, query: &str) {
        // A handler's commands are the answer to the query already.
        let query = if self.handler.is_none() {
            query.trim().to_string()
        } else {
            String::new()
        };
        self.rows = item::rows(&self.entries, &query);
        self.list.reset(self.rows.len());
        self.highlighted = None;
        if let Some(first) = (0..self.rows.len()).find(|row| self.is_selectable(*row)) {
            self.highlight(first);
        }
    }

    fn row_of(&self, key: &SharedString) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            Row::Item(item) => item.key() == key && !item.is_disabled(),
            Row::Heading(_) | Row::Separator => false,
        })
    }

    fn is_selectable(&self, row: usize) -> bool {
        matches!(self.rows.get(row), Some(Row::Item(item)) if !item.is_disabled())
    }

    fn highlight(&mut self, row: usize) {
        self.highlighted = Some(row);
        self.list.scroll_to_reveal_item(row);
    }

    /// Moves the highlight one command up or down, wrapping at the ends.
    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.rows.len() as isize;
        if count == 0 {
            return;
        }
        let mut row = self
            .highlighted
            .map(|row| row as isize)
            .unwrap_or(if delta > 0 { -1 } else { count });
        for _ in 0..count {
            row = (row + delta).rem_euclid(count);
            if self.is_selectable(row as usize) {
                self.highlight(row as usize);
                cx.notify();
                return;
            }
        }
    }

    /// The pointer moved onto `row`.
    fn hover_row(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.is_selectable(row) && self.highlighted != Some(row) {
            self.highlighted = Some(row);
            cx.notify();
        }
    }

    /// Chooses `row`: clears the query for the next time, reports the
    /// command, and hands back its action and where it goes, to dispatch
    /// once the state is released.
    fn choose(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Pending> {
        let Some(Row::Item(item)) = self.rows.get(row).cloned() else {
            return None;
        };
        if item.is_disabled() {
            return None;
        }
        let pending = item.dispatched_action().map(|action| {
            let context = item
                .own_action_context()
                .cloned()
                .unwrap_or_else(|| self.input.read(cx).focus_handle(cx));
            (action.boxed_clone(), context)
        });
        self.set_query("", window, cx);
        cx.emit(CommandEvent::Confirmed(item.key().clone()));
        pending
    }

    /// Enter: chooses the highlighted command, or submits the query when
    /// no command matches it.
    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Pending> {
        match self.highlighted {
            Some(row) => self.choose(row, window, cx),
            None => {
                let query = self.query(cx);
                if !query.trim().is_empty() {
                    self.set_query("", window, cx);
                    cx.emit(CommandEvent::Submitted(query));
                }
                None
            }
        }
    }

    /// Chooses as the pointer or the keyboard does, then dispatches the
    /// command's action once the state is released, so a handler that
    /// reads or updates the palette can.
    fn activate(
        this: &Entity<Self>,
        choose: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> Option<Pending>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((action, context)) = this.update(cx, |state, cx| choose(state, window, cx)) {
            context.dispatch_action(action.as_ref(), window, cx);
        }
    }
}

/// A command palette on a [`CommandState`], as shadcn's `Command`: the
/// search field, a hairline, and the matching commands in a scroll
/// region, drawn with a menu's rows.
///
/// ```
/// use gpui_cn::{Command, CommandItem, CommandState};
/// use gpui_kit::{AppContext as _, Context, Entity, IntoElement, Window};
///
/// fn palette(window: &mut Window, cx: &mut Context<()>) -> Entity<CommandState> {
///     cx.new(|cx| {
///         CommandState::new("Type a command or search...", window, cx)
///             .with_entries([CommandItem::new("calendar", "Calendar")])
///     })
/// }
///
/// fn view(state: &Entity<CommandState>) -> impl IntoElement {
///     Command::new("palette", state)
/// }
/// ```
///
/// The palette draws its own frame; inside a [`Popover`](crate::Popover)
/// or another frame, turn it off with [`bordered`](Self::bordered). The
/// id keys the rows, so it must be stable across frames.
#[derive(IntoElement)]
pub struct Command {
    id: ElementId,
    state: Entity<CommandState>,
    bordered: bool,
    empty: SharedString,
    style: StyleRefinement,
}

impl Command {
    /// A palette on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<CommandState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            bordered: true,
            empty: DEFAULT_EMPTY.into(),
            style: StyleRefinement::default(),
        }
    }

    /// Whether the palette draws its own frame. On by default.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// The text shown while no command matches. The default is shadcn's
    /// "No results."
    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = text.into();
        self
    }
}

impl Styled for Command {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Command {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = MenuLook::of(cx.theme(), window.rem_size());
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let state = self.state;
        let (input, list, empty) = {
            let state = state.read(cx);
            (
                state.input.clone(),
                state.list.clone(),
                state.rows.is_empty(),
            )
        };
        let child = |name: &'static str| ElementId::NamedChild(self.id.clone().into(), name.into());
        let body = if empty {
            div()
                .py_6()
                .text_center()
                .text_color(look.muted_foreground)
                .child(self.empty)
                .into_any_element()
        } else {
            let rows = Rows {
                id: self.id.clone(),
                state: state.clone(),
                look: look.clone(),
                pointer_cursors,
                action_context: input.read(cx).focus_handle(cx),
            };
            ScrollArea::list(
                child("rows"),
                &list,
                look.row_height,
                move |row, window, cx| rows.render(row, window, cx),
            )
            .max_h(look.rows_max_height(true))
            .into_any_element()
        };
        let update = |f: fn(&mut CommandState, &mut Context<CommandState>)| {
            let state = state.clone();
            move |cx: &mut App| state.update(cx, f)
        };
        let (up, down) = (
            update(|state, cx| state.move_highlight(-1, cx)),
            update(|state, cx| state.move_highlight(1, cx)),
        );
        v_flex()
            .id(self.id.clone())
            .test_support()
            .key_context(CONTEXT)
            .min_w(look.search_min_width)
            .max_w(look.max_width)
            .text_size(look.text_size)
            .line_height(look.line_height)
            .text_color(look.foreground)
            .when(self.bordered, |this| {
                this.p(look.padding)
                    .rounded(look.radius)
                    .bg(look.surface)
                    .border_1()
                    .border_color(look.border)
            })
            .refine_style(&self.style)
            .on_action(move |_: &SelectUp, _, cx| up(cx))
            .on_action(move |_: &SelectDown, _, cx| down(cx))
            .on_action({
                let state = state.clone();
                move |_: &Confirm, window, cx| {
                    CommandState::activate(
                        &state,
                        |state, window, cx| state.confirm(window, cx),
                        window,
                        cx,
                    );
                }
            })
            .on_action({
                let state = state.clone();
                // Escape clears a query first; with none it goes on, to
                // the popover or dialog around the palette.
                move |_: &Cancel, window, cx| {
                    if state.read(cx).query(cx).is_empty() {
                        cx.propagate();
                    } else {
                        state.update(cx, |state, cx| state.set_query("", window, cx));
                    }
                }
            })
            .child(
                search_row(&input, &look, window, cx)
                    .id(child("search"))
                    .test_support(),
            )
            .child(separator(&look))
            .child(body)
    }
}

/// What the rows are drawn from, owned by the list's row builder, which
/// outlives the render that made it.
struct Rows {
    id: ElementId,
    state: Entity<CommandState>,
    look: MenuLook,
    pointer_cursors: bool,
    /// Where the rows' actions go when an item names no context of its
    /// own, whose bindings their shortcuts show: the search field.
    action_context: FocusHandle,
}

impl Rows {
    fn render(&self, row: usize, window: &mut Window, cx: &mut App) -> AnyElement {
        let look = &self.look;
        let (entry, highlighted, query) = {
            let state = self.state.read(cx);
            let Some(entry) = state.rows.get(row).cloned() else {
                return div().into_any_element();
            };
            (entry, state.highlighted == Some(row), state.query(cx))
        };
        let context = &self.action_context;
        let item = match entry {
            Row::Separator => return separator(look),
            Row::Heading(text) => {
                return div()
                    .w_full()
                    .h(look.row_height)
                    .px(look.row_padding)
                    .flex()
                    .items_center()
                    .text_color(look.muted_foreground)
                    .child(text)
                    .into_any_element();
            }
            Row::Item(item) => item,
        };
        let disabled = item.is_disabled();
        let foreground = if disabled {
            look.muted_foreground
        } else {
            look.foreground
        };
        let shortcut = item.dispatched_action().and_then(|action| {
            shortcut(action, item.own_action_context().unwrap_or(context), window)
        });
        let row_id = ElementId::NamedChild(self.id.clone().into(), item.key().clone());
        let hover = self.state.clone();
        let choose = self.state.clone();
        let touch = look.touch;
        row_frame(look, highlighted && !disabled)
            .id(row_id)
            .test_support()
            .role(Role::ListBoxOption)
            .aria_label(item.label().clone())
            .when_some(shortcut.clone(), |this, shortcut| {
                this.aria_keyshortcuts(shortcut)
            })
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
            .map(|this| match item.renderer() {
                Some(render) => this.child(render(
                    CommandRow {
                        highlighted,
                        disabled,
                    },
                    window,
                    cx,
                )),
                None => this.child(
                    row_line()
                        .children(item.leading_icon().cloned().map(|icon| {
                            line_slot(look)
                                .w_4()
                                .justify_center()
                                .child(icon.size_4().text_color(foreground))
                        }))
                        .child(div().flex_1().min_w_0().child(label_block(
                            item.label().clone(),
                            &crate::fuzzy::matched(&query, item.label()).unwrap_or_default(),
                            None,
                            look,
                        )))
                        .children(shortcut.map(|shortcut| {
                            line_slot(look)
                                .ml_4()
                                .text_color(look.description)
                                .child(shortcut)
                        })),
                ),
            })
            // A row that scrolls under a resting pointer does not take the
            // keyboard's highlight; a finger has no hover.
            .when(!touch, |this| {
                this.on_mouse_move(move |_, _, cx| {
                    hover.update(cx, |state, cx| state.hover_row(row, cx));
                })
            })
            .on_click(move |_, window, cx| {
                CommandState::activate(
                    &choose,
                    |state, window, cx| state.choose(row, window, cx),
                    window,
                    cx,
                );
            })
            .into_any_element()
    }
}
