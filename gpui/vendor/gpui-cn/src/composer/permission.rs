use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StyleRefinement, Styled, Subscription, Window,
    assets::IconName,
    base::{Align, Selectable, StyledExt as _},
    div,
    prelude::FluentBuilder as _,
    px, rems,
};

use crate::{
    ActiveTheme as _, Button, ButtonSize, DropdownMenu, Icon, MenuEntry, MenuEvent, MenuItem,
    MenuState,
    collapse::{self, Need},
    menu::{MenuLook, OpenSlot, label_block, line_slot, row_line},
};

/// How many characters of its label the permission trigger keeps before it
/// becomes an icon. It keeps fewer than the other controls
/// (`collapse::MIN_LABEL_CHARS`) because the mode names are short words,
/// so six characters already tell them apart.
const MIN_LABEL_CHARS: usize = 6;

/// The key of the menu row that reports [`PermissionEvent::LearnMore`].
const LEARN_MORE: &str = "learn-more";

/// How a permission mode reads: an ordinary choice, or a risky one that
/// shows in the warning color.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionTone {
    /// Muted, like the other labels. The default.
    #[default]
    Normal,
    /// The warning color, for a mode that runs without asking.
    Warning,
}

/// How much an agent may do without asking, as one entry of a
/// [`PermissionMenu`]: an id, a label, a line of description, and an icon.
///
/// The id comes from the domain and is unique in the menu. The default
/// set is [`PermissionMode::defaults`]; an application defines its own
/// modes the same way.
///
/// ```
/// use gpui_cn::PermissionMode;
/// use gpui_kit::assets::IconName;
///
/// let strict = PermissionMode::new("strict", "Strict")
///     .description("Ask before every change")
///     .icon(IconName::Check);
/// assert_eq!(strict.label().as_ref(), "Strict");
/// ```
#[derive(Clone)]
#[non_exhaustive]
pub struct PermissionMode {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    icon: Option<Icon>,
    tone: PermissionTone,
}

impl PermissionMode {
    /// The id of the default `Auto` mode.
    pub const AUTO: &'static str = "auto";
    /// The id of the default `Manual` mode.
    pub const MANUAL: &'static str = "manual";
    /// The id of the default `Plan` mode.
    pub const PLAN: &'static str = "plan";
    /// The id of the default `Bypass all` mode.
    pub const BYPASS: &'static str = "bypass";

    /// A mode with a stable id and the label it shows.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            icon: None,
            tone: PermissionTone::Normal,
        }
    }

    /// A line under the label in the menu.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// The icon before the label, in the menu and on the trigger.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Marks the mode risky or ordinary. A risky one shows in the warning
    /// color on the trigger and in the menu.
    pub fn tone(mut self, tone: PermissionTone) -> Self {
        self.tone = tone;
        self
    }

    /// The tone.
    pub fn tone_of(&self) -> PermissionTone {
        self.tone
    }

    /// The id.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// The label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The line under the label.
    pub fn description_text(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    /// The icon.
    pub fn icon_of(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    /// The four default modes: `Auto`, `Manual`, `Plan`, and `Bypass all`.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new(Self::AUTO, "Auto")
                .description("Run safe actions, ask before risky ones")
                .icon(IconName::Gauge),
            Self::new(Self::MANUAL, "Manual")
                .description("Ask before every action")
                .icon(IconName::Hand),
            Self::new(Self::PLAN, "Plan")
                .description("Read only: propose a plan and wait")
                .icon(IconName::ClipboardList),
            Self::new(Self::BYPASS, "Bypass all")
                .description("Run everything without asking")
                .icon(IconName::ShieldAlert)
                .tone(PermissionTone::Warning),
        ]
    }
}

/// What a [`PermissionState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PermissionEvent {
    /// The selected mode changed. The payload is its id.
    Changed(SharedString),
    /// The "Learn more" row was chosen.
    LearnMore,
}

/// The modes, the selected one, and the open menu of a
/// [`PermissionMenu`].
///
/// Owned by the view that shows the menu, which observes it so a change
/// renders. The selection is kept by id.
pub struct PermissionState {
    modes: Vec<PermissionMode>,
    selected: SharedString,
    menu: Entity<MenuState>,
    _menu_events: Subscription,
}

impl EventEmitter<PermissionEvent> for PermissionState {}

