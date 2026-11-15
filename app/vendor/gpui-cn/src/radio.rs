use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, Axis, ClickEvent, ElementId, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, ParentElement, Pixels, PlatformInput,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    TestSupportExt as _, Window,
    accesskit::Orientation,
    base::{self, Disableable, StyledExt as _},
    div, point,
    prelude::FluentBuilder as _,
    px,
};

use crate::{ActiveTheme as _, Theme, ThemeTokens};

type ChangeHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The round indicator of a radio: a thin grey ring, or while checked a
/// disc of the control accent with a white dot, as macOS paints it. The
/// accent is the one a switch that is on uses. It paints only. A row that is already a
/// button, such as a model in a list, draws it to show which one is on.
///
/// ```
/// use gpui_cn::RadioMark;
///
/// let _ = RadioMark::new(true);
/// ```
#[derive(IntoElement)]
pub struct RadioMark {
    checked: bool,
    style: StyleRefinement,
}

impl RadioMark {
    /// An indicator, with its dot shown while `checked`.
    pub fn new(checked: bool) -> Self {
        Self {
            checked,
            style: StyleRefinement::default(),
        }
    }

    /// Whether the dot shows.
    pub fn is_checked(&self) -> bool {
        self.checked
    }
}

impl Styled for RadioMark {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The ring and the fill of an indicator: a chosen one is a disc of the
/// control accent, the color a switch that is on takes for its track.
fn colors(checked: bool, theme: &ThemeTokens) -> (Hsla, Option<Hsla>) {
    if checked {
        (theme.switch_track_on, Some(theme.switch_track_on))
    } else {
        (theme.field_border, None)
    }
}

impl RenderOnce for RadioMark {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (size, dot) = (theme.metrics.radio_size, theme.metrics.radio_dot);
        let radius = theme.radius_full();
        let (ring, fill) = colors(self.checked, theme);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(size)
            .rounded(radius)
            .border_1()
            .border_color(ring)
            .when_some(fill, |this, fill| this.bg(fill))
            .when(self.checked, |this| {
                this.child(div().size(dot).rounded(radius).bg(theme.switch_thumb))
            })
            .refine_style(&self.style)
    }
}

/// A shadcn-style radio on `gpui_base::Radio`: a [`RadioMark`] with an
/// optional label beside it.
///
/// Base owns focus, Tab order, Enter and Space activation, the disabled
/// contract, and the radio role with its selected state. A radio cannot
/// uncheck itself, so activation reports only the request to check it. The
/// checked value is the application's: a group of radios renders the one
/// value it keeps. Wrap them in a [`RadioGroup`].
///
/// ```
/// use gpui_cn::Radio;
///
/// let _ = Radio::new("small").checked(true).label("Small");
/// ```
///
/// The id keys focus, so it must be stable across frames and unique under
/// the nearest stateful ancestor.
#[derive(IntoElement)]
#[non_exhaustive]
pub struct Radio {
    id: ElementId,
    style: StyleRefinement,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    accessibility_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    tab_index: isize,
    tab_stop: bool,
    children: Vec<AnyElement>,
}

impl Radio {
    /// An unchecked radio with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            checked: false,
            disabled: false,
            label: None,
            accessibility_label: None,
            on_change: None,
            tab_index: 0,
            tab_stop: true,
            children: Vec::new(),
        }
    }

    /// Whether the radio is the chosen one.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Whether the radio is the chosen one.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// The text beside the indicator. It also names the radio for a screen
    /// reader unless [`accessibility_label`](Self::accessibility_label) is set.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The name a screen reader announces, when it differs from the label.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Called when the radio is activated while unchecked, by pointer,
    /// Enter, or Space.
    pub fn on_change(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// The focus traversal index. The default is `0`.
    pub fn tab_index(mut self, tab_index: isize) -> Self {
        self.tab_index = tab_index;
        self
    }

    /// Whether Tab reaches the radio. The default is `true`.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
}

