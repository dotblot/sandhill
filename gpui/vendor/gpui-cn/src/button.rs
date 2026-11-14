use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, FocusHandle, Hsla, InteractiveElement, Interactivity,
    IntoElement, ParentElement, Pixels, RenderOnce, Role, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window,
    base::{
        self, Disableable, ElementExt as _, Interpolate, Placement, Selectable, StyledExt as _,
        transition,
    },
    div,
    prelude::FluentBuilder as _,
    px, relative,
};

use crate::{
    ActiveTheme as _, Icon, Spinner, Theme, ThemeTokens, TooltipHost,
    theme::mix,
    tooltip::{TooltipTrigger, TruncatedLabel},
};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type HoverHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// The visual treatment of a [`Button`], by meaning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// An ordinary action: a soft filled surface with the foreground as
    /// text.
    #[default]
    Default,
    /// The one emphasized commitment in a decision area: the theme's
    /// `primary` fill.
    Primary,
    /// A destructive commitment.
    Destructive,
    /// An ordinary action with a hairline boundary and no fill.
    Outline,
    /// A quiet toolbar or inline action with no surface at rest.
    Ghost,
    /// Looks like a link, in the link color, for an action that opens a
    /// resource.
    Link,
}

/// The size tier of a [`Button`]. Height, text size, icon size, and
/// padding change together. Heights are given at the default 16px rem and
/// scale with the theme's UI font size.
///
/// The tiers are the reference app's, not shadcn's: 28px is its
/// most common control height and 32px its largest. Any tier can be used
/// anywhere. An icon-only button (an icon and no label or children) is a
/// square of its tier's height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// 28px, 13px text.
    #[default]
    Default,
    /// 20px, 12px text.
    Xs,
    /// 24px, 13px text.
    Sm,
    /// 32px, 14px text, the large radius.
    Lg,
}

/// The box a control of one size tier takes: shared by the button and
/// the select trigger, so a trigger beside a button lines up with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ControlGeometry {
    pub radius: Pixels,
    pub height: Pixels,
    pub padding: Pixels,
    pub text_size: Pixels,
}

impl ButtonSize {
    pub(crate) fn geometry(self, theme: &ThemeTokens) -> ControlGeometry {
        let metrics = &theme.metrics;
        ControlGeometry {
            radius: if self == Self::Lg {
                theme.radius_lg()
            } else {
                theme.radius_md()
            },
            height: match self {
                Self::Xs => metrics.control_xs,
                Self::Sm => metrics.control_sm,
                Self::Default => metrics.control_md,
                Self::Lg => metrics.control_lg,
            },
            padding: match self {
                Self::Xs => metrics.control_padding_xs,
                Self::Sm => metrics.control_padding_sm,
                Self::Default => metrics.control_padding_md,
                Self::Lg => metrics.control_padding_lg,
            },
            text_size: match self {
                Self::Xs => theme.base.typography.xs.size,
                Self::Sm | Self::Default => theme.text_control.size,
                Self::Lg => theme.base.typography.sm.size,
            },
        }
    }

    fn is_xs(self) -> bool {
        matches!(self, Self::Xs)
    }

    /// The tiers that use the smaller icon.
    fn is_compact(self) -> bool {
        matches!(self, Self::Xs | Self::Sm)
    }
}

