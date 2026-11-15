//! A browser-style tab strip: a bar of tabs whose selected tab merges into
//! the content below it, with a close control, a new-tab control, and
//! scroll controls when the tabs overflow.
//!
//! The look is measured from the tab reference app. [`TabsState`] owns the
//! tabs, the selection, and the scroll position; [`Tabs`] renders it.
//!
//! ```
//! use gpui_cn::{Tab, Tabs, TabsState};
//! use gpui_kit::{Entity, IntoElement};
//!
//! fn strip(state: &Entity<TabsState>) -> impl IntoElement {
//!     Tabs::new("tabs", state)
//! }
//!
//! fn tabs() -> TabsState {
//!     TabsState::new([Tab::new("home", "Home"), Tab::new("logs", "Logs")])
//! }
//! ```

use std::{fmt, rc::Rc, sync::Arc};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DispatchPhase, Div, ElementId, Entity, EventEmitter,
    HitboxBehavior, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    ParentElement as _, Pixels, RenderOnce, ScrollEvent, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Subscription, Window,
    assets::IconName,
    base::{
        self, Disableable as _, ElementExt as _, Interpolate as _, Sequence, StyledExt as _,
        TestSupportExt as _, h_flex, transition,
    },
    canvas, div, point,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    ActiveTheme as _, Button, ButtonSize, Icon, Input, InputEvent, InputState, MenuAnchor,
    MenuEntry, MenuState, Theme, ThemeTokens, TooltipHost, menu::MenuPanels,
    tooltip::TooltipTrigger,
};
/// One tab in the strip. The id is the application's key for the tab and
/// must be unique in the strip; it never comes from a list index.
#[derive(Clone)]
pub struct Tab {
    id: SharedString,
    label: SharedString,
    icon: Option<Icon>,
}

impl Tab {
    /// A tab with a stable id and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
        }
    }

    /// An icon before the label.
    pub fn with_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The application's key for the tab.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// The visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The icon before the label, if any.
    pub fn icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }
}

impl fmt::Debug for Tab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tab")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("icon", &self.icon.as_ref().map(Icon::source))
            .finish()
    }
}

/// Emitted by [`TabsState`] after it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TabsEvent {
    /// The selected tab changed. The payload is the tab id.
    Selected(SharedString),
    /// A tab was removed by its close control or by
    /// [`remove`](TabsState::remove). The payload is its id.
    Closed(SharedString),
    /// The new-tab control was activated. The application decides what a
    /// new tab is and calls [`push`](TabsState::push).
    AddRequested,
    /// A tab's label changed, by [`rename`](TabsState::rename) or by an
    /// inline rename the user committed. The payload is its id.
    Renamed(SharedString),
    /// A tab's close control was activated on a strip built with
    /// [`confirm_close`](Tabs::confirm_close). The tab stays until the
    /// application calls [`remove`](TabsState::remove). The payload is its
    /// id.
    CloseRequested(SharedString),
}

/// The tabs, which one is selected, and the scroll position of the strip.
///
/// Edit it through its methods so the strip observes the change. Every
/// method leaves the state whole before it emits, so a subscriber reads
/// the tabs and the selection as they will render.
pub struct TabsState {
    tabs: Vec<Tab>,
    selected: Option<SharedString>,
    scroll: ScrollHandle,
    glide: Option<Glide>,
    /// Whether the selected tab must be brought into view. Set by a
    /// selection, cleared by the strip once it has asked the scroll for it.
    reveal: bool,
    /// Tabs pushed since the last frame settled, which grow into place.
    entering: Vec<SharedString>,
    /// Tabs removed but still shrinking out of the strip, at the index
    /// they held.
    leaving: Vec<Leaving>,
    /// The tab whose label is a text field, while the user renames it.
    renaming: Option<Renaming>,
}

/// An inline rename under way: the tab and the field its label became.
struct Renaming {
    id: SharedString,
    input: Entity<InputState>,
    _subscription: Subscription,
}

/// A removed tab on its way out. It is no longer in [`TabsState::tabs`]
/// and takes no input; the strip draws it until its width reaches zero.
#[derive(Clone, Debug)]
struct Leaving {
    tab: Tab,
    index: usize,
}

/// A scroll the controls asked for, which the strip plays as motion from
/// the offset it had to the one it wants. The serial tells one glide from
/// the next, so each starts where the strip is and not where the last
/// glide ended.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    serial: usize,
    from: Pixels,
    to: Pixels,
}

impl EventEmitter<TabsEvent> for TabsState {}

impl TabsState {
    /// The tabs, with the first one selected. Ids must be unique.
    pub fn new(tabs: impl IntoIterator<Item = Tab>) -> Self {
        let tabs: Vec<Tab> = tabs.into_iter().collect();
        debug_assert!(unique_ids(&tabs), "tab ids must be unique");
        let selected = tabs.first().map(|tab| tab.id.clone());
        Self {
            tabs,
            selected,
            scroll: ScrollHandle::new(),
            glide: None,
            reveal: false,
            entering: Vec::new(),
            leaving: Vec::new(),
            renaming: None,
        }
    }

    /// The tabs, in strip order.
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The id of the selected tab.
    pub fn selected(&self) -> Option<&SharedString> {
        self.selected.as_ref()
    }

