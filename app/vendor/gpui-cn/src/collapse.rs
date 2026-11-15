//! Deciding which controls of a narrow row give up their labels.
//!
//! A row of dropdowns shrinks in a fixed order. Labels end in an ellipsis
//! down to a minimum width first, and then the controls named by the
//! caller, one at a time, become icons alone with a tooltip for the label.
//! The row's width comes from the last frame, so a resize settles in a
//! frame.

use std::{cell::Cell, rc::Rc};

use gpui_kit::{IntoElement, Pixels, Styled as _, Window, canvas, px};

/// How many characters a label keeps before its control may become an icon.
pub(crate) const MIN_LABEL_CHARS: usize = 8;

/// A width that the last frame measured.
#[derive(Clone, Default)]
pub(crate) struct Measured(Rc<Cell<Pixels>>);

impl Measured {
    /// The width of the last frame, zero before the first.
    pub(crate) fn width(&self) -> Pixels {
        self.0.get()
    }

    /// An element, as large as its parent, that records the parent's width
    /// and asks for another frame when it changed.
    pub(crate) fn probe(&self) -> impl IntoElement {
        let cell = self.0.clone();
        canvas(
            move |bounds, window, _| {
                let width = bounds.size.width;
                if (f32::from(cell.get()) - f32::from(width)).abs() > 0.5 {
                    cell.set(width);
                    window.refresh();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// What a control needs: the width it takes by itself, its width with the
/// label cut to the minimum, and its width as an icon alone, which is
/// `None` when it has no icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Need {
    pub(crate) natural: Pixels,
    pub(crate) min: Pixels,
    pub(crate) icon_only: Option<Pixels>,
}

impl Need {
    /// A control that shrinks on its own: its natural width is its minimum.
    pub(crate) fn flexible(min: Pixels, icon_only: Option<Pixels>) -> Self {
        Self {
            natural: min,
            min,
            icon_only,
        }
    }
}

/// How a control fits a row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fit {
    /// As wide as it is by itself.
    Natural,
    /// Cut to this width, which its label ends in an ellipsis to fit.
    Shrunk(Pixels),
    /// An icon alone.
    Icon,
}

impl Fit {
    /// Whether the control is an icon alone.
    pub(crate) fn is_icon(self) -> bool {
        self == Fit::Icon
    }
}

/// How `needs`, with `gap` between them (`tight_gap` once any is an icon),
/// fit `available`. Labels shrink first, each no further than its minimum,
/// the longest giving up room first. Then controls become icons in the
/// order of `order`, which holds indexes into `needs`, and the labels
/// share what the icons leave. Before the row has a width (`available` is
/// zero) everything is natural.
pub(crate) fn plan(
    available: Pixels,
    gap: Pixels,
    tight_gap: Pixels,
    needs: &[Need],
    order: &[usize],
) -> Vec<Fit> {
    let mut fits = vec![Fit::Natural; needs.len()];
    if available <= px(0.) {
        return fits;
    }
    let available = f32::from(available);
    let count = needs.len().saturating_sub(1) as f32;
    let width = |need: &Need, fit: Fit| match (fit, need.icon_only) {
        (Fit::Icon, Some(icon)) => f32::from(icon),
        _ => f32::from(need.natural),
    };
    let minimum = |need: &Need, fit: Fit| match (fit, need.icon_only) {
        (Fit::Icon, Some(icon)) => f32::from(icon),
        _ => f32::from(need.min),
    };
    let mut icons = vec![false; needs.len()];
    let gap_of = |icons: &[bool]| {
        f32::from(if icons.iter().any(|icon| *icon) {
            tight_gap
        } else {
            gap
        })
    };
    let fits_with = |icons: &[bool]| {
        let total: f32 = needs
            .iter()
            .zip(icons)
            .map(|(need, &icon)| minimum(need, if icon { Fit::Icon } else { Fit::Natural }))
            .sum();
        total + gap_of(icons) * count <= available
    };
    for &index in order {
        if fits_with(&icons) {
            break;
        }
        if needs[index].icon_only.is_some() {
            icons[index] = true;
        }
    }
    let gaps = gap_of(&icons) * count;
    let mut remaining = available - gaps;
    let mut open: Vec<usize> = Vec::new();
    for (index, need) in needs.iter().enumerate() {
        if icons[index] {
            fits[index] = Fit::Icon;
            remaining -= width(need, Fit::Icon);
        } else {
            open.push(index);
        }
    }
    // Controls that need less than an equal share keep their width, and
    // the rest split what is left, none under its minimum.
    loop {
        if open.is_empty() {
            break;
        }
        let share = remaining / open.len() as f32;
        let small: Vec<usize> = open
            .iter()
            .copied()
            .filter(|&i| f32::from(needs[i].natural) <= share)
            .collect();
        if small.is_empty() {
            for &index in &open {
                let cut = share.max(f32::from(needs[index].min));
                fits[index] = Fit::Shrunk(px(cut));
            }
            break;
        }
        for &index in &small {
            remaining -= f32::from(needs[index].natural);
        }
        open.retain(|index| !small.contains(index));
    }
    fits
}

/// The width of `text` in the window's current text style.
pub(crate) fn text_width(text: &str, window: &Window) -> Pixels {
    let style = window.text_style();
    let size = style.font_size.to_pixels(window.rem_size());
    window
        .text_system()
        .shape_line(
            gpui_kit::SharedString::from(text.to_string()),
            size,
            &[style.to_run(text.len())],
            None,
        )
        .width
}

/// The width of `MIN_LABEL_CHARS` average characters in the window's
/// current text style: what a label keeps before its control collapses.
pub(crate) fn min_label_width(window: &Window, chars: usize) -> Pixels {
    text_width(&"n".repeat(chars), window)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(natural: f32, min: f32, icon: Option<f32>) -> Need {
        Need {
            natural: px(natural),
            min: px(min),
            icon_only: icon.map(px),
        }
    }

    fn plan_of(available: f32, needs: &[Need], order: &[usize]) -> Vec<Fit> {
        plan(px(available), px(10.), px(10.), needs, order)
    }

    #[test]
    fn nothing_changes_while_the_row_fits_or_has_no_width_yet() {
        let needs = [need(100., 60., Some(28.)), need(100., 60., Some(28.))];
        assert_eq!(plan_of(300., &needs, &[1, 0]), [Fit::Natural; 2]);
        assert_eq!(plan_of(0., &needs, &[1, 0]), [Fit::Natural; 2]);
    }

    #[test]
    fn labels_shrink_to_their_minimum_before_any_control_becomes_an_icon() {
        let needs = [need(100., 60., Some(28.)), need(100., 60., Some(28.))];
        assert_eq!(
            plan_of(170., &needs, &[1, 0]),
            [Fit::Shrunk(px(80.)), Fit::Shrunk(px(80.))]
        );
        assert_eq!(
            plan_of(130., &needs, &[1, 0]),
            [Fit::Shrunk(px(60.)), Fit::Shrunk(px(60.))]
        );
    }

    #[test]
    fn controls_become_icons_in_the_order_given_until_the_labels_fit() {
        let needs = [
            need(100., 60., Some(28.)),
            need(100., 60., Some(28.)),
            need(100., 60., Some(28.)),
        ];
        let fits = plan_of(150., &needs, &[2, 1, 0]);
        assert_eq!(fits[2], Fit::Icon);
        assert!(!fits[0].is_icon());
        let fits = plan_of(60., &needs, &[2, 1, 0]);
        assert_eq!(fits, [Fit::Icon; 3]);
    }

    #[test]
    fn a_short_label_keeps_its_width_and_the_long_ones_share_the_rest() {
        let needs = [need(40., 40., None), need(200., 60., Some(28.))];
        let fits = plan_of(150., &needs, &[1]);
        assert_eq!(fits[0], Fit::Natural);
        assert_eq!(fits[1], Fit::Shrunk(px(100.)));
    }

    #[test]
    fn a_control_without_an_icon_never_collapses() {
        let needs = [need(100., 100., None), need(100., 60., Some(28.))];
        assert_eq!(plan_of(50., &needs, &[0, 1])[0], Fit::Shrunk(px(100.)));
    }
}