/// A shadcn-style button on `gpui_base::Button`.
///
/// Base owns focus, Tab order, Enter and Space activation, the disabled
/// contract, and the accessibility role and name. This type owns the look:
/// variant colors, size geometry, icon and label layout, the animated hover
/// and pressed surfaces, and the tooltip.
///
/// ```
/// use gpui_cn::{Button, ButtonSize, ButtonVariant, Icon};
///
/// let _ = Button::new("save").label("Save").on_click(|_, _, _| {});
/// let _ = Button::new("create").primary().label("Create");
/// let _ = Button::new("delete").destructive().label("Delete");
/// let _ = Button::new("more")
///     .ghost()
///     .icon(Icon::new("icons/ellipsis.svg"))
///     .accessibility_label("More")
///     .tooltip("More");
/// ```
///
/// The id keys focus, hover state, and motion, so it must be stable across
/// frames and unique under the nearest stateful ancestor.
///
/// `Styled` refinements (`.w_full()`, `.rounded_full()`, `.h(..)`) apply
/// after the variant and size styles, so they win. Every component panics
/// when rendered before [`crate::init`].
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    base: base::Button,
    style: StyleRefinement,
    variant: ButtonVariant,
    size: ButtonSize,
    label: Option<SharedString>,
    icon: Option<Icon>,
    trailing_icon: Option<Icon>,
    children: Vec<AnyElement>,
    disabled: bool,
    loading: bool,
    selected: bool,
    open: bool,
    toggled: Option<bool>,
    accessibility_label: Option<SharedString>,
    tooltip: Option<(SharedString, Option<Placement>)>,
    truncation_tooltip: bool,
    on_click: Option<ClickHandler>,
    on_hover: Option<HoverHandler>,
    focus_handle: Option<FocusHandle>,
    tab_index: isize,
    tab_stop: bool,
    /// The role assistive technology reads, when a composite such as a
    /// menu bar gives its buttons one of its own.
    role: Option<Role>,
    /// Whether the popup this button opens is open, for assistive
    /// technology, when the button is known to open one.
    expanded: Option<bool>,
}

impl Button {
    /// A button with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            base: base::Button::new(id.clone()),
            id,
            style: StyleRefinement::default(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            label: None,
            icon: None,
            trailing_icon: None,
            children: Vec::new(),
            disabled: false,
            loading: false,
            selected: false,
            open: false,
            toggled: None,
            accessibility_label: None,
            tooltip: None,
            truncation_tooltip: true,
            on_click: None,
            on_hover: None,
            focus_handle: None,
            tab_index: 0,
            tab_stop: true,
            role: None,
            expanded: None,
        }
    }

    /// The visual treatment.
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    /// [`ButtonVariant::Primary`].
    pub fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    /// [`ButtonVariant::Destructive`].
    pub fn destructive(self) -> Self {
        self.variant(ButtonVariant::Destructive)
    }

    /// [`ButtonVariant::Outline`].
    pub fn outline(self) -> Self {
        self.variant(ButtonVariant::Outline)
    }

    /// [`ButtonVariant::Ghost`].
    pub fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    /// [`ButtonVariant::Link`].
    pub fn link(self) -> Self {
        self.variant(ButtonVariant::Link)
    }

    /// The size tier.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// The visible text. It is also the accessible name unless
    /// [`accessibility_label`](Self::accessibility_label) is set.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An icon before the label. With no label and no children the button
    /// is an icon-only square.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// An icon after the label.
    pub fn trailing_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.trailing_icon = Some(icon.into());
        self
    }

    /// The name a screen reader announces when the visible content is not
    /// it: an icon-only button, or a button whose content is not text.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Exposes the button as a toggle to assistive technology, with `toggled`
    /// as its pressed state. Pair it with [`Selectable::selected`] for the
    /// pressed look.
    pub fn toggled(mut self, toggled: bool) -> Self {
        self.toggled = Some(toggled);
        self
    }

    /// Marks the button busy with work it started: a [`Spinner`] takes the
    /// leading icon's place (or comes before the label when there is no
    /// icon), and activation does nothing until the work ends. Unlike
    /// `disabled`, the button keeps its colors and its focus.
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Whether the button is busy.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// A text tooltip. Icon-only buttons should have one. A button with a
    /// label and no tooltip shows the full label as its tooltip whenever
    /// the label is truncated (through the window's tooltip overlay).
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some((text.into(), None));
        self
    }

    /// Whether a truncated label shows the full text as a tooltip. On by
    /// default; an explicit [`tooltip`](Self::tooltip) replaces it.
    pub fn truncation_tooltip(mut self, enabled: bool) -> Self {
        self.truncation_tooltip = enabled;
        self
    }

    /// A text tooltip that prefers a side.
    pub fn tooltip_at(mut self, placement: Placement, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some((text.into(), Some(placement)));
        self
    }

    /// The activation handler for pointer, Enter, and Space.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Called when the pointer enters (`true`) or leaves (`false`).
    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }

    /// Uses a caller-owned focus handle instead of one keyed by the id, so
    /// the owner can focus the button or ask whether it is focused.
    pub fn track_focus(mut self, focus_handle: &FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle.clone());
        self
    }

    /// The focus traversal index. The default is `0`.
    pub fn tab_index(mut self, tab_index: isize) -> Self {
        self.tab_index = tab_index;
        self
    }

    /// Whether Tab reaches the button. The default is `true`.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// The role a composite gives its buttons, such as a menu bar's items.
    pub(crate) fn with_role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// Tells assistive technology whether the popup this button opens is
    /// open, for a composite whose buttons always open one.
    pub(crate) fn with_expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    /// Whether the button is an icon with no label or children, and so
    /// renders as a square.
    pub fn is_icon_only(&self) -> bool {
        self.icon.is_some() && self.label.is_none() && self.children.is_empty()
    }

    /// Whether the button accepts the pointer and activation: not while
    /// it is disabled, and not while it is busy.
    fn is_interactive(&self) -> bool {
        !self.disabled && !self.loading
    }
}