impl PermissionState {
    /// A state over the default modes with `Auto` selected.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let menu = cx.new(MenuState::new);
        let subscription = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Activated(key) = event {
                if key.as_ref() == LEARN_MORE {
                    cx.emit(PermissionEvent::LearnMore);
                } else {
                    this.set_selected(key.clone(), cx);
                }
            }
        });
        Self {
            modes: PermissionMode::defaults(),
            selected: PermissionMode::AUTO.into(),
            menu,
            _menu_events: subscription,
        }
    }

    /// Replaces the modes. The first one is selected when the selected
    /// id is not among them.
    pub fn with_modes(mut self, modes: impl IntoIterator<Item = PermissionMode>) -> Self {
        self.modes = modes.into_iter().collect();
        if !self.modes.iter().any(|mode| mode.id == self.selected)
            && let Some(first) = self.modes.first()
        {
            self.selected = first.id.clone();
        }
        self
    }

    /// Selects the mode with `id`, if there is one.
    pub fn with_selected(mut self, id: impl Into<SharedString>) -> Self {
        let id = id.into();
        if self.modes.iter().any(|mode| mode.id == id) {
            self.selected = id;
        }
        self
    }

    /// The modes, in order.
    pub fn modes(&self) -> &[PermissionMode] {
        &self.modes
    }

    /// The id of the selected mode.
    pub fn selected(&self) -> &SharedString {
        &self.selected
    }

    /// The selected mode.
    pub fn selected_mode(&self) -> Option<&PermissionMode> {
        self.modes.iter().find(|mode| mode.id == self.selected)
    }

    /// Selects the mode with `id` and emits `Changed`. An unknown id, or
    /// the mode already selected, changes nothing.
    pub fn set_selected(&mut self, id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let id = id.into();
        if id == self.selected || !self.modes.iter().any(|mode| mode.id == id) {
            return;
        }
        self.selected = id.clone();
        cx.emit(PermissionEvent::Changed(id));
        cx.notify();
    }

    /// Replaces the modes, keeping the selection when it survives and
    /// otherwise moving it to the first mode.
    pub fn set_modes(
        &mut self,
        modes: impl IntoIterator<Item = PermissionMode>,
        cx: &mut Context<Self>,
    ) {
        self.modes = modes.into_iter().collect();
        if !self.modes.iter().any(|mode| mode.id == self.selected)
            && let Some(first) = self.modes.first()
        {
            self.selected = first.id.clone();
            cx.emit(PermissionEvent::Changed(self.selected.clone()));
        }
        cx.notify();
    }

    /// The state of the menu this picker opens.
    pub fn menu(&self) -> &Entity<MenuState> {
        &self.menu
    }
}

type TriggerBuilder = Box<dyn FnOnce(bool) -> gpui_kit::AnyElement>;

/// A picker for a [`PermissionMode`]: a quiet trigger that shows the
/// selected mode, and a menu of every mode with its description, the
/// selected one checked, under a header with a "Learn more" row.
///
/// The trigger is plain text with an icon until the pointer, a press, or
/// the open menu gives it a surface. [`trigger`](Self::trigger) replaces
/// it. The menu is a [`DropdownMenu`], so keyboard, dismissal, and focus
/// are the menu's.
///
/// ```no_run
/// use gpui_cn::{PermissionMenu, PermissionState};
/// use gpui_kit::{AppContext as _, Context};
///
/// fn picker(cx: &mut Context<()>) -> PermissionMenu {
///     let state = cx.new(PermissionState::new);
///     PermissionMenu::new("permission", &state)
/// }
/// ```
///
/// The default icons (the mic, the shield, the file kinds, the git branch) are
/// not in the default `gpui_kit::assets::Assets`: register
/// [`ComposerAssets`](crate::ComposerAssets).
#[derive(IntoElement)]
#[non_exhaustive]
pub struct PermissionMenu {
    id: ElementId,
    state: Entity<PermissionState>,
    trigger: Option<TriggerBuilder>,
    learn_more: bool,
    align: Align,
    icon_only: bool,
    style: StyleRefinement,
}

impl PermissionMenu {
    /// A permission menu on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<PermissionState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            trigger: None,
            learn_more: true,
            align: Align::Start,
            icon_only: false,
            style: StyleRefinement::default(),
        }
    }

    /// Replaces the trigger. It shows as selected while the menu is open.
    pub fn trigger(mut self, trigger: impl Selectable + IntoElement + 'static) -> Self {
        self.trigger = Some(Box::new(move |open| trigger.open(open).into_any_element()));
        self
    }

    /// Whether the menu has its header with the "Learn more" row. On by
    /// default.
    pub fn learn_more(mut self, learn_more: bool) -> Self {
        self.learn_more = learn_more;
        self
    }

    /// Which edge of the trigger the menu lines up with. The default is
    /// `Start`.
    pub fn align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }

    /// Shows the icon of the chosen mode alone, with its label in a
    /// tooltip and no chevron, as a narrow toolbar does. A mode with no
    /// icon ignores it.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }

    /// What the trigger needs of a row: its width with the label at the
    /// minimum, and its width as an icon alone.
    pub(crate) fn need(&self, window: &Window, cx: &App) -> Need {
        let theme = cx.theme();
        let padding = theme.metrics.control_padding_sm;
        let rem = window.rem_size();
        let gap = rems(0.375).to_pixels(rem);
        let (label, has_icon) = match self.state.read(cx).selected_mode() {
            Some(mode) => (mode.label.clone(), mode.icon.is_some()),
            None => ("Permission".into(), false),
        };
        let label = collapse::text_width(&label, window)
            .min(collapse::min_label_width(window, MIN_LABEL_CHARS));
        let icon = rems(1.).to_pixels(rem);
        let lead = if has_icon { icon + gap } else { px(0.) };
        Need::flexible(
            padding * 2. + lead + label,
            has_icon.then_some(theme.metrics.control_sm),
        )
    }
}

