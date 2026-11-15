//! The geometry and colors every menu panel shares: a select's menu, a
//! dropdown menu, and a context menu draw from the same look, so their
//! panels and rows cannot drift apart.

use std::ops::Range;
use std::time::Duration;

use gpui_kit::{
    AnyElement, App, BoxShadow, Div, ElementId, Entity, Hsla, IntoElement, ParentElement as _,
    Pixels, SharedString, Styled as _, Window,
    assets::IconName,
    base::{
        Presence, PresencePhase, Transition, h_flex,
        input::{Input, InputEditorStyle, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, rems,
};

use crate::{Icon, Theme, ThemeTokens, touch_selection};

/// Everything a menu reads from the theme, in one borrow.
#[derive(Clone)]
pub(crate) struct MenuLook {
    pub(crate) surface: Hsla,
    pub(crate) border: Hsla,
    pub(crate) accent: Hsla,
    pub(crate) separator: Hsla,
    pub(crate) foreground: Hsla,
    pub(crate) muted_foreground: Hsla,
    pub(crate) description: Hsla,
    pub(crate) indicator: Hsla,
    pub(crate) destructive: Hsla,
    pub(crate) shadow: Vec<BoxShadow>,
    pub(crate) radius: Pixels,
    pub(crate) row_radius: Pixels,
    pub(crate) row_height: Pixels,
    pub(crate) row_padding: Pixels,
    pub(crate) search_height: Pixels,
    /// The menu's inset around its rows, and a separator's margin: one
    /// step of the spacing scale (`p_1`), so it follows the rem size.
    pub(crate) padding: Pixels,
    pub(crate) text_size: Pixels,
    pub(crate) line_height: Pixels,
    pub(crate) min_width: Pixels,
    pub(crate) search_min_width: Pixels,
    pub(crate) max_width: Pixels,
    pub(crate) max_height: Pixels,
    pub(crate) gap: Pixels,
    /// How far above its resting place the menu starts: two steps of the
    /// spacing scale, shadcn's `slide-in-from-top-2`.
    pub(crate) enter_offset: Pixels,
    /// Whether a finger is the pointer: a row has no hover then.
    pub(crate) touch: bool,
}

impl MenuLook {
    pub(crate) fn of(theme: &ThemeTokens, rem_size: Pixels) -> Self {
        let metrics = &theme.metrics;
        Self {
            surface: theme.popover,
            border: theme.popover_border,
            accent: theme.popover_accent,
            separator: theme.popover_separator,
            foreground: theme.popover_foreground,
            muted_foreground: theme.muted_foreground(),
            description: theme.popover_muted_foreground,
            indicator: theme.select_indicator,
            destructive: theme.destructive(),
            shadow: theme.base.shadow.md.clone(),
            radius: theme.radius_xl(),
            row_radius: theme.radius_sm(),
            row_height: metrics.row_sm,
            row_padding: metrics.control_padding_md,
            search_height: metrics.control_md,
            padding: rems(0.25).to_pixels(rem_size),
            text_size: theme.text_control.size,
            line_height: theme.text_control.line_height,
            min_width: metrics.menu_min_width,
            search_min_width: metrics.menu_search_min_width,
            max_width: metrics.menu_max_width,
            max_height: metrics.menu_max_height,
            gap: metrics.menu_gap,
            enter_offset: -rems(0.5).to_pixels(rem_size),
            touch: theme.touch,
        }
    }

    /// The height the rows can take: the menu's cap less its padding
    /// and border, and less the search field and the separator under it
    /// when the menu has one.
    pub(crate) fn rows_max_height(&self, has_search: bool) -> Pixels {
        let border = px(2.);
        let search = if has_search {
            self.search_height + self.padding * 2. + px(1.)
        } else {
            px(0.)
        };
        self.max_height - self.padding * 2. - border - search
    }

    /// The height of a palette that fills a frame of its own, with the
    /// search field and the list at their tallest: a menu's maximum height
    /// less the padding and hairline a bordered palette would add.
    pub(crate) fn palette_height(&self) -> Pixels {
        self.max_height - self.padding * 2. - px(2.)
    }

    /// The panel's shadow at `strength` of its ink.
    pub(crate) fn shadow(&self, strength: f32) -> Vec<BoxShadow> {
        self.shadow
            .iter()
            .map(|shadow| BoxShadow {
                color: shadow.color.opacity(strength),
                ..*shadow
            })
            .collect()
    }
}

/// Where the menu is in its entrance: how much of it shows, how much of
/// its shadow, and how far it still has to slide.
#[derive(Clone, Copy)]
pub(crate) struct MenuMotion {
    pub(crate) opacity: f32,
    pub(crate) shadow_strength: f32,
    pub(crate) offset: Pixels,
}

impl MenuMotion {
    /// Samples a menu's entrance under `id`, or `None` while it is absent.
    ///
    /// It comes in as shadcn's `animate-in`: it fades up, and a menu that
    /// `slides` also moves the last step down from its trigger. It goes
    /// out in one frame. GPUI fades each primitive on its own, so a panel
    /// fading out would show its own shadow through itself as a dark
    /// slab, and the same slab would flash on the way in unless the
    /// shadow's ink rises by the cube of the fade, which keeps it out of
    /// sight until the panel covers it. A zero exit also means a menu
    /// that has never opened stays absent from the first frame.
    pub(crate) fn sample(
        id: impl Into<ElementId>,
        open: bool,
        slides: bool,
        look: &MenuLook,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Self> {
        let enter = Theme::global(cx).motion.enter_transition();
        let presence = Presence::new(id.into(), open)
            .transition(if open {
                enter
            } else {
                Transition::new(Duration::ZERO)
            })
            .sample(window, cx);
        if !presence.should_render() {
            return None;
        }
        let progress = match presence.phase {
            PresencePhase::Entering => presence.progress,
            PresencePhase::Present => 1.,
            PresencePhase::Exiting | PresencePhase::Absent => 0.,
        };
        Some(Self {
            opacity: progress,
            shadow_strength: progress * progress * progress,
            offset: if slides {
                look.enter_offset * (1. - progress)
            } else {
                px(0.)
            },
        })
    }
}

/// A row's label over its description, as the reference app draws a
/// select row: the label on one line, cut off with an ellipsis, and the
/// description under it in the menu's description color. The byte
/// `matched` ranges of the label, the characters a search matched, are
/// drawn in the text color at medium weight.
pub(crate) fn label_block(
    label: SharedString,
    matched: &[Range<usize>],
    description: Option<SharedString>,
    look: &MenuLook,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1p5()
        .child(
            div()
                .overflow_hidden()
                .text_ellipsis()
                .child(matched_text(label, matched, look)),
        )
        .when_some(description, |this, text| {
            this.child(div().text_color(look.description).child(text))
        })
        .into_any_element()
}

/// A row's text with the byte `matched` ranges marked: the text color at
/// medium weight. No range gives the text as it is.
pub(crate) fn matched_text(
    text: SharedString,
    matched: &[Range<usize>],
    look: &MenuLook,
) -> AnyElement {
    if matched.is_empty() {
        text.into_any_element()
    } else {
        crate::fuzzy::highlighted(text, matched, look.foreground).into_any_element()
    }
}

/// The frame of one row, the same in a select, a menu, a palette, and the
/// pickers' lists: a full-width row at least a row tall, padded by the row
/// padding, with the row radius, and the highlight fill when `highlighted`.
/// The fill is painted on this element, so it takes the radius. Callers
/// add their id, content, and handlers.
pub(crate) fn row_frame(look: &MenuLook, highlighted: bool) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .min_h(look.row_height)
        .py_1p5()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .px(look.row_padding)
        .rounded(look.row_radius)
        .when(highlighted, |this| this.bg(look.accent))
}

/// The check column at the right of a row: a row-line tall slot that holds
/// the check while `shown`, and keeps its width otherwise so labels line up.
pub(crate) fn check_slot(look: &MenuLook, shown: bool, muted: bool) -> gpui_kit::Div {
    line_slot(look).child(div().size_4().when(shown, |this| {
        this.child(Icon::from(IconName::Check).size_4().text_color(if muted {
            look.muted_foreground
        } else {
            look.indicator
        }))
    }))
}

/// A box one text line tall that centers its child on that line, for a
/// row's leading and trailing parts: in a row whose parts lie along the
/// top, they line up with the label, the first line, and not with the
/// middle of a row a description makes taller.
pub(crate) fn line_slot(look: &MenuLook) -> gpui_kit::Div {
    div()
        .flex_shrink_0()
        .min_h(look.line_height)
        .flex()
        .items_center()
}

/// The inside of a row: its parts in a line along the top, so the leading
/// and trailing slots sit on the label's line. It is as tall as its
/// tallest part, so a one-line row is exactly as before.
pub(crate) fn row_line() -> gpui_kit::Div {
    h_flex().flex_1().min_w_0().items_start().gap_2()
}

/// A hairline between two groups, inset from the menu's edge, with the
/// menu's inset above and below. The room is padding, not margin: a
/// virtual list measures a row without its margin, so a margin would
/// fold the gap away and paint the line on the next row.
pub(crate) fn separator(look: &MenuLook) -> AnyElement {
    div()
        .w_full()
        .py(look.padding)
        .px(look.padding * 2.)
        .child(div().h_px().w_full().bg(look.separator))
        .into_any_element()
}

/// The search field at the top of a menu: the search icon, then the
/// field, a control row tall. The caller gives it its id and the
/// separator under it.
pub(crate) fn search_row(
    input: &Entity<InputState>,
    look: &MenuLook,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .h(look.search_height)
        .px(look.row_padding)
        .child(
            Icon::from(IconName::Search)
                .size_4()
                .text_color(look.muted_foreground),
        )
        .child(div().flex_1().min_w_0().child(Input::new(input)))
        .children(touch_selection::for_state(input, window, cx))
}

/// The colors a menu's search field paints its text, caret, and
/// selection in: the menu's, not the window's. Left unset, base paints
/// the selection in its `accent`, which on this theme is the soft fill,
/// invisible on the menu; the theme's selection is the accent color
/// itself.
pub(crate) fn search_style(theme: &ThemeTokens) -> InputEditorStyle {
    InputEditorStyle {
        foreground: theme.popover_foreground,
        muted_foreground: theme.muted_foreground(),
        selection: theme.selection(),
        caret: theme.popover_foreground,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeConfig, to_hex};
    use gpui_kit::base::ThemeAppearance;

    #[test]
    fn the_menu_reads_the_reference_values() {
        let theme = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let menu = MenuLook::of(&theme, px(16.));
        assert_eq!(to_hex(menu.surface), "#2d2d2d");
        assert_eq!(to_hex(menu.accent), "#3d3d3d");
        assert_eq!(to_hex(menu.indicator), "#cacaca");
        assert_eq!(menu.row_height, px(28.));
        assert_eq!(menu.radius, px(14.));
        assert_eq!(menu.row_radius, px(6.));
        assert_eq!(menu.padding, px(4.));
        assert_eq!(menu.enter_offset, px(-8.));
        assert_eq!(menu.rows_max_height(false), px(380.));
        assert_eq!(menu.rows_max_height(true), px(343.));
    }
}