impl Disableable for Button {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }

    /// The state a popover, menu, or dropdown holds on its trigger while it
    /// is open. Paints like `selected`, and is kept apart from it because
    /// the two mean different things.
    fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    fn is_open(&self) -> bool {
        self.open
    }
}

impl Styled for Button {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Button {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

/// The colors of one variant in one state, mixed as one value so the
/// three channels always agree.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Surface {
    background: Hsla,
    foreground: Hsla,
    border: Hsla,
}

impl Surface {
    fn plain(background: Hsla, foreground: Hsla) -> Self {
        Self {
            background,
            foreground,
            border: gpui_kit::transparent_black(),
        }
    }
}

/// Interpolates each channel of each color. A rest state with no fill uses
/// the hover color at zero alpha (not transparent black), so only the alpha
/// moves and no gray passes through mid-transition.
impl Interpolate for Surface {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        // Weighted this way, progress 0 and 1 give exactly `from` and `to`
        // on every platform, so whole state weights paint the state's own
        // colors.
        let weigh = |from: f32, to: f32| from * (1. - progress) + to * progress;
        let lerp = |from: Hsla, to: Hsla| Hsla {
            h: weigh(from.h, to.h),
            s: weigh(from.s, to.s),
            l: weigh(from.l, to.l),
            a: weigh(from.a, to.a),
        };
        Self {
            background: lerp(self.background, target.background),
            foreground: lerp(self.foreground, target.foreground),
            border: lerp(self.border, target.border),
        }
    }
}

/// What the pointer is doing to the button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerState {
    Rest,
    Hovered,
    Pressed,
}

/// How far the button is into each state, from 0 to 1. The weights
/// animate and the colors are mixed from the current theme each frame,
/// so a theme change repaints at once instead of fading from the old
/// theme's colors.
#[derive(Clone, Copy, Debug, PartialEq)]
struct StateWeights {
    hovered: f32,
    pressed: f32,
    selected: f32,
    disabled: f32,
}

impl StateWeights {
    fn of(pointer: PointerState, selected: bool, disabled: bool) -> Self {
        let weight = |on: bool| if on { 1. } else { 0. };
        Self {
            hovered: weight(pointer != PointerState::Rest),
            pressed: weight(pointer == PointerState::Pressed),
            selected: weight(selected),
            disabled: weight(disabled),
        }
    }
}

impl Interpolate for StateWeights {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        let lerp = |from: f32, to: f32| from + (to - from) * progress;
        Self {
            hovered: lerp(self.hovered, target.hovered),
            pressed: lerp(self.pressed, target.pressed),
            selected: lerp(self.selected, target.selected),
            disabled: lerp(self.disabled, target.disabled),
        }
    }
}

