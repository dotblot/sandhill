use std::rc::Rc;

use gpui_kit::{
    App, ElementId, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    StyleRefinement, Styled, Window,
    base::{self, Disableable, Interpolate, StyledExt as _, transition},
    point,
    prelude::FluentBuilder as _,
    px,
};

use crate::{ActiveTheme as _, Theme, ThemeTokens};

type ChangeHandler = Rc<dyn Fn(&bool, &mut Window, &mut App)>;

/// A shadcn-style switch on `gpui_base::Switch`: a pill track with a
/// round thumb that slides to the right when checked.
///
/// Base owns focus, Tab order, Enter and Space activation, the disabled
/// contract, and the switch role with its toggled state. This type owns
/// the look: the track colors, the thumb, the slide, the focus ring, and
/// the disabled strength.
///
/// The checked value is the application's. Activation reports the next
/// value through [`on_change`](Self::on_change), and the application
/// renders it back through [`checked`](Self::checked).
///
/// ```
/// use gpui_cn::Switch;
///
/// let _ = Switch::new("wifi")
///     .checked(true)
///     .accessibility_label("Wi-Fi")
///     .on_change(|checked, _, _| println!("{checked}"));
/// ```
///
/// The id keys focus and motion, so it must be stable across frames and
/// unique under the nearest stateful ancestor.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    style: StyleRefinement,
    checked: bool,
    disabled: bool,
    accessibility_label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    tab_index: isize,
    tab_stop: bool,
}

impl Switch {
    /// A switch with a stable id, off.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            checked: false,
            disabled: false,
            accessibility_label: None,
            on_change: None,
            tab_index: 0,
            tab_stop: true,
        }
    }

    /// Whether the switch is on.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Whether the switch is on.
    pub fn is_checked(&self) -> bool {
        self.checked
    }

    /// The name a screen reader announces. A switch has no visible text of
    /// its own, so every switch should have one.
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    /// Called with the next value when the switch is activated by pointer,
    /// Enter, or Space. The value comes by reference so `cx.listener` fits.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// The focus traversal index. The default is `0`.
    pub fn tab_index(mut self, tab_index: isize) -> Self {
        self.tab_index = tab_index;
        self
    }

    /// Whether Tab reaches the switch. The default is `true`.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }
}

impl Disableable for Switch {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl Styled for Switch {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// How far the switch is into checked and disabled, from 0 to 1. The
/// progress animates and the track is mixed from the current theme each
/// frame, so a theme change repaints at once instead of fading from the
/// old theme's colors.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Progress {
    checked: f32,
    disabled: f32,
}

impl Progress {
    fn of(checked: bool, disabled: bool) -> Self {
        let weight = |on: bool| if on { 1. } else { 0. };
        Self {
            checked: weight(checked),
            disabled: weight(disabled),
        }
    }
}

impl Interpolate for Progress {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        let lerp = |from: f32, to: f32| from + (to - from) * progress;
        Self {
            checked: lerp(self.checked, target.checked),
            disabled: lerp(self.disabled, target.disabled),
        }
    }
}

/// The look that follows the progress: the track color and where the
/// thumb sits, measured from the track's padding edge.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Surface {
    track: Hsla,
    thumb_offset: Pixels,
}

impl Interpolate for Surface {
    fn interpolate(&self, target: &Self, progress: f32) -> Self {
        Self {
            track: self.track.interpolate(&target.track, progress),
            thumb_offset: self
                .thumb_offset
                .interpolate(&target.thumb_offset, progress),
        }
    }
}

impl Surface {
    /// The track and thumb position for `checked`. A disabled track keeps
    /// its color at the disabled strength, over whatever is behind it.
    fn of(checked: bool, disabled: bool, theme: &ThemeTokens) -> Self {
        let metrics = &theme.metrics;
        let track = if checked {
            theme.switch_track_on
        } else {
            theme.switch_track_off
        };
        Self {
            track: if disabled {
                track.opacity(theme.disabled_opacity)
            } else {
                track
            },
            thumb_offset: if checked {
                metrics.switch_track_width
                    - metrics.switch_thumb_inset * 2.
                    - metrics.switch_thumb_size
            } else {
                px(0.)
            },
        }
    }

    /// The look at `progress`, mixed from the looks of the current theme.
    /// Whole progress gives exactly the look of that state.
    fn at(progress: Progress, theme: &ThemeTokens) -> Self {
        let checked = |disabled| {
            Self::of(false, disabled, theme)
                .interpolate(&Self::of(true, disabled, theme), progress.checked)
        };
        checked(false).interpolate(&checked(true), progress.disabled)
    }
}

/// The thumb color. GPUI fades each quad on its own, so a disabled thumb
/// cannot fade as a group with its track: it would show the track through
/// itself. The reference composites the whole control over its card, so
/// the disabled thumb is painted as the thumb at the disabled strength
/// over the card surface (#a7a7a7 on dark).
fn thumb_color(disabled: bool, theme: &ThemeTokens) -> Hsla {
    if disabled {
        // `blend` is the sRGB source-over the GPU applies, so the flat
        // thumb matches what a faded one would have shown over the card.
        theme
            .base
            .colors
            .surface
            .blend(theme.switch_thumb.opacity(theme.disabled_opacity))
    } else {
        theme.switch_thumb
    }
}