    fn index_of(&self, id: &SharedString) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.id == id)
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|id| self.index_of(id))
    }

    /// Selects the tab with `id` and scrolls it into view. A no-op for an
    /// unknown id or the tab already selected.
    pub fn select(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = id.into();
        let Some(index) = self.index_of(&id) else {
            return;
        };
        if self.selected.as_ref() == Some(&id) {
            return;
        }
        self.select_index(index);
        cx.emit(TabsEvent::Selected(id));
        cx.notify();
    }

    /// Selects the tab `step` places from the selected one, wrapping at
    /// the ends: `-1` for the previous tab, `1` for the next.
    pub fn select_neighbor(&mut self, step: isize, cx: &mut Context<Self>) {
        let len = self.tabs.len();
        if len == 0 {
            return;
        }
        let current = self.selected_index().unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(len as isize) as usize;
        let id = self.tabs[next].id.clone();
        self.select(id, cx);
    }

    /// Selects the first tab, or the last one when `last` is set.
    pub fn select_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(tab) = (if last {
            self.tabs.last()
        } else {
            self.tabs.first()
        }) else {
            return;
        };
        let id = tab.id.clone();
        self.select(id, cx);
    }

    fn select_index(&mut self, index: usize) {
        self.selected = Some(self.tabs[index].id.clone());
        self.reveal = true;
    }

    fn revealed(&mut self, cx: &mut Context<Self>) {
        if self.reveal {
            self.reveal = false;
            cx.notify();
        }
    }

    /// Appends a tab and selects it. Its id must be new to the strip.
    pub fn push(&mut self, tab: Tab, cx: &mut Context<Self>) {
        debug_assert!(self.index_of(&tab.id).is_none(), "tab ids must be unique");
        let id = tab.id.clone();
        self.leaving.retain(|leaving| leaving.tab.id != id);
        self.tabs.push(tab);
        self.entering.push(id.clone());
        self.select_index(self.tabs.len() - 1);
        cx.emit(TabsEvent::Selected(id));
        cx.notify();
    }

    /// Inserts a tab at `index`, clamped to the end, without selecting it,
    /// so a host that keeps its own order can put a tab where it belongs.
    /// Its id must be new to the strip.
    pub fn insert(&mut self, index: usize, tab: Tab, cx: &mut Context<Self>) {
        debug_assert!(self.index_of(&tab.id).is_none(), "tab ids must be unique");
        let index = index.min(self.tabs.len());
        let id = tab.id.clone();
        self.leaving.retain(|leaving| leaving.tab.id != id);
        self.tabs.insert(index, tab);
        self.entering.push(id);
        cx.notify();
    }

    /// Removes the tab with `id`. If it was selected, the tab after it is
    /// selected, or the one before it when it was last. Emits `Closed`,
    /// then `Selected` when the selection moved.
    pub fn remove(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = id.into();
        let Some(index) = self.index_of(&id) else {
            return;
        };
        let was_selected = self.selected.as_ref() == Some(&id);
        let tab = self.tabs.remove(index);
        self.entering.retain(|entering| entering != &id);
        self.leaving.push(Leaving { tab, index });
        let next = if was_selected {
            self.selected = None;
            next_selection(index, self.tabs.len())
        } else {
            None
        };
        if let Some(next) = next {
            self.select_index(next);
        }
        cx.emit(TabsEvent::Closed(id));
        if let Some(next) = next {
            cx.emit(TabsEvent::Selected(self.tabs[next].id.clone()));
        }
        cx.notify();
    }

    /// Sets or clears the icon of the tab with `id`, such as to mark a
    /// state the tab is in. A no-op for an unknown id.
    pub fn set_icon(
        &mut self,
        id: impl Into<SharedString>,
        icon: Option<impl Into<Icon>>,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        if let Some(index) = self.index_of(&id) {
            self.tabs[index].icon = icon.map(Into::into);
            cx.notify();
        }
    }

    /// Asks the application for a new tab.
    pub fn request_add(&mut self, cx: &mut Context<Self>) {
        cx.emit(TabsEvent::AddRequested);
    }

    /// Sets the label of the tab with `id` and emits `Renamed`. A no-op
    /// for an unknown id or an unchanged label.
    pub fn rename(
        &mut self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        let label = label.into();
        let Some(index) = self.index_of(&id) else {
            return;
        };
        if self.tabs[index].label == label {
            return;
        }
        self.tabs[index].label = label;
        cx.emit(TabsEvent::Renamed(id));
        cx.notify();
    }

    /// Turns the label of the tab with `id` into a text field holding the
    /// label, selected and focused. Enter or leaving the field commits a
    /// non-empty name; Escape cancels. A strip that is
    /// [`renamable`](Tabs::renamable) starts one on a double click.
    pub fn start_rename(
        &mut self,
        id: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = id.into();
        let Some(index) = self.index_of(&id) else {
            return;
        };
        let label = self.tabs[index].label.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(label));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_rename(cx);
            }
        });
        self.renaming = Some(Renaming {
            id,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Ends an inline rename and keeps the typed name, unless it is blank.
    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(renaming) = self.renaming.take() else {
            return;
        };
        let value = renaming.input.read(cx).value();
        let name = value.trim();
        if !name.is_empty() {
            self.rename(renaming.id, name.to_owned(), cx);
        }
        cx.notify();
    }

    /// Ends an inline rename and keeps the old label.
    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            cx.notify();
        }
    }

    /// The id of the tab being renamed inline, if any.
    pub fn renaming(&self) -> Option<&SharedString> {
        self.renaming.as_ref().map(|renaming| &renaming.id)
    }

    /// The strip reports that the tab's enter or exit motion ended: an
    /// entered tab is an ordinary tab from now on, and a left tab is gone.
    fn settle(&mut self, id: &SharedString, cx: &mut Context<Self>) {
        let before = (self.entering.len(), self.leaving.len());
        self.entering.retain(|entering| entering != id);
        self.leaving.retain(|leaving| &leaving.tab.id != id);
        if before != (self.entering.len(), self.leaving.len()) {
            cx.notify();
        }
    }

    /// Glides the strip by `delta`, clamped to the content. A positive
    /// delta shows tabs further right.
    fn scroll_by(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        let from = self.scroll.offset().x;
        self.glide_to(from, from - delta, cx);
    }

    /// Glides the strip by a mouse wheel step. Steps that arrive while a
    /// glide is under way stack on its end, so a spinning wheel keeps one
    /// smooth run.
    fn wheel_by(&mut self, step: Pixels, cx: &mut Context<Self>) {
        let from = self.scroll.offset().x;
        let base = match self.glide {
            Some(glide)
                if (glide.from..=glide.to).contains(&from)
                    || (glide.to..=glide.from).contains(&from) =>
            {
                glide.to
            }
            _ => from,
        };
        self.glide_to(from, base + step, cx);
    }

    /// Moves the strip by a trackpad or touch step at once, clamped to
    /// the content, and ends any glide.
    fn pan_by(&mut self, step: Pixels, cx: &mut Context<Self>) {
        let now = self.scroll.offset();
        let max = self.scroll.max_offset().x;
        let to = (now.x + step).clamp(-max, px(0.));
        self.glide = None;
        if to != now.x {
            self.scroll.set_offset(point(to, now.y));
            cx.notify();
        }
    }

    fn glide_to(&mut self, from: Pixels, to: Pixels, cx: &mut Context<Self>) {
        let max = self.scroll.max_offset().x;
        let to = to.clamp(-max, px(0.));
        if to != from {
            let serial = self.glide.map_or(0, |glide| glide.serial + 1);
            self.glide = Some(Glide { serial, from, to });
            cx.notify();
        }
    }
}