impl ButtonVariant {
    /// The surface for a pointer state, with `selected` for a persistent
    /// pressed, checked, or open look on the variants that have a surface.
    ///
    /// Every surface carries a border color so it can animate. Only
    /// `Outline` paints a border width.
    ///
    /// Inside a sidebar the soft and selected fills are the sidebar's, as
    /// shadcn remaps its variables there, so a hover reads on the lifted
    /// surface. A selected surface still answers the pointer: it lifts a
    /// step on hover and another when pressed.
    fn surface(
        self,
        state: PointerState,
        selected: bool,
        in_sidebar: bool,
        theme: &ThemeTokens,
    ) -> Surface {
        use PointerState::*;
        let dark = theme.is_dark();
        let ink = theme.foreground();
        let (surface, soft, selected_rest) = if in_sidebar {
            (theme.sidebar, theme.sidebar_accent, theme.sidebar_selected)
        } else {
            (theme.background(), theme.secondary(), theme.selected)
        };
        // The soft fill steps toward the ink as the pointer presses.
        let soft_hover = mix(soft, ink, 0.05);
        let soft_pressed = mix(soft, ink, 0.10);
        let selected_fill = match state {
            Rest => selected_rest,
            Hovered => mix(selected_rest, ink, 0.03),
            Pressed => mix(selected_rest, ink, 0.06),
        };
        match self {
            Self::Default => {
                if selected {
                    return Surface::plain(selected_fill, ink);
                }
                let fill = match state {
                    Rest => soft,
                    Hovered => soft_hover,
                    Pressed => soft_pressed,
                };
                Surface::plain(fill, ink)
            }
            Self::Primary => {
                let primary = theme.primary();
                let fill = match state {
                    Rest => primary,
                    Hovered => mix(primary, surface, 0.09),
                    Pressed => mix(primary, surface, 0.18),
                };
                Surface::plain(fill, theme.primary_foreground())
            }
            Self::Destructive => {
                // A tint of the destructive color with the color as text,
                // not a solid fill: the reference keeps the alarm quiet
                // until the pointer commits to it.
                let red = theme.destructive();
                let fill = match state {
                    Rest => theme.destructive_tint,
                    Hovered => red.opacity(if dark { 0.3 } else { 0.16 }),
                    Pressed => red.opacity(if dark { 0.4 } else { 0.22 }),
                };
                Surface::plain(fill, red)
            }
            Self::Outline => {
                let hover = soft;
                let background = if selected {
                    selected_fill
                } else {
                    match state {
                        Rest => hover.alpha(0.),
                        Hovered => hover,
                        Pressed => soft_hover,
                    }
                };
                Surface {
                    background,
                    foreground: ink,
                    border: theme.input(),
                }
            }
            Self::Ghost => {
                // Quiet at rest, as the reference draws unselected segments
                // and row icons: muted text, no surface. Hover and selection
                // bring the foreground and a fill.
                if selected {
                    return Surface::plain(selected_fill, ink);
                }
                match state {
                    Rest => Surface::plain(soft.alpha(0.), theme.muted_foreground()),
                    Hovered => Surface::plain(soft, ink),
                    Pressed => Surface::plain(soft_hover, ink),
                }
            }
            Self::Link => Surface::plain(gpui_kit::transparent_black(), theme.link),
        }
    }

    /// The surface at `weights`, mixed from the surfaces of the current
    /// theme. Whole weights give exactly the surface of that state.
    fn surface_at(self, weights: StateWeights, in_sidebar: bool, theme: &ThemeTokens) -> Surface {
        use PointerState::*;
        let at = |selected: bool| {
            let surface = |state| self.surface(state, selected, in_sidebar, theme);
            surface(Rest)
                .interpolate(&surface(Hovered), weights.hovered)
                .interpolate(&surface(Pressed), weights.pressed)
        };
        let enabled = at(false).interpolate(&at(true), weights.selected);
        if weights.disabled == 0. {
            return enabled;
        }
        enabled.interpolate(&self.disabled(enabled, theme), weights.disabled)
    }

    /// `surface` as a disabled button paints it. The reference keeps a
    /// disabled button's fill at half strength and drops its text most of
    /// the way to the surface (#6e6e6e on #181818), rather than fading the
    /// whole control.
    fn disabled(self, surface: Surface, theme: &ThemeTokens) -> Surface {
        Surface {
            background: surface.background.opacity(theme.disabled_opacity),
            border: surface.border.opacity(theme.disabled_opacity),
            foreground: match self {
                ButtonVariant::Primary => surface.foreground,
                _ => mix(
                    theme.foreground(),
                    theme.background(),
                    if theme.is_dark() { 0.52 } else { 0.64 },
                ),
            },
        }
    }
}

/// Everything the render needs from the theme, read in one borrow so the
/// tokens are never cloned.
struct Look {
    surface: Surface,
    underline: bool,
    ring: Hsla,
    ring_spread: Pixels,
    outline_focus_border: Hsla,
    geometry: ControlGeometry,
}