impl Disableable for Radio {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for Radio {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Radio {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Radio {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let (ring, ring_spread, strength, gap, text): (_, Pixels, _, _, _) = {
            let theme = cx.theme();
            (
                theme.focus_ring(),
                theme.metrics.focus_ring,
                theme.disabled_opacity,
                theme.base.spacing.sm,
                theme.text_control,
            )
        };
        let disabled = self.disabled;
        let checked = self.checked;
        let focus_handle = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let focus_visible = focus_handle.is_focused(window) && window.last_input_was_keyboard();
        let name = self.accessibility_label.or_else(|| self.label.clone());
        let on_change = self.on_change;
        base::Radio::new(self.id)
            .track_focus(&focus_handle)
            .checked(checked)
            .disabled(disabled)
            .tab_index(self.tab_index)
            .tab_stop(self.tab_stop)
            .when_some(name, |this, name| this.accessibility_label(name))
            .flex()
            .items_center()
            .gap(gap)
            .text_size(text.size)
            .line_height(text.line_height)
            .when(disabled, |this| this.opacity(strength))
            .map(|this| {
                if !disabled && pointer_cursors {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .refine_style(&self.style)
            .child(RadioMark::new(checked).when(focus_visible, |this| {
                this.shadow(vec![gpui_kit::BoxShadow {
                    color: ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: ring_spread,
                    inset: false,
                }])
            }))
            .children(self.label)
            .children(self.children)
            .when_some(on_change.filter(|_| !disabled), |this, on_change| {
                this.on_change(move |_, event, window, cx| on_change(event, window, cx))
            })
    }
}

/// The container of a set of [`Radio`] values: the radio-group role with
/// its orientation (base's `RadioGroup` is only that, and an element that
/// tests can find needs the id on the element itself), laid out as a column, or as a row for
/// [`horizontal`](Self::horizontal).
///
/// The arrow keys move through the group, wrapping at the ends: the focus
/// goes to the next radio that can take it and that radio is chosen, as
/// Space would choose it. A radio that is disabled is skipped. For one tab
/// stop in the group, give the radios that are not checked
/// `.tab_stop(false)`.
///
/// ```
/// use gpui_cn::{Radio, RadioGroup};
/// use gpui_kit::ParentElement as _;
///
/// let _ = RadioGroup::new("size")
///     .child(Radio::new("small").checked(true).label("Small"))
///     .child(Radio::new("large").label("Large"));
/// ```
#[derive(IntoElement)]
#[non_exhaustive]
pub struct RadioGroup {
    id: ElementId,
    style: StyleRefinement,
    axis: Axis,
    children: Vec<AnyElement>,
}

impl RadioGroup {
    /// A vertical group with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            axis: Axis::Vertical,
            children: Vec::new(),
        }
    }

    /// Lays the radios out in a row.
    pub fn horizontal(mut self) -> Self {
        self.axis = Axis::Horizontal;
        self
    }

    /// Which way the radios run.
    pub fn axis(&self) -> Axis {
        self.axis
    }
}

impl Styled for RadioGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for RadioGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

/// Moves the choice to the next (or previous) radio of the group that can
/// take focus, wrapping at the ends, the way a native radio group does: the
/// focus moves, and the radio that gets it is activated as Space would.
fn step(group: &FocusHandle, forward: bool, window: &mut Window, cx: &mut App) {
    if !group.contains_focused(window, cx) {
        return;
    }
    // Past either end the tab order goes on around the window, and comes
    // back into the group at its other end.
    for _ in 0..64 {
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        if group.contains_focused(window, cx) {
            break;
        }
    }
    if !group.contains_focused(window, cx) {
        return;
    }
    for event in [
        PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse("space").expect("a valid keystroke"),
            is_held: false,
            prefer_character_input: false,
        }),
        PlatformInput::KeyUp(KeyUpEvent {
            keystroke: Keystroke::parse("space").expect("a valid keystroke"),
        }),
    ] {
        window.dispatch_event(event, cx);
    }
}

impl RenderOnce for RadioGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let gap = cx.theme().base.spacing.md;
        let group = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let keys = group.clone();
        div()
            .id(self.id)
            .track_focus(&group)
            .on_key_down(move |event, window, cx| {
                let forward = match event.keystroke.key.as_str() {
                    "down" | "right" => true,
                    "up" | "left" => false,
                    _ => return,
                };
                if event.keystroke.modifiers.modified() {
                    return;
                }
                cx.stop_propagation();
                let keys = keys.clone();
                window.defer(cx, move |window, cx| step(&keys, forward, window, cx));
            })
            .role(Role::RadioGroup)
            .aria_orientation(match self.axis {
                Axis::Horizontal => Orientation::Horizontal,
                Axis::Vertical => Orientation::Vertical,
            })
            .test_support()
            .flex()
            .when(self.axis == Axis::Vertical, |this| this.flex_col())
            .when(self.axis == Axis::Horizontal, |this| this.items_center())
            .gap(gap)
            .refine_style(&self.style)
            .children(self.children)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_radio_fills_with_the_color_of_an_on_switch() {
        use crate::theme::{ThemeConfig, to_hex};
        use gpui_kit::base::ThemeAppearance;
        for (config, appearance, hex) in [
            (ThemeConfig::dark(), ThemeAppearance::Dark, "#539af8"),
            (ThemeConfig::light(), ThemeAppearance::Light, "#339cff"),
        ] {
            let theme = crate::theme::test_tokens(&config, appearance);
            let (ring, fill) = colors(true, &theme);
            assert_eq!(fill.map(to_hex).as_deref(), Some(hex));
            assert_eq!(to_hex(ring), hex);
            assert_eq!(
                fill,
                Some(theme.switch_track_on),
                "the switch's track token"
            );
            assert_eq!(colors(false, &theme), (theme.field_border, None));
        }
    }

    #[test]
    fn builders_read_back() {
        assert!(Radio::new("r").checked(true).is_checked());
        assert!(!Radio::new("r").is_checked());
        assert!(RadioMark::new(true).is_checked());
        assert_eq!(RadioGroup::new("g").axis(), Axis::Vertical);
        assert_eq!(RadioGroup::new("g").horizontal().axis(), Axis::Horizontal);
    }
}