/// Everything the render needs from the theme, read in one borrow.
struct Look {
    thumb: Hsla,
    width: Pixels,
    height: Pixels,
    thumb_size: Pixels,
    inset: Pixels,
    hit_height: Pixels,
    ring: Hsla,
    ring_spread: Pixels,
    radius: Pixels,
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let settings = Theme::global(cx);
        let slide = settings.motion.slide_transition();
        let pointer_cursors = settings.pointer_cursors;
        let checked = self.checked;
        let disabled = self.disabled;
        let progress = transition(
            ElementId::NamedChild(self.id.clone().into(), "progress".into()),
            Progress::of(checked, disabled),
            slide,
            window,
            cx,
        );
        let (look, surface) = {
            let theme = cx.theme();
            let metrics = &theme.metrics;
            (
                Look {
                    thumb: thumb_color(disabled, theme),
                    width: metrics.switch_track_width,
                    height: metrics.switch_track_height,
                    thumb_size: metrics.switch_thumb_size,
                    inset: metrics.switch_thumb_inset,
                    // On touch the track is the platform's own size and
                    // the switch takes the row's hit height around it.
                    hit_height: if theme.touch {
                        metrics.control_md
                    } else {
                        metrics.switch_track_height
                    },
                    ring: theme.focus_ring(),
                    ring_spread: metrics.focus_ring,
                    radius: theme.radius_full(),
                },
                Surface::at(progress, theme),
            )
        };

        // The switch owns its focus handle and hands it to base, so it can
        // tell whether the ring shows.
        let focus_handle = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "focus".into()),
                cx,
                |_, cx| cx.focus_handle(),
            )
            .read(cx)
            .clone();
        let focus_visible = focus_handle.is_focused(window) && window.last_input_was_keyboard();

        let track_id = ElementId::NamedChild(self.id.clone().into(), "track".into());
        let on_change = self.on_change;
        base::Switch::new(self.id)
            .track_focus(&focus_handle)
            .checked(checked)
            .disabled(disabled)
            .tab_index(self.tab_index)
            .tab_stop(self.tab_stop)
            .when_some(self.accessibility_label, |this, label| {
                this.accessibility_label(label)
            })
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .w(look.width)
            .h(look.hit_height)
            .map(|this| {
                if !disabled && pointer_cursors {
                    this.cursor_pointer()
                } else {
                    this.cursor_default()
                }
            })
            .refine_style(&self.style)
            .child(
                base::SwitchTrack::new(track_id)
                    .checked(checked)
                    .disabled(disabled)
                    .flex()
                    .items_center()
                    .w(look.width)
                    .h(look.height)
                    .p(look.inset)
                    .rounded(look.radius)
                    .bg(surface.track)
                    .when(focus_visible, |this| {
                        this.shadow(vec![gpui_kit::BoxShadow {
                            color: look.ring,
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: look.ring_spread,
                            inset: false,
                        }])
                    })
                    .child(
                        base::SwitchThumb::new(checked)
                            .disabled(disabled)
                            .flex_shrink_0()
                            .size(look.thumb_size)
                            .ml(surface.thumb_offset)
                            .rounded(look.radius)
                            .bg(look.thumb),
                    ),
            )
            .when_some(on_change.filter(|_| !disabled), |this, on_change| {
                this.on_change(move |next, _, window, cx| on_change(&next, window, cx))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeConfig, to_hex};
    use gpui_kit::base::ThemeAppearance;

    #[test]
    fn the_thumb_travels_the_track_minus_the_insets() {
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let off = Surface::of(false, false, &theme);
        let on = Surface::of(true, false, &theme);
        assert_eq!(off.thumb_offset, px(0.));
        assert_eq!(on.thumb_offset, px(12.), "32 - 2 * 2 - 16");
        assert_eq!(to_hex(on.track), "#539af8", "the accent");
        let light = crate::theme::test_tokens(&ThemeConfig::light(), ThemeAppearance::Light);
        assert_eq!(
            to_hex(Surface::of(true, false, &light).track),
            "#339cff",
            "the accent on light"
        );
        assert_eq!(off.track, theme.switch_track_off);
        let mid = off.interpolate(&on, 0.5);
        assert_eq!(mid.thumb_offset, px(6.));
    }

    #[test]
    fn whole_progress_gives_the_look_of_that_state() {
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        for checked in [false, true] {
            for disabled in [false, true] {
                assert_eq!(
                    Surface::at(Progress::of(checked, disabled), &theme),
                    Surface::of(checked, disabled, &theme),
                    "checked {checked}, disabled {disabled}"
                );
            }
        }
        let half = Surface::at(
            Progress {
                checked: 0.5,
                disabled: 0.,
            },
            &theme,
        );
        assert_eq!(half.thumb_offset, px(6.));
    }

    #[test]
    fn a_disabled_switch_fades_the_track_and_flattens_the_thumb() {
        // The reference's disabled toggle on dark: the accent and white at
        // the disabled strength over the #232323 card.
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let on = Surface::of(true, true, &theme);
        assert_eq!(on.track.a, theme.disabled_opacity);
        assert_eq!(to_hex(on.track.alpha(1.)), "#539af8");
        assert_eq!(thumb_color(false, &theme).a, 1.);
        let thumb = thumb_color(true, &theme);
        assert_eq!(thumb.a, 1., "flat, so the track does not show through");
        assert_eq!(to_hex(thumb), "#9c9c9c");
    }

    #[test]
    fn builders_read_back() {
        let switch = Switch::new("s").checked(true);
        assert!(switch.is_checked());
        assert!(!Switch::new("s").is_checked());
    }
}
