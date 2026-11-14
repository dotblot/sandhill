use gpui_kit::{
    App, ElementId, Entity, IntoElement, ParentElement as _, RenderOnce, StyleRefinement, Styled,
    Window,
    assets::IconName,
    base::{Align, StyledExt as _},
    div, px, rems,
};

use crate::{
    ActiveTheme as _, Button, ButtonSize, DropdownMenu, Icon, MenuEntry, MenuItem,
    ModelPickerState,
    collapse::{self, Need},
};

/// The effort of the chosen model as a small dropdown: the level's name,
/// such as "Medium", and a chevron. Its menu lists the state's effort
/// levels, [`EFFORT_LABELS`](crate::EFFORT_LABELS) unless
/// [`with_effort_levels`](crate::ModelPickerState::with_effort_levels) set
/// others, with a check on the current one, and choosing a level
/// sets it on the [`ModelPickerState`], which reports
/// [`ModelPickerEvent::EffortChanged`](crate::ModelPickerEvent::EffortChanged).
///
/// The menu is a [`DropdownMenu`]. It draws nothing while the chosen model
/// has no effort setting, so a toolbar can always include it; use
/// [`ModelPickerState::effort`] to know whether it shows.
///
/// ```no_run
/// use gpui_cn::{EffortMenu, ModelPickerState};
/// use gpui_kit::Entity;
///
/// fn effort(models: &Entity<ModelPickerState>) -> EffortMenu {
///     EffortMenu::new("effort", models)
/// }
/// ```
///
/// The default icons (the mic, the shield, the file kinds, the git branch) are
/// not in the default `gpui_kit::assets::Assets`: register
/// [`ComposerAssets`](crate::ComposerAssets).
#[derive(IntoElement)]
#[non_exhaustive]
pub struct EffortMenu {
    id: ElementId,
    state: Entity<ModelPickerState>,
    icon_only: bool,
    style: StyleRefinement,
}

impl EffortMenu {
    /// An effort menu on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<ModelPickerState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            icon_only: false,
            style: StyleRefinement::default(),
        }
    }

    /// Shows the gauge alone, with "Effort: Medium" in a tooltip and no
    /// chevron, as a narrow toolbar does.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }

    /// What the trigger needs of a row: its width with the label at the
    /// minimum, and its width as an icon alone. Nothing while the chosen
    /// model has no effort.
    pub(crate) fn need(&self, window: &Window, cx: &App) -> Option<Need> {
        let level = self.state.read(cx).effort()?;
        let theme = cx.theme();
        let padding = theme.metrics.control_padding_sm;
        let rem = window.rem_size();
        let gap = rems(0.375).to_pixels(rem);
        let label = collapse::text_width(
            &self.state.read(cx).effort_levels()[usize::from(level)],
            window,
        )
        .min(collapse::min_label_width(window, collapse::MIN_LABEL_CHARS));
        Some(Need::flexible(
            padding * 2. + rems(1.).to_pixels(rem) + gap + label + gap + rems(0.75).to_pixels(rem),
            Some(theme.metrics.control_sm),
        ))
    }
}

impl Styled for EffortMenu {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for EffortMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let need = self.need(window, cx);
        let (level, menu) = {
            let state = self.state.read(cx);
            (state.effort(), state.effort_menu_state().clone())
        };
        let Some(level) = level else {
            return gpui_kit::div().into_any_element();
        };
        let muted = cx.theme().muted_foreground();
        let word = self.state.read(cx).effort_levels()[usize::from(level)].clone();
        let min = match (self.icon_only, need.and_then(|need| need.icon_only)) {
            (true, Some(icon_only)) => icon_only,
            _ => need.map_or(px(0.), |need| need.min),
        };
        let button = Button::new(ElementId::NamedChild(
            self.id.clone().into(),
            "trigger".into(),
        ))
        .ghost()
        .size(ButtonSize::Sm)
        .accessibility_label("Effort")
        .min_w(min)
        .text_color(muted)
        .icon(Icon::from(IconName::Gauge));
        let trigger = if self.icon_only {
            button.tooltip(format!("Effort: {word}"))
        } else {
            let keep = collapse::text_width(&word, window)
                .min(collapse::min_label_width(window, collapse::MIN_LABEL_CHARS));
            button
                .child(
                    div()
                        .min_w(keep)
                        .flex_shrink(1.)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(word),
                )
                .flex_shrink(1.)
                .trailing_icon(Icon::from(IconName::ChevronDown).size_3())
        };
        let state = self.state.clone();
        DropdownMenu::new(self.id, &menu)
            .align(Align::End)
            .refine_style(&self.style)
            .trigger(trigger)
            .items(move |_, cx| {
                let current = state.read(cx).effort();
                state
                    .read(cx)
                    .effort_levels()
                    .iter()
                    .enumerate()
                    .map(|(index, label)| {
                        MenuEntry::from(
                            MenuItem::new(index.to_string(), label.clone())
                                .checked(current == Some(index as u8)),
                        )
                    })
                    .collect()
            })
            .into_any_element()
    }
}
