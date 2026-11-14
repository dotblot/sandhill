use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    StyleRefinement, Styled, Subscription, Task, Window,
    base::{Align, StyledExt as _, h_flex},
    div,
    prelude::FluentBuilder as _,
    px, rems,
};

use crate::{
    ActiveTheme as _, ButtonSize, Icon, Select, SelectEntry, SelectEvent, SelectItem, SelectState,
    TooltipExt as _,
    collapse::{self, Need},
    fuzzy,
    menu::{MenuLook, check_slot, label_block, line_slot},
    middle_text::MiddleText,
};

/// How many options make a [`StatusSelect`] searchable when it is not told
/// either way: three. A list of two, such as a device, is a plain menu; a
/// list of three or more, such as projects and branches, has a search
/// field and ranks what is typed.
pub const SEARCH_MIN_OPTIONS: usize = 3;

/// One choice of a [`StatusSelect`]: an id from the domain, a label, and
/// optionally a leading icon, a line of description in a menu, and a short
/// muted word at the right of a search list row, such as "current" or
/// "remote".
///
/// ```
/// use gpui_cn::StatusOption;
///
/// let option = StatusOption::new("main", "main").description("The default branch");
/// assert_eq!(option.label().as_ref(), "main");
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct StatusOption {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    trailing: Option<SharedString>,
    icon: Option<Icon>,
}

impl StatusOption {
    /// An option with a stable id and the label it shows.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            trailing: None,
            icon: None,
        }
    }

    /// A short muted word at the right of the row in a search list, such
    /// as "current".
    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// An icon before the label in the list.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The word at the right of the row, when there is one.
    pub fn trailing_label(&self) -> Option<&SharedString> {
        self.trailing.as_ref()
    }

    /// A line under the label in the menu.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The id.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// The label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// What a [`StatusSelectState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StatusSelectEvent {
    /// Another option was chosen. The payload is its id.
    Changed(SharedString),
}

/// The options of a status item and the chosen one.
///
/// Owned by the view that shows the item, which observes it so a change
/// renders. The choice is kept by option id.
pub struct StatusSelectState {
    options: Vec<StatusOption>,
    selected: SharedString,
}

impl EventEmitter<StatusSelectEvent> for StatusSelectState {}

impl StatusSelectState {
    /// A state over `options` with the first chosen.
    pub fn new(options: impl IntoIterator<Item = StatusOption>, _: &mut Context<Self>) -> Self {
        let options: Vec<_> = options.into_iter().collect();
        Self {
            selected: options
                .first()
                .map(|option| option.id.clone())
                .unwrap_or_default(),
            options,
        }
    }

    /// Chooses the option with `id`, if there is one.
    pub fn with_selected(mut self, id: impl Into<SharedString>) -> Self {
        let id = id.into();
        if self.options.iter().any(|option| option.id == id) {
            self.selected = id;
        }
        self
    }

    /// The options, in order.
    pub fn options(&self) -> &[StatusOption] {
        &self.options
    }

    /// The id of the chosen option.
    pub fn selected(&self) -> &SharedString {
        &self.selected
    }

    /// The label of the chosen option.
    pub fn selected_label(&self) -> Option<&SharedString> {
        self.options
            .iter()
            .find(|option| option.id == self.selected)
            .map(|option| &option.label)
    }

    /// Replaces the options. The choice stays when its id is still there,
    /// and reports nothing. When it is gone the first option is chosen and
    /// `Changed` is emitted. The menu follows the new options. Whether the
    /// list has a search field is decided when the item is first shown.
    pub fn set_options(
        &mut self,
        options: impl IntoIterator<Item = StatusOption>,
        cx: &mut Context<Self>,
    ) {
        self.options = options.into_iter().collect();
        if !self.options.iter().any(|option| option.id == self.selected) {
            self.selected = self
                .options
                .first()
                .map(|option| option.id.clone())
                .unwrap_or_default();
            cx.emit(StatusSelectEvent::Changed(self.selected.clone()));
        }
        cx.notify();
    }

    /// Chooses the option with `id` and emits `Changed`. An unknown id, or
    /// the one already chosen, changes nothing.
    pub fn select(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = id.into();
        if id == self.selected || !self.options.iter().any(|option| option.id == id) {
            return;
        }
        self.selected = id.clone();
        cx.emit(StatusSelectEvent::Changed(id));
        cx.notify();
    }
}