/// Sizes an icon, or the spinner in its place, to the button's tier.
fn icon_size_rem<E: Styled>(icon: E, size: ButtonSize) -> E {
    if size.is_xs() {
        icon.size_3()
    } else if size.is_compact() {
        icon.size_3p5()
    } else {
        icon.size_4()
    }
}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let settings = Theme::global(cx);
        let fast = settings.motion.fast_transition();
        let pointer_cursors = settings.pointer_cursors;

        let interactive = self.is_interactive();
        let icon_only = self.is_icon_only();
        let variant = self.variant;
        let size = self.size;
        let disabled = self.disabled;
        let shows_selected = self.selected || self.open;

        // The pointer state lives in keyed element state so the surface can
        // animate toward it. GPUI's own `hover`/`active` refinements would
        // flip instantly.
        let pointer = window.use_keyed_state(
            ElementId::NamedChild(self.id.clone().into(), "pointer".into()),
            cx,
            |_, _| PointerState::Rest,
        );
        if !interactive && *pointer.read(cx) != PointerState::Rest {
            // A button that is disabled while hovered or pressed must not
            // come back painted that way.
            pointer.update(cx, |state, _| *state = PointerState::Rest);
        }
        // A finger cannot hover. GPUI still reports the last tap's point
        // as hovered, so on touch the hover state paints as rest; a press
        // still paints as pressed.
        let touch = cx.theme().touch;
        let pointer_state = match *pointer.read(cx) {
            PointerState::Hovered if touch => PointerState::Rest,
            state => state,
        };

        let in_sidebar = crate::sidebar::in_sidebar(cx);
        let weights = transition(
            ElementId::NamedChild(self.id.clone().into(), "state".into()),
            StateWeights::of(pointer_state, shows_selected, disabled),
            fast,
            window,
            cx,
        );
        let look =
            {
                let theme = cx.theme();
                Look {
                    surface: variant.surface_at(weights, in_sidebar, theme),
                    underline: variant == ButtonVariant::Link
                        && (shows_selected || pointer_state != PointerState::Rest),
                    ring: match variant {
                        ButtonVariant::Destructive => theme
                            .destructive()
                            .opacity(if theme.is_dark() { 0.4 } else { 0.2 }),
                        _ => theme.focus_ring(),
                    },
                    ring_spread: theme.metrics.focus_ring,
                    outline_focus_border: theme.ring(),
                    geometry: size.geometry(theme),
                }
            };
        let surface = look.surface;

        let focus_handle = self.focus_handle.clone().unwrap_or_else(|| {
            window
                .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone()
        });
        let focus_visible = focus_handle.is_focused(window) && window.last_input_was_keyboard();

        let spinner = self.loading.then(|| {
            icon_size_rem(
                Spinner::new(ElementId::NamedChild(
                    self.id.clone().into(),
                    "spinner".into(),
                )),
                size,
            )
        });

        let accessibility_label = self.accessibility_label.or_else(|| self.label.clone());
        let on_click = self.on_click;
        let on_hover = self.on_hover;
        let pointer_for_hover = pointer.clone();
        let pointer_for_down = pointer.clone();
        let pointer_for_up = pointer.clone();
        let pointer_for_up_out = pointer;
        let label = self
            .label
            .map(|label| (label.clone(), TruncatedLabel::new(label).into_parts()));
        let has_overlay = TooltipHost::overlay(window, cx).is_some();
        let tooltip = match (self.tooltip, &label) {
            (Some((text, placement)), _) => {
                Some((TooltipTrigger::text(self.id.clone(), placement, text), None))
            }
            (None, Some((label, (_, check)))) if has_overlay && self.truncation_tooltip => Some((
                TooltipTrigger::text(self.id.clone(), None, label.clone()),
                Some(check.clone()),
            )),
            _ => None,
        };
        // GPUI allows one hover listener per element, so the tooltip rides on
        // the button's own; a window without an overlay gets the native one.
        let (managed_tooltip, native_tooltip) = match tooltip {
            Some((trigger, only_when_truncated)) if has_overlay => {
                (Some((trigger, only_when_truncated)), None)
            }
            Some((trigger, _)) => (None, Some(trigger)),
            None => (None, None),
        };
        let content = div()
            .flex()
            .items_center()
            .justify_center()
            // Base sets the button's line box to exactly the font size. The
            // label clips to its line box for truncation, so the box must be
            // taller than the glyphs or descenders are cut.
            .line_height(relative(1.25))
            .min_w_0()
            .whitespace_nowrap()
            .map(|this| {
                if size.is_xs() {
                    this.gap_1()
                } else {
                    this.gap_1p5()
                }
            })
            .map(|this| match spinner {
                Some(spinner) => this.child(spinner),
                None => this.when_some(self.icon, |this, icon| {
                    this.child(icon_size_rem(icon, size))
                }),
            })
            .when_some(label, |this, (_, (text, _))| {
                this.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(look.underline && interactive, |this| {
                            this.text_decoration_1()
                        })
                        .child(text),
                )
            })
            .children(self.children)
            .when_some(self.trailing_icon, |this, icon| {
                this.child(icon_size_rem(icon, size))
            });

        self.base
            .flex_shrink_0()
            .font_medium()
            .text_size(look.geometry.text_size)
            .rounded(look.geometry.radius)
            .h(look.geometry.height)
            .px(look.geometry.padding)
            .when(icon_only, |this| this.px_0().w(look.geometry.height))
            .bg(surface.background)
            .text_color(surface.foreground)
            // Every variant carries the same 1px border, transparent where it
            // does not show, so an outline button is exactly as large as the
            // others and the border color can animate between states.
            .border_1()
            .border_color(if focus_visible && variant == ButtonVariant::Outline {
                look.outline_focus_border
            } else {
                surface.border
            })
            .when(focus_visible, |this| {
                // A 3px spread outside the box; an `overflow_hidden` ancestor
                // that hugs the button clips it, so leave room.
                this.shadow(vec![gpui_kit::BoxShadow {
                    color: look.ring,
                    offset: gpui_kit::point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: look.ring_spread,
                    inset: false,
                }])
            })
            .map(|this| {
                if interactive && (pointer_cursors || variant == ButtonVariant::Link) {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .refine_style(&self.style)
            .role(self.role.unwrap_or(if variant == ButtonVariant::Link {
                Role::Link
            } else {
                Role::Button
            }))
            .selected(shows_selected)
            .disabled(disabled)
            .when_some(accessibility_label, |this, label| {
                this.accessibility_label(label)
            })
            .when_some(self.expanded, |this, expanded| this.aria_expanded(expanded))
            .when_some(self.toggled, |this, toggled| {
                this.aria_toggled(if toggled {
                    gpui_kit::accesskit::Toggled::True
                } else {
                    gpui_kit::accesskit::Toggled::False
                })
            })
            .track_focus(&focus_handle)
            .tab_index(self.tab_index)
            .tab_stop(self.tab_stop)
            .child(content)
            .when_some(managed_tooltip.as_ref(), |this, (trigger, _)| {
                this.on_prepaint(trigger.track_bounds())
            })
            .when_some(native_tooltip, |this, trigger| {
                this.tooltip(trigger.native())
            })
            .on_hover(move |hovered, window, cx| {
                match &managed_tooltip {
                    Some((trigger, Some(check))) => check.hovered(trigger, *hovered, window, cx),
                    Some((trigger, None)) => trigger.hovered(*hovered, window, cx),
                    None => {}
                }
                if !interactive {
                    return;
                }
                pointer_for_hover.update(cx, |state, cx| {
                    // GPUI reports "not hovered" while a press is pending
                    // and the pointer moves; a press stays pressed until the
                    // button is released.
                    let next = match (*hovered, *state) {
                        (false, PointerState::Pressed) => PointerState::Pressed,
                        (true, _) => PointerState::Hovered,
                        (false, _) => PointerState::Rest,
                    };
                    if *state != next {
                        *state = next;
                        cx.notify();
                    }
                });
                if let Some(on_hover) = &on_hover {
                    on_hover(hovered, window, cx);
                }
            })
            .when(interactive, |this| {
                this.on_mouse_down(gpui_kit::MouseButton::Left, move |_, window, cx| {
                    // Pressing must not move focus with the pointer, and must
                    // not start the window text selection.
                    window.prevent_default();
                    base::GlobalState::suppress_text_selection(cx);
                    TooltipTrigger::pressed(window, cx);
                    pointer_for_down.update(cx, |state, cx| {
                        *state = PointerState::Pressed;
                        cx.notify();
                    });
                })
                .on_mouse_up(gpui_kit::MouseButton::Left, move |_, _, cx| {
                    pointer_for_up.update(cx, |state, cx| {
                        if *state == PointerState::Pressed {
                            *state = PointerState::Hovered;
                            cx.notify();
                        }
                    });
                })
                .on_mouse_up_out(gpui_kit::MouseButton::Left, move |_, _, cx| {
                    pointer_for_up_out.update(cx, |state, cx| {
                        if *state != PointerState::Rest {
                            *state = PointerState::Rest;
                            cx.notify();
                        }
                    });
                })
            })
            .when_some(on_click.filter(|_| interactive), |this, on_click| {
                this.on_click(move |event, window, cx| on_click(event, window, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeConfig, to_hex};
    use gpui_kit::base::ThemeAppearance;

    fn dark() -> ThemeTokens {
        crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark)
    }

    #[test]
    fn whole_weights_mix_to_the_surface_of_that_state() {
        let theme = dark();
        let ghost = ButtonVariant::Ghost;
        for pointer in [
            PointerState::Rest,
            PointerState::Hovered,
            PointerState::Pressed,
        ] {
            for selected in [false, true] {
                let weights = StateWeights::of(pointer, selected, false);
                assert_eq!(
                    ghost.surface_at(weights, true, &theme),
                    ghost.surface(pointer, selected, true, &theme),
                    "{pointer:?}, selected {selected}"
                );
            }
        }
        let selected = ghost.surface(PointerState::Rest, true, false, &theme);
        assert_eq!(
            ghost.surface_at(
                StateWeights::of(PointerState::Rest, true, true),
                false,
                &theme
            ),
            ghost.disabled(selected, &theme),
        );
    }

    #[test]
    fn primary_reads_the_theme_tokens() {
        let theme = dark();
        let rest = ButtonVariant::Primary.surface(PointerState::Rest, false, false, &theme);
        assert_eq!(to_hex(rest.background), "#dfdfdf");
        assert_eq!(to_hex(rest.foreground), "#2d2d2d");
        let hovered = ButtonVariant::Primary.surface(PointerState::Hovered, false, false, &theme);
        assert!(hovered.background.l < rest.background.l);
    }

    #[test]
    fn rest_surfaces_without_a_fill_share_the_hover_color_at_zero_alpha() {
        let theme = dark();
        for variant in [ButtonVariant::Outline, ButtonVariant::Ghost] {
            let rest = variant.surface(PointerState::Rest, false, false, &theme);
            let hovered = variant.surface(PointerState::Hovered, false, false, &theme);
            assert_eq!(rest.background.a, 0., "{variant:?} rest is clear");
            assert_eq!(
                rest.background.l, hovered.background.l,
                "{variant:?} only alpha moves"
            );
            let mid = rest.interpolate(&hovered, 0.5);
            assert_eq!(mid.background.l, hovered.background.l);
            assert!((mid.background.a - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn default_soft_fill_matches_the_reference() {
        let theme = dark();
        let rest = ButtonVariant::Default.surface(PointerState::Rest, false, false, &theme);
        assert_eq!(to_hex(rest.background), "#222222");
        assert_eq!(to_hex(rest.foreground), "#dfdfdf");
        let ghost = ButtonVariant::Ghost.surface(PointerState::Rest, false, false, &theme);
        assert_eq!(to_hex(ghost.foreground), "#969696");
    }

    #[test]
    fn inside_a_sidebar_the_fills_are_the_sidebars() {
        let theme = dark();
        let hovered = ButtonVariant::Ghost.surface(PointerState::Hovered, false, true, &theme);
        assert_eq!(hovered.background, theme.sidebar_accent);
        let selected = ButtonVariant::Ghost.surface(PointerState::Rest, true, true, &theme);
        assert_eq!(selected.background, theme.sidebar_selected);
        assert!(hovered.background.l < selected.background.l);
        let selected_hovered =
            ButtonVariant::Ghost.surface(PointerState::Hovered, true, true, &theme);
        assert!(selected_hovered.background.l > selected.background.l);
        let window = ButtonVariant::Ghost.surface(PointerState::Hovered, false, false, &theme);
        assert_eq!(window.background, theme.secondary());
    }
}