impl Styled for PermissionMenu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The rows of the menu, built as it opens.
fn entries(state: &Entity<PermissionState>, learn_more: bool, cx: &App) -> Vec<MenuEntry> {
    let state = state.read(cx);
    let mut entries = Vec::new();
    if learn_more {
        entries.push(
            MenuItem::new(LEARN_MORE, "Learn more")
                .render(|_, window, cx| {
                    let look = MenuLook::of(cx.theme(), window.rem_size());
                    let link = cx.theme().link;
                    row_line()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .text_color(look.muted_foreground)
                                .child("Permission mode"),
                        )
                        .child(div().text_color(link).child("Learn more"))
                })
                .into(),
        );
        entries.push(MenuEntry::Separator);
    }
    entries.extend(state.modes.iter().map(|mode| {
        let selected = mode.id == state.selected;
        let (label, description, icon, tone) = (
            mode.label.clone(),
            mode.description.clone(),
            mode.icon.clone(),
            mode.tone,
        );
        MenuItem::new(mode.id.clone(), mode.label.clone())
            .render(move |_, window, cx| {
                let look = MenuLook::of(cx.theme(), window.rem_size());
                let warning = cx.theme().warning;
                let icon_color = match tone {
                    PermissionTone::Warning => warning,
                    PermissionTone::Normal => look.muted_foreground,
                };
                let icon = icon
                    .clone()
                    .map(|icon| line_slot(&look).child(icon.size_4().text_color(icon_color)));
                let mut block = label_block(label.clone(), &[], description.clone(), &look);
                if tone == PermissionTone::Warning {
                    block = div()
                        .flex_1()
                        .min_w_0()
                        .text_color(warning)
                        .child(block)
                        .into_any_element();
                }
                row_line()
                    .children(icon)
                    .child(block)
                    .when(selected, |this| {
                        this.child(
                            line_slot(&look).child(
                                Icon::from(IconName::Check)
                                    .size_4()
                                    .text_color(look.indicator),
                            ),
                        )
                    })
            })
            .into()
    }));
    entries
}

impl RenderOnce for PermissionMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let menu_state = self.state.read(cx).menu.clone();
        let learn_more = self.learn_more;
        let state = self.state.clone();
        let need = self.need(window, cx);
        let menu = DropdownMenu::new(self.id.clone(), &menu_state)
            .align(self.align)
            .min_w(match (self.icon_only, need.icon_only) {
                (true, Some(icon_only)) => icon_only,
                _ => need.min,
            })
            .refine_style(&self.style)
            .items(move |_, cx| entries(&state, learn_more, cx));
        match self.trigger {
            Some(build) => menu.trigger(OpenSlot::built(build)).into_any_element(),
            None => {
                let button = Button::new(ElementId::NamedChild(self.id.into(), "trigger".into()))
                    .ghost()
                    .size(ButtonSize::Sm)
                    .flex_shrink(1.)
                    .accessibility_label("Permission mode");
                let theme = cx.theme();
                let (warning, muted) = (theme.warning, theme.muted_foreground());
                let button = match self.state.read(cx).selected_mode().cloned() {
                    Some(mode) => {
                        let color = match mode.tone {
                            PermissionTone::Warning => warning,
                            PermissionTone::Normal => muted,
                        };
                        match mode.icon.clone() {
                            Some(icon) if self.icon_only => button
                                .icon(icon)
                                .text_color(color)
                                .tooltip(mode.label.clone()),
                            icon => {
                                let keep = collapse::text_width(&mode.label, window)
                                    .min(collapse::min_label_width(window, MIN_LABEL_CHARS));
                                button
                                    .when_some(icon, |button, icon| button.icon(icon))
                                    .child(
                                        div()
                                            .min_w(keep)
                                            .flex_shrink(1.)
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(mode.label.clone()),
                                    )
                                    .text_color(color)
                            }
                        }
                    }
                    None => button.label("Permission").text_color(muted),
                };
                menu.trigger(button).into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_mode_that_runs_without_asking_is_a_warning_by_default() {
        let tones: Vec<_> = PermissionMode::defaults()
            .iter()
            .map(|mode| (mode.id().to_string(), mode.tone_of()))
            .collect();
        for (id, tone) in tones {
            let want = if id == PermissionMode::BYPASS {
                PermissionTone::Warning
            } else {
                PermissionTone::Normal
            };
            assert_eq!(tone, want, "{id}");
        }
        assert_eq!(
            PermissionMode::new("x", "X").tone_of(),
            PermissionTone::Normal
        );
    }
}