/// An item of a [`ComposerStatusTab`](crate::ComposerStatusTab) that opens
/// a [`Select`]: a small ghost trigger with an icon, the chosen option's
/// label, and a chevron, over the select's menu with a check on the chosen
/// option. A list of [`SEARCH_MIN_OPTIONS`] options or more has the
/// select's search field, which ranks what is typed fuzzily.
///
/// ```no_run
/// use gpui_cn::{StatusOption, StatusSelect, StatusSelectState};
/// use gpui_kit::{AppContext as _, Context};
/// use gpui_kit::assets::IconName;
///
/// fn branch(cx: &mut Context<()>) -> StatusSelect {
///     let state = cx.new(|cx| {
///         StatusSelectState::new(
///             [StatusOption::new("main", "main"), StatusOption::new("dev", "dev")],
///             cx,
///         )
///     });
///     StatusSelect::new("branch", &state).icon(IconName::GitBranch)
/// }
/// ```
///
/// The default icons (the mic, the shield, the file kinds, the git branch) are
/// not in the default `gpui_kit::assets::Assets`: register
/// [`ComposerAssets`](crate::ComposerAssets).
#[derive(IntoElement)]
#[non_exhaustive]
pub struct StatusSelect {
    id: ElementId,
    state: Entity<StatusSelectState>,
    icon: Option<Icon>,
    searchable: Option<bool>,
    placeholder: SharedString,
    empty: SharedString,
    icon_only: bool,
    width: Option<Pixels>,
    style: StyleRefinement,
}

impl StatusSelect {
    /// An item on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<StatusSelectState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            icon: None,
            searchable: None,
            placeholder: "Search...".into(),
            empty: "No results".into(),
            icon_only: false,
            width: None,
            style: StyleRefinement::default(),
        }
    }

    /// The icon before the label.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Forces the presentation: `true` gives the menu a search field,
    /// `false` a plain list. Left alone, a select with
    /// [`SEARCH_MIN_OPTIONS`] options or more is searchable. It is read
    /// when the item first renders.
    pub fn searchable(mut self, searchable: bool) -> Self {
        self.searchable = Some(searchable);
        self
    }

    /// The hint in the search field. The default is "Search...". It is
    /// read when the item first renders.
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The text shown while a search matches nothing. The default is "No
    /// results".
    pub fn empty(mut self, text: impl Into<SharedString>) -> Self {
        self.empty = text.into();
        self
    }

    /// Shows the icon alone, with the label in a tooltip and no chevron, as
    /// a tab does when it is too narrow for the labels. An item with no
    /// icon ignores it.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }

    /// Cuts the trigger to `width`, which the label ends in an ellipsis to
    /// fit. A tab sets it when the labels do not all fit.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Whether the item opens a menu with a search field, for a list of
    /// `options` options.
    pub fn is_searchable(&self, options: usize) -> bool {
        self.searchable.unwrap_or(options >= SEARCH_MIN_OPTIONS)
    }

    /// What the item needs of a row: its natural width, its width with the
    /// label at the minimum, and its width as an icon alone.
    pub(crate) fn need(&self, window: &Window, cx: &App) -> Need {
        let theme = cx.theme();
        let padding = theme.metrics.control_padding_sm;
        let rem = window.rem_size();
        let gap = rems(0.375).to_pixels(rem);
        let border = px(2.);
        let icon = rems(1.).to_pixels(rem);
        let chevron = rems(1.).to_pixels(rem);
        let label = self
            .state
            .read(cx)
            .selected_label()
            .cloned()
            .unwrap_or_default();
        let natural_label = collapse::text_width(&label, window);
        let kept = natural_label.min(collapse::min_label_width(window, collapse::MIN_LABEL_CHARS));
        let lead = if self.icon.is_some() {
            icon + gap
        } else {
            px(0.)
        };
        let chrome = padding * 2. + border + lead + gap + chevron;
        Need {
            natural: chrome + natural_label,
            min: chrome + kept,
            icon_only: self.icon.is_some().then_some(theme.metrics.control_sm),
        }
    }
}