fn unique_ids(tabs: &[Tab]) -> bool {
    tabs.iter()
        .enumerate()
        .all(|(index, tab)| !tabs[..index].iter().any(|other| other.id == tab.id))
}

/// The index to select after the tab at `removed` left a strip that now
/// holds `len` tabs: the tab that took its place, else the one before it.
fn next_selection(removed: usize, len: usize) -> Option<usize> {
    if len == 0 {
        None
    } else if removed < len {
        Some(removed)
    } else {
        Some(len - 1)
    }
}

/// Whether the tab at `index` paints a separator at its right edge: it is
/// not selected, not followed by the selected tab, and not the last tab.
fn separator_after(index: usize, selected_index: Option<usize>, len: usize) -> bool {
    if index + 1 >= len {
        return false;
    }
    match selected_index {
        Some(selected) => index != selected && index + 1 != selected,
        None => true,
    }
}

/// A browser-style tab strip on `gpui_base::Tabs` and `gpui_base::Tab`.
///
/// Base owns the tab list role and each tab's role, selected state, and
/// pointer activation. This type owns the look: the bar, the selected
/// tab's surface with its shoulders, the hover surface, the separators,
/// and the close, new-tab, and scroll controls. The strip is one tab stop:
/// with it focused, Left and Right select a neighbor, wrapping at the
/// ends, and the selected tab shows the focus ring.
///
/// A click selects a tab. The close control removes it. The new-tab control
/// emits [`TabsEvent::AddRequested`], unless the strip hands it to the
/// application with [`add_trigger`](Tabs::add_trigger). When the tabs overflow the strip,
/// scroll controls appear at its start.
///
/// The strip and the content below share the window surface. A hairline
/// runs under the strip, and the selected tab is an outline in the
/// hairline's color that opens into it, with a shoulder at each bottom
/// corner. A hovered tab fills the same shape.
///
/// The strip fills its parent, so give it a sized bar. A tab item is as
/// tall as the strip, with its surface starting an inset below the top, so
/// the shoulders always meet the content below. In a [`TitleBar`] clear
/// the bar's padding (`pl_0()`, `pr_0()`, `pt_0()`) so the strip and its
/// hairline span the whole bar; the surface inset already holds the tabs
/// off the top, and the strip keeps its own room at its edges and around
/// its controls, on the spacing scale.
///
/// [`TitleBar`]: crate::TitleBar
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    state: Entity<TabsState>,
    style: StyleRefinement,
    closable: bool,
    confirm_close: bool,
    addable: bool,
    add_trigger: Option<AddTrigger>,
    leading: Pixels,
    renamable: bool,
    context_menu: Option<(Entity<MenuState>, TabMenu)>,
}

type AddTrigger = Box<dyn FnOnce(Button) -> AnyElement>;

/// Builds the rows of a tab's context menu, given the tab's id.
type TabMenu = Rc<dyn Fn(&SharedString, &mut Window, &mut App) -> Vec<MenuEntry>>;