impl Styled for StatusSelect {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The select an item shows, made once, and the subscription that carries
/// its choice back to the state.
struct Inner {
    select: Entity<SelectState<SharedString>>,
    /// The state's choice the select last agreed with, so a choice made in
    /// the select is not undone before the state hears of it.
    synced: SharedString,
    /// The options the search answers from, shared so it follows changes.
    options: Rc<RefCell<Vec<StatusOption>>>,
    /// The ids and labels the select was last given.
    shown: Vec<(SharedString, SharedString)>,
    _subscription: Subscription,
}

impl RenderOnce for StatusSelect {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let child = |name: &'static str| ElementId::NamedChild(self.id.clone().into(), name.into());
        let (options, selected, label) = {
            let read = self.state.read(cx);
            (
                read.options.clone(),
                read.selected.clone(),
                read.selected_label().cloned().unwrap_or_default(),
            )
        };
        let searchable = self.is_searchable(options.len());
        let by_id: Rc<HashMap<SharedString, StatusOption>> = Rc::new(
            options
                .iter()
                .map(|option| (option.id.clone(), option.clone()))
                .collect(),
        );
        let inner = window.use_keyed_state(child("inner"), cx, {
            let (state, options, selected, placeholder) = (
                self.state.clone(),
                options.clone(),
                selected.clone(),
                self.placeholder.clone(),
            );
            move |window, cx| {
                let shared = Rc::new(RefCell::new(options.clone()));
                let shown: Vec<_> = options
                    .iter()
                    .map(|option| (option.id.clone(), option.label.clone()))
                    .collect();
                let selected_for_inner = selected.clone();
                let entries: Vec<SelectEntry<SharedString>> = options
                    .iter()
                    .map(|option| SelectItem::new(option.id.clone(), option.label.clone()).into())
                    .collect();
                let select = cx.new(|cx| {
                    let select = SelectState::new(entries, cx).with_selected([selected]);
                    if searchable {
                        let options = shared.clone();
                        select.with_search_handler(
                            placeholder,
                            move |query, _| Task::ready(ranked(&options.borrow(), &query)),
                            window,
                            cx,
                        )
                    } else {
                        select
                    }
                });
                let subscription = cx.subscribe(
                    &select,
                    move |inner: &mut Inner, _, event: &SelectEvent<SharedString>, cx| {
                        if let SelectEvent::Changed(values) = event
                            && let Some(value) = values.first()
                        {
                            let value = value.clone();
                            inner.synced = value.clone();
                            state.update(cx, |state, cx| state.select(value, cx));
                        }
                    },
                );
                Inner {
                    select,
                    synced: selected_for_inner,
                    options: shared,
                    shown,
                    _subscription: subscription,
                }
            }
        });
        let select_state = inner.read(cx).select.clone();
        let now: Vec<_> = options
            .iter()
            .map(|option| (option.id.clone(), option.label.clone()))
            .collect();
        if inner.read(cx).shown != now {
            let entries: Vec<SelectEntry<SharedString>> = options
                .iter()
                .map(|option| SelectItem::new(option.id.clone(), option.label.clone()).into())
                .collect();
            *inner.read(cx).options.borrow_mut() = options.clone();
            inner.update(cx, |inner, _| inner.shown = now);
            select_state.update(cx, |state, cx| state.set_entries(entries, cx));
        }
        if inner.read(cx).synced != selected {
            inner.update(cx, |inner, _| inner.synced = selected.clone());
            select_state.update(cx, |state, cx| state.set_selected([selected.clone()], cx));
        }

        let theme = cx.theme();
        let menu_width = theme.metrics.menu_max_width;
        let icon_width = theme.metrics.control_sm;
        let icon_only = self.icon_only && self.icon.is_some();
        let mut select = Select::new(child("select"), &select_state)
            .size(ButtonSize::Sm)
            .ghost()
            .align(Align::Start)
            .accessibility_label(label.clone())
            .empty_text(self.empty.clone())
            .when(searchable, |select| select.menu_width(menu_width))
            .refine_style(&self.style)
            .render_item({
                let by_id = by_id.clone();
                move |item, row, window, cx| option_row(&by_id, item, row, window, cx)
            });
        if icon_only {
            let icon = self.icon.clone().expect("checked above");
            select = select
                .chevron(false)
                .render_value(move |_, _, _| {
                    div().w_full().flex().justify_center().child(icon.clone())
                })
                .w(icon_width);
        } else {
            select = select
                .when_some(self.icon.clone(), |select, icon| select.icon(icon))
                .when_some(self.width, |select, width| select.w(width));
        }
        if icon_only {
            div()
                .id(child("tip"))
                .flex_shrink_0()
                .child(select)
                .managed_tooltip(label, window, &*cx)
                .into_any_element()
        } else {
            select.into_any_element()
        }
    }
}

/// The options a query matches, best first, as the select's rows. An empty
/// query lists every option in order.
fn ranked(options: &[StatusOption], query: &str) -> Vec<SelectEntry<SharedString>> {
    fuzzy::rank(query, options, |option| &option.label)
        .into_iter()
        .map(|found| {
            let option = &options[found.index];
            SelectItem::new(option.id.clone(), option.label.clone()).into()
        })
        .collect()
}

/// The inside of an option's row: its icon, its label cut in the middle to
/// fit, a muted word at the right, and the check, in the select's own row.
fn option_row(
    by_id: &HashMap<SharedString, StatusOption>,
    item: &SelectItem<SharedString>,
    row: crate::SelectRow,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let look = MenuLook::of(cx.theme(), window.rem_size());
    let option = by_id.get(&item.key());
    let caption_text = cx.theme().base.typography.xs;
    let label = item.label().clone();
    let description = option.and_then(|option| option.description.clone());
    h_flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap_2()
        .children(
            option
                .and_then(|option| option.icon.clone())
                .map(|icon| line_slot(&look).child(icon.size_4())),
        )
        .child(match description {
            Some(description) => label_block(label, row.matched(), Some(description), &look),
            None => div()
                .flex_1()
                .min_w_0()
                .child(MiddleText::new(label).marked(row.matched(), look.foreground))
                .into_any_element(),
        })
        .children(
            option
                .and_then(|option| option.trailing.clone())
                .map(|word| {
                    line_slot(&look)
                        .text_size(caption_text.size)
                        .text_color(look.muted_foreground)
                        .child(word)
                }),
        )
        .child(check_slot(&look, row.is_selected(), row.is_disabled()))
        .into_any_element()
}