impl Tabs {
    /// A strip for `state`, with close and new-tab controls.
    pub fn new(id: impl Into<ElementId>, state: &Entity<TabsState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            style: StyleRefinement::default(),
            closable: true,
            confirm_close: false,
            addable: true,
            add_trigger: None,
            leading: px(0.),
            renamable: false,
            context_menu: None,
        }
    }

    /// Whether a double click on a tab turns its label into a text field
    /// to rename it, as [`TabsState::start_rename`] does. Off by default.
    pub fn renamable(mut self, renamable: bool) -> Self {
        self.renamable = renamable;
        self
    }

    /// Opens a menu at the pointer on a right click on a tab, with the rows
    /// `items` builds for that tab's id, as a
    /// [`ContextMenu`](crate::ContextMenu) does. A tab with no rows opens
    /// nothing.
    pub fn context_menu(
        mut self,
        state: &Entity<MenuState>,
        items: impl Fn(&SharedString, &mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> Self {
        self.context_menu = Some((state.clone(), Rc::new(items)));
        self
    }

    /// Room at the strip's start, under controls that float over it such
    /// as a sidebar trigger; the hairline runs under the room and the
    /// tabs and the scroll controls start after it. The default is none.
    pub fn with_leading(mut self, leading: impl Into<Pixels>) -> Self {
        self.leading = leading.into();
        self
    }

    /// Whether each tab shows a close control. On by default.
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }

    /// Whether a tab's close control asks the application first: it emits
    /// [`TabsEvent::CloseRequested`] and leaves the tab, so the
    /// application can confirm and then [`remove`](TabsState::remove) it.
    /// Off by default.
    pub fn confirm_close(mut self, confirm: bool) -> Self {
        self.confirm_close = confirm;
        self
    }

    /// Whether the strip ends with a new-tab control. On by default.
    pub fn addable(mut self, addable: bool) -> Self {
        self.addable = addable;
        self
    }

    /// Hands the new-tab control to `wrap` in place of emitting
    /// [`TabsEvent::AddRequested`], such as to make it the trigger of a
    /// [`Popover`](crate::Popover) of what to add. `wrap` gets the control
    /// as the strip draws it, with no click handler.
    pub fn add_trigger<E: IntoElement>(mut self, wrap: impl FnOnce(Button) -> E + 'static) -> Self {
        self.add_trigger = Some(Box::new(move |button| wrap(button).into_any_element()));
        self
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Everything the render needs from the theme, read in one borrow.
struct Look {
    touch: bool,
    bar: Hsla,
    outline: Hsla,
    hover: Hsla,
    foreground: Hsla,
    muted_foreground: Hsla,
    separator: Hsla,
    ring: Hsla,
    surface_inset: Pixels,
    width: Pixels,
    min_width: Pixels,
    shoulder: Pixels,
    radius: Pixels,
    gutter: Pixels,
    separator_inset: Pixels,
    icon: Pixels,
    gap: Pixels,
    control: Pixels,
    close: Pixels,
    text_size: Pixels,
    line_height: Pixels,
    /// Room at the strip's edges and around its controls: the `sm` step.
    edge: Pixels,
    /// The gap between the two scroll controls: the `xs` step.
    control_gap: Pixels,
}

impl Look {
    fn of(theme: &ThemeTokens) -> Self {
        let metrics = &theme.metrics;
        Self {
            touch: theme.touch,
            bar: theme.background(),
            outline: theme.border(),
            hover: theme.muted(),
            foreground: theme.foreground(),
            muted_foreground: theme.muted_foreground(),
            separator: theme.tab_separator,
            ring: theme.ring(),
            surface_inset: metrics.tab_surface_inset,
            width: metrics.tab_width,
            min_width: metrics.tab_min_width,
            shoulder: metrics.tab_shoulder,
            radius: metrics.tab_radius,
            gutter: metrics.tab_separator_gutter,
            separator_inset: metrics.tab_separator_inset,
            icon: metrics.tab_icon,
            gap: metrics.tab_content_gap,
            control: metrics.tab_control,
            close: metrics.tab_close,
            text_size: theme.text_control.size,
            line_height: theme.text_control.line_height,
            edge: theme.base.spacing.sm,
            control_gap: theme.base.spacing.xs,
        }
    }
}

/// One place in the strip: a live tab at its index, or a tab on its way
/// out.
struct Slot<'a> {
    tab: &'a Tab,
    live: Option<usize>,
    entering: bool,
}

/// What the last layout said about the strip, read from the scroll handle.
/// The render draws with it, and the prepaint asks for another frame when
/// the new layout disagrees.
#[derive(Clone, Copy, PartialEq)]
struct Overflow {
    overflowing: bool,
    at_start: bool,
    at_end: bool,
}

impl Overflow {
    fn of(scroll: &ScrollHandle) -> Self {
        let max = scroll.max_offset().x;
        let offset = scroll.offset().x;
        Self {
            overflowing: max > px(0.),
            at_start: offset >= px(0.),
            at_end: offset <= -max,
        }
    }
}

/// A control slot at the strip's start or end. It is as tall as a tab item
/// and starts below the surface inset, so its glyph centers on the tab
/// labels.
fn slot(look: &Look, control: impl IntoElement) -> impl IntoElement {
    h_flex()
        .w(look.control)
        .h_full()
        .pt(look.surface_inset)
        .flex_none()
        .justify_center()
        .border_b_1()
        .border_color(look.outline)
        .child(control)
}

/// Empty room in the strip, `width` wide, with its piece of the hairline.
fn room(look: &Look, width: Pixels) -> Div {
    div()
        .flex_none()
        .h_full()
        .w(width)
        .border_b_1()
        .border_color(look.outline)
}

/// The hairline under the strip: every part of the strip but the selected
/// tab draws its own piece of it at the bottom, and this fills the room
/// after the last control.
fn hairline(look: &Look) -> impl IntoElement {
    room(look, look.edge).flex_1().min_w(look.edge)
}

impl RenderOnce for Tabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let look = Look::of(cx.theme());
        let strip: Arc<ElementId> = Arc::new(self.id.clone());
        let child = |name: &'static str| ElementId::NamedChild(strip.clone(), name.into());
        let state = self.state;
        let (tabs, selected_index, scroll, glide, reveal, entering, leaving, renaming) = {
            let state = state.read(cx);
            (
                state.tabs.clone(),
                state.selected_index(),
                state.scroll.clone(),
                state.glide,
                state.reveal,
                state.entering.clone(),
                state.leaving.clone(),
                state
                    .renaming
                    .as_ref()
                    .map(|renaming| (renaming.id.clone(), renaming.input.clone())),
            )
        };
        let renamable = self.renamable;
        let context_menu = self.context_menu.clone();
        let len = tabs.len();
        let (motion, fast) = {
            let motion = &Theme::global(cx).motion;
            (motion.glide_transition(), motion.fast_transition())
        };
        if let Some(glide) = glide {
            // Keyed by serial, so each glide starts from the offset the
            // strip had; once landed, the offset is left to the wheel.
            let key = (child("glide"), SharedString::from(glide.serial.to_string()));
            let sample = Sequence::new(key, glide.from)
                .with_step(glide.to, motion.clone())
                .sample(window, cx);
            let landed = window.use_keyed_state(child("landed"), cx, |_, _| None::<usize>);
            if !sample.is_finished() || *landed.read(cx) != Some(glide.serial) {
                scroll.set_offset(point(*sample.value(), scroll.offset().y));
                if sample.is_finished() {
                    landed.update(cx, |landed, _| *landed = Some(glide.serial));
                }
            }
        }
        let overflow = Overflow::of(&scroll);
        let has_overlay = TooltipHost::overlay(window, cx).is_some();

        let focus = window
            .use_keyed_state(child("focus"), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let focus_visible = focus.is_focused(window) && window.last_input_was_keyboard();

        let mut slots: Vec<Slot> = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| Slot {
                tab,
                live: Some(index),
                entering: entering.contains(&tab.id),
            })
            .collect();
        for leaving in &leaving {
            let at = leaving.index.min(slots.len());
            slots.insert(
                at,
                Slot {
                    tab: &leaving.tab,
                    live: None,
                    entering: false,
                },
            );
        }
        // Among the viewport's children: after the lead-in and any tab
        // leaving before it.
        let selected_child = slots
            .iter()
            .position(|slot| slot.live == selected_index && slot.live.is_some())
            .map(|position| position + 1);
        if reveal {
            if let Some(child) = selected_child {
                scroll.scroll_to_item(child);
            }
            let revealed = state.clone();
            cx.defer(move |cx| revealed.update(cx, |state, cx| state.revealed(cx)));
        }
        let settle_state = state.clone();
        let items = slots.into_iter().map(|slot| {
            let tab = slot.tab;
            let index = slot.live.unwrap_or(0);
            let live = slot.live.is_some();
            let selected = live && selected_index == Some(index);
            let tab_id = Arc::new(ElementId::NamedChild(strip.clone(), tab.id.clone()));
            // Settling is deferred so the render never writes the entity
            // it reads.
            let width = if slot.entering || !live {
                let (from, to) = if live {
                    (px(0.), look.width)
                } else {
                    (look.width, px(0.))
                };
                let sample = Sequence::new((tab_id.as_ref().clone(), "width"), from)
                    .with_step(to, motion.clone())
                    .sample(window, cx);
                if sample.is_finished() {
                    let settle_state = settle_state.clone();
                    let id = tab.id.clone();
                    cx.defer(move |cx| settle_state.update(cx, |state, cx| state.settle(&id, cx)));
                }
                Some(*sample.value())
            } else {
                None
            };
            let part = |name: &'static str| ElementId::NamedChild(tab_id.clone(), name.into());
            let group = tab.id.clone();
            let separator = live && separator_after(index, selected_index, len);
            let gutter = if separator { look.gutter } else { px(0.) };
            // A shoulder is a circle two shoulders wide, clipped to the
            // shoulder's square at the corner: its ring is the outline's
            // curve, its fill carves the hover fill.
            let hovered = window
                .use_keyed_state(part("hovered"), cx, |_, _| false)
                .clone();
            let hovers = live && !selected && !look.touch && *hovered.read(cx);
            let selection = transition(
                part("selection"),
                f32::from(selected),
                fast.clone(),
                window,
                cx,
            );
            let hover = transition(part("hover"), f32::from(hovers), fast.clone(), window, cx);
            let text = look
                .muted_foreground
                .interpolate(&look.foreground, selection);
            let outline_color = if selected && focus_visible {
                look.ring
            } else {
                look.outline
            }
            .opacity(selection);
            let hairline_color = look.outline.opacity(1. - selection);
            let hover_color = look.hover.opacity(hover);
            let shoulder = |side_left: bool| {
                div()
                    .absolute()
                    .bottom_0()
                    .size(look.shoulder)
                    .overflow_hidden()
                    .bg(hover_color)
                    .map(|this| {
                        if side_left {
                            this.left_0()
                        } else {
                            this.right(gutter)
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .size(look.shoulder * 2.)
                            .rounded_full()
                            .top(-look.shoulder)
                            .bg(look.bar)
                            .border_1()
                            .border_color(outline_color)
                            .map(|this| {
                                if side_left {
                                    this.left(-look.shoulder)
                                } else {
                                    this.right(-look.shoulder)
                                }
                            }),
                    )
            };
            let label_id = part("label");
            let tooltip = TooltipTrigger::text(label_id.clone(), None, tab.label.clone());
            let select_state = state.clone();
            let select_id = tab.id.clone();
            let close_state = state.clone();
            let close_id = tab.id.clone();
            let confirm_close = self.confirm_close;
            let editing = renaming
                .as_ref()
                .filter(|(id, _)| live && id == &tab.id)
                .map(|(_, input)| input.clone());
            let is_editing = editing.is_some();
            let menu_id = tab.id.clone();
            let menu = context_menu.clone();
            let label = match editing {
                Some(input) => {
                    let cancel_state = state.clone();
                    div()
                        .flex_1()
                        .min_w_0()
                        // Escape cancels before the field sees it, which
                        // would pass it on anyway.
                        .capture_key_down(move |event: &KeyDownEvent, _, cx| {
                            if event.keystroke.key == "escape" {
                                cx.stop_propagation();
                                cancel_state.update(cx, |state, cx| state.cancel_rename(cx));
                            }
                        })
                        .child(
                            Input::new(&input)
                                .id(part("rename"))
                                .accessibility_label("Tab name")
                                .h(look.close)
                                .px(look.control_gap)
                                .text_size(look.text_size),
                        )
                        .into_any_element()
                }
                None => div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(look.text_size)
                    .line_height(look.line_height)
                    .child(tab.label.clone())
                    .into_any_element(),
            };
            base::Tab::new(tab_id.as_ref().clone())
                .selected(selected)
                .set_position(index + 1, len)
                .accessibility_label(tab.label.clone())
                .group(group.clone())
                .relative()
                .flex_shrink(1.)
                .h_full()
                .map(|this| match width {
                    Some(width) => this
                        .w(width)
                        .min_w(width.min(look.min_width))
                        .overflow_hidden(),
                    None => this.w(look.width).min_w(look.min_width),
                })
                .border_b_1()
                .border_color(hairline_color)
                .when(live, |this| {
                    this.on_hover(move |is_hovered, _, cx| {
                        hovered.update(cx, |hovered, cx| {
                            if *hovered != *is_hovered {
                                *hovered = *is_hovered;
                                cx.notify();
                            }
                        })
                    })
                })
                .when(live, |this| {
                    this.on_click(move |event, window, cx| {
                        if is_editing {
                            return;
                        }
                        select_state.update(cx, |state, cx| {
                            state.select(select_id.clone(), cx);
                            if renamable && event.click_count() >= 2 {
                                state.start_rename(select_id.clone(), window, cx);
                            }
                        })
                    })
                })
                .when_some(menu.filter(|_| live), |this, (menu, items)| {
                    this.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        cx.stop_propagation();
                        let entries = items(&menu_id, window, cx);
                        if entries.is_empty() {
                            return;
                        }
                        let anchor = MenuAnchor::Point(event.position);
                        menu.update(cx, |menu, cx| menu.open(entries, anchor, window, cx));
                    })
                })
                .child(
                    div()
                        .absolute()
                        .top(look.surface_inset)
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .child(shoulder(true))
                        .child(shoulder(false))
                        // The arc's ring sits one pixel inside its circle,
                        // so the sides sit one pixel in to meet it.
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom(look.shoulder)
                                .left(look.shoulder - px(1.))
                                .right(look.shoulder + gutter - px(1.))
                                .rounded_t(look.radius)
                                .border_1()
                                .border_b_0()
                                .border_color(outline_color),
                        )
                        .child(
                            h_flex()
                                .id(label_id)
                                .test_support()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(look.shoulder)
                                .right(look.shoulder + gutter)
                                .min_w_0()
                                .rounded_t(look.radius)
                                .overflow_hidden()
                                .bg(hover_color)
                                .px_2()
                                .gap(look.gap)
                                .text_color(text)
                                .map(|this| {
                                    if has_overlay {
                                        this.on_prepaint(tooltip.track_bounds())
                                            .on_hover(move |hovered, window, cx| {
                                                tooltip.hovered(*hovered, window, cx)
                                            })
                                            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                                                TooltipTrigger::pressed(window, cx)
                                            })
                                    } else {
                                        this.tooltip(tooltip.native())
                                    }
                                })
                                .when_some(tab.icon.clone(), |this, icon| {
                                    this.child(icon.size(look.icon).flex_none())
                                })
                                .child(label)
                                .when(self.closable && live && !is_editing, |this| {
                                    this.child(
                                        h_flex()
                                            .size(look.close)
                                            .flex_none()
                                            .justify_center()
                                            .when(!selected && !look.touch, |this| {
                                                this.invisible()
                                                    .group_hover(group, |style| style.visible())
                                            })
                                            .child(
                                                Button::new(part("close"))
                                                    .ghost()
                                                    .size(ButtonSize::Xs)
                                                    .icon(IconName::Close)
                                                    .rounded_full()
                                                    .accessibility_label("Close tab")
                                                    .tooltip("Close tab")
                                                    .tab_stop(false)
                                                    .on_click(move |_, _, cx| {
                                                        cx.stop_propagation();
                                                        close_state.update(cx, |state, cx| {
                                                            if confirm_close {
                                                                cx.emit(TabsEvent::CloseRequested(
                                                                    close_id.clone(),
                                                                ));
                                                            } else {
                                                                state.remove(close_id.clone(), cx);
                                                            }
                                                        })
                                                    }),
                                            ),
                                    )
                                }),
                        ),
                )
                .when(separator, |this| {
                    this.child(
                        div()
                            .absolute()
                            .top(look.separator_inset)
                            .bottom(look.separator_inset)
                            .right((look.gutter - px(1.)) / 2.)
                            .w(px(1.))
                            .bg(look.separator),
                    )
                })
        });

        let prepaint_scroll = scroll.clone();
        let wheel_scroll = scroll.clone();
        let wheel_state = state.clone();
        let viewport = div()
            .on_children_prepainted(move |_, window, _| {
                if Overflow::of(&prepaint_scroll) != overflow {
                    // The controls that appear next frame shift the tabs,
                    // so the selected tab is revealed again after them.
                    if let Some(child) = selected_child {
                        prepaint_scroll.scroll_to_item(child);
                    }
                    window.request_animation_frame();
                }
            })
            .id(child("scroll"))
            .test_support()
            .flex()
            .items_end()
            .h_full()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&scroll)
            // A child, not padding: GPUI counts a scroll box's padding into
            // its content twice, which keeps an empty strip scrollable.
            .child(room(&look, look.edge))
            .children(items);

        let back_state = state.clone();
        let forward_state = state.clone();
        let add_state = state.clone();
        let key_state = state;

        div()
            .id(self.id)
            .test_support()
            .w_full()
            .h_full()
            .min_w_0()
            .bg(look.bar)
            .track_focus(&focus)
            .tab_index(0)
            .on_key_down(move |event: &KeyDownEvent, _, cx| {
                // While a label is a text field, the arrows move its caret.
                if event.keystroke.modifiers.modified() || key_state.read(cx).renaming.is_some() {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "left" => key_state.update(cx, |state, cx| state.select_neighbor(-1, cx)),
                    "right" => key_state.update(cx, |state, cx| state.select_neighbor(1, cx)),
                    "home" => key_state.update(cx, |state, cx| state.select_edge(false, cx)),
                    "end" => key_state.update(cx, |state, cx| state.select_edge(true, cx)),
                    _ => return,
                }
                cx.stop_propagation();
            })
            .relative()
            .refine_style(&self.style)
            // A scroll area around the strip takes the wheel in the capture
            // phase, from the outside in, so the strip takes its steps
            // there too while it overflows, and its stop keeps them from
            // the page. A mouse line glides; a trackpad step lands at once.
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, _| {
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture
                                || !hitbox.should_handle_scroll(window)
                                || wheel_scroll.max_offset().x <= px(0.)
                            {
                                return;
                            }
                            cx.stop_propagation();
                            let delta = event.delta.pixel_delta(window.line_height());
                            let step = if delta.x != px(0.) { delta.x } else { delta.y };
                            if step == px(0.) {
                                return;
                            }
                            wheel_state.update(cx, |state, cx| {
                                if event.delta.precise() {
                                    state.pan_by(step, cx);
                                } else {
                                    state.wheel_by(step, cx);
                                }
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .child(
                base::Tabs::new(child("list"))
                    .flex()
                    .items_end()
                    .size_full()
                    .when(self.leading > px(0.), |this| {
                        this.child(room(&look, self.leading))
                    })
                    .when(overflow.overflowing, |this| {
                        this.child(room(&look, look.edge))
                            .child(slot(
                                &look,
                                Button::new(child("back"))
                                    .ghost()
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::ChevronLeft)
                                    .accessibility_label("Scroll tabs left")
                                    .tooltip("Scroll tabs left")
                                    .tab_stop(false)
                                    .disabled(overflow.at_start)
                                    .on_click(move |_, _, cx| {
                                        back_state.update(cx, |state, cx| {
                                            state.scroll_by(-look.width, cx)
                                        })
                                    }),
                            ))
                            .child(room(&look, look.control_gap))
                            .child(slot(
                                &look,
                                Button::new(child("forward"))
                                    .ghost()
                                    .size(ButtonSize::Sm)
                                    .icon(IconName::ChevronRight)
                                    .accessibility_label("Scroll tabs right")
                                    .tooltip("Scroll tabs right")
                                    .tab_stop(false)
                                    .disabled(overflow.at_end)
                                    .on_click(move |_, _, cx| {
                                        forward_state
                                            .update(cx, |state, cx| state.scroll_by(look.width, cx))
                                    }),
                            ))
                            .child(room(&look, look.edge))
                    })
                    .child(viewport)
                    .when(self.addable, |this| {
                        let button = Button::new(child("add"))
                            .ghost()
                            .size(ButtonSize::Sm)
                            .icon(IconName::Plus)
                            .accessibility_label("New tab")
                            .tooltip("New tab");
                        let control = match self.add_trigger {
                            Some(wrap) => wrap(button),
                            None => button
                                .on_click(move |_, _, cx| {
                                    add_state.update(cx, |state, cx| state.request_add(cx))
                                })
                                .into_any_element(),
                        };
                        this.child(room(&look, look.edge))
                            .child(slot(&look, control))
                    })
                    .child(hairline(&look)),
            )
            .when_some(self.context_menu, |this, (menu, _)| {
                this.child(MenuPanels::new(child("menu"), &menu))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    fn ids(state: &TabsState) -> Vec<&str> {
        state.tabs.iter().map(|tab| tab.id.as_ref()).collect()
    }

    #[test]
    fn removing_the_selected_tab_selects_the_next_or_the_previous() {
        assert_eq!(next_selection(0, 2), Some(0), "the tab that took its place");
        assert_eq!(next_selection(1, 2), Some(1));
        assert_eq!(
            next_selection(2, 2),
            Some(1),
            "the last tab: the one before"
        );
        assert_eq!(next_selection(0, 0), None, "nothing left");
    }

    #[test]
    fn a_separator_follows_a_tab_away_from_the_selection_and_the_end() {
        // Five tabs, the third selected: | a | b   c   d | e
        assert!(separator_after(0, Some(2), 5));
        assert!(!separator_after(1, Some(2), 5), "before the selected tab");
        assert!(!separator_after(2, Some(2), 5), "the selected tab");
        assert!(separator_after(3, Some(2), 5));
        assert!(!separator_after(4, Some(2), 5), "the last tab");
        assert!(separator_after(0, None, 2), "no selection");
        assert!(!separator_after(0, None, 1));
    }

    #[gpui_kit::test]
    fn the_edges_are_selectable(cx: &mut TestAppContext) {
        let state = cx
            .new(|_| TabsState::new([Tab::new("a", "A"), Tab::new("b", "B"), Tab::new("c", "C")]));
        state.update(cx, |state, cx| state.select_edge(true, cx));
        assert_eq!(
            state.read_with(cx, |state, _| state.selected().cloned()),
            Some("c".into())
        );
        state.update(cx, |state, cx| state.select_edge(false, cx));
        assert_eq!(
            state.read_with(cx, |state, _| state.selected().cloned()),
            Some("a".into())
        );
    }

    #[gpui_kit::test]
    fn inserting_puts_a_tab_in_place_without_selecting_it(cx: &mut TestAppContext) {
        let state = cx.new(|_| TabsState::new([Tab::new("a", "A"), Tab::new("c", "C")]));
        state.update(cx, |state, cx| {
            state.insert(1, Tab::new("b", "B"), cx);
            state.insert(99, Tab::new("d", "D"), cx);
        });
        state.read_with(cx, |state, _| {
            assert_eq!(ids(state), ["a", "b", "c", "d"]);
            assert_eq!(state.selected().map(AsRef::as_ref), Some("a"));
        });
    }

    #[gpui_kit::test]
    fn the_state_selects_pushes_and_removes(cx: &mut TestAppContext) {
        let state = cx.new(|_| TabsState::new([Tab::new("a", "A"), Tab::new("b", "B")]));
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update({
            let events = events.clone();
            let state = state.clone();
            move |cx| {
                cx.subscribe(&state, move |_, event: &TabsEvent, _| {
                    events.borrow_mut().push(event.clone())
                })
                .detach();
            }
        });
        assert_eq!(
            state.read_with(cx, |state, _| state.selected().cloned()),
            Some("a".into())
        );
        state.update(cx, |state, cx| {
            state.select("b", cx);
            state.select("b", cx);
            state.select("missing", cx);
            state.push(Tab::new("c", "C"), cx);
            state.remove("c", cx);
            state.remove("a", cx);
            state.request_add(cx);
            assert_eq!(ids(state), ["b"]);
            assert_eq!(state.selected(), Some(&"b".into()));
            state.remove("b", cx);
            assert!(state.selected().is_none());
        });
        cx.run_until_parked();
        assert_eq!(
            *events.borrow(),
            [
                TabsEvent::Selected("b".into()),
                TabsEvent::Selected("c".into()),
                TabsEvent::Closed("c".into()),
                TabsEvent::Selected("b".into()),
                TabsEvent::Closed("a".into()),
                TabsEvent::AddRequested,
                TabsEvent::Closed("b".into()),
            ]
        );
    }

    #[gpui_kit::test]
    fn a_neighbor_step_wraps_at_both_ends(cx: &mut TestAppContext) {
        let state = cx.new(|_| TabsState::new([Tab::new("a", "A"), Tab::new("b", "B")]));
        state.update(cx, |state, cx| {
            state.select_neighbor(1, cx);
            assert_eq!(state.selected(), Some(&"b".into()));
            state.select_neighbor(1, cx);
            assert_eq!(state.selected(), Some(&"a".into()), "wraps");
            state.select_neighbor(-1, cx);
            assert_eq!(state.selected(), Some(&"b".into()), "wraps back");
        });
        let empty = cx.new(|_| TabsState::new([]));
        empty.update(cx, |state, cx| state.select_neighbor(1, cx));
    }

    #[test]
    fn a_tab_reads_back_and_debugs_without_its_icon_bytes() {
        let tab = Tab::new("t", "Terminal").with_icon(IconName::SquareTerminal);
        assert_eq!(tab.id(), &SharedString::from("t"));
        assert_eq!(tab.label(), &SharedString::from("Terminal"));
        assert!(tab.icon().is_some());
        assert!(format!("{tab:?}").contains("square-terminal"));
    }
}
