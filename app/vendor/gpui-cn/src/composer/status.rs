use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement, RenderOnce,
    StyleRefinement, Styled, TestSupportExt as _, Window, base::StyledExt as _, div,
    prelude::FluentBuilder as _, px,
};

use crate::{
    ActiveTheme as _, Progress, StatusSelect,
    collapse::{self, Fit, Measured},
};

/// The context a conversation has used, as a ring and a percent: a
/// circular [`Progress`] beside the number, in the muted text color.
///
/// The percent is a plain value the application sets.
///
/// ```
/// use gpui_cn::ContextMeter;
///
/// let _ = ContextMeter::new("context").percent(57.);
/// ```
#[derive(IntoElement)]
#[non_exhaustive]
pub struct ContextMeter {
    id: ElementId,
    percent: f32,
    style: StyleRefinement,
}

impl ContextMeter {
    /// An empty meter with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            percent: 0.,
            style: StyleRefinement::default(),
        }
    }

    /// The percentage used, clamped to `0..=100`.
    pub fn percent(mut self, percent: f32) -> Self {
        self.percent = percent.clamp(0., 100.);
        self
    }

    /// The percentage used.
    pub fn percent_used(&self) -> f32 {
        self.percent
    }
}

impl Styled for ContextMeter {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ContextMeter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, gap) = (theme.muted_foreground(), theme.metrics.status_label_gap);
        let percent = self.percent;
        div()
            .id(self.id.clone())
            .test_support()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(gap)
            .text_color(muted)
            .refine_style(&self.style)
            .child(
                Progress::new(ElementId::NamedChild(self.id.into(), "ring".into()))
                    .circular()
                    .value(percent)
                    .accessibility_label("Context used"),
            )
            .child(format!("{percent:.0}%"))
    }
}

/// The strip behind the top of a composer's card: any number of items
/// from the left, such as a folder, a device, and a branch, in the full
/// text color (each a [`StatusSelect`] that opens a menu), and optionally a [`ContextMeter`] or any element at the far
/// right. With no trailing element there is nothing at the right.
///
/// The tab is inset from each side of the card and rounded on top only.
/// The tab is exactly [`status_tab_height`](crate::theme::MetricTokens::status_tab_height)
/// tall, with the items centered in it and square bottom corners, and it
/// carries no hidden height. A [`Composer`](crate::Composer) sets its card
/// directly under the tab, tucked over the tab's bottom edge by the card's
/// hairline.
/// [`select`](Self::select) adds a dropdown item, any other element joins
/// the left side as a child, [`context`](Self::context) adds the meter,
/// and [`trailing`](Self::trailing) replaces what is at the right.
///
/// ```
/// use gpui_cn::ComposerStatusTab;
///
/// let _ = ComposerStatusTab::new("status").context(57.);
/// ```
#[derive(IntoElement)]
#[non_exhaustive]
pub struct ComposerStatusTab {
    id: ElementId,
    items: Vec<Entry>,
    trailing: Option<AnyElement>,
    style: StyleRefinement,
}

/// An item of the tab: a select the tab can collapse to an icon, or any
/// other element.
enum Entry {
    Select(Box<StatusSelect>),
    Other(AnyElement),
}

impl ComposerStatusTab {
    /// An empty tab with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            trailing: None,
            style: StyleRefinement::default(),
        }
    }

    /// An item that opens a menu of options. See [`StatusSelect`].
    pub fn select(mut self, select: StatusSelect) -> Self {
        self.items.push(Entry::Select(Box::new(select)));
        self
    }

    /// The context meter at the far right, at `percent` used.
    pub fn context(mut self, percent: f32) -> Self {
        let id = ElementId::NamedChild(self.id.clone().into(), "context".into());
        self.trailing = Some(ContextMeter::new(id).percent(percent).into_any_element());
        self
    }

    /// Replaces what sits at the far right.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }
}

impl Styled for ComposerStatusTab {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Children that are not a [`StatusSelect`](crate::StatusSelect) keep the width
/// they have: they take no part in the plan that shrinks labels and turns
/// selects into icons when the tab is narrow.
impl ParentElement for ComposerStatusTab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.items.extend(elements.into_iter().map(Entry::Other));
    }
}

impl RenderOnce for ComposerStatusTab {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let (height, inset, gap) = (
            metrics.status_tab_height,
            metrics.status_tab_inset,
            metrics.status_tab_gap,
        );
        let (pad_left, pad_right) = (metrics.status_tab_inset, theme.base.spacing.lg);
        let tight = theme.base.spacing.sm;
        let (fill, radius, text) = (theme.sidebar, theme.radius_lg(), theme.text_control);
        let widths = window
            .use_keyed_state(
                ElementId::NamedChild(self.id.clone().into(), "widths".into()),
                cx,
                |_, _| (Measured::default(), Measured::default()),
            )
            .read(cx)
            .clone();
        let (tab_width, trailing_width) = widths;
        let needs: Vec<_> = self
            .items
            .iter()
            .filter_map(|entry| match entry {
                Entry::Select(select) => Some(select.need(window, cx)),
                Entry::Other(_) => None,
            })
            .collect();
        let order: Vec<usize> = (0..needs.len()).rev().collect();
        let trailing_gap = if self.trailing.is_some() { gap } else { px(0.) };
        let available =
            tab_width.width() - pad_left - pad_right - trailing_width.width() - trailing_gap;
        let collapsed = collapse::plan(
            if tab_width.width() > px(0.) {
                available
            } else {
                px(0.)
            },
            gap,
            tight,
            &needs,
            &order,
        );
        let gap = if collapsed.iter().any(|fit| fit.is_icon()) {
            tight
        } else {
            gap
        };
        let mut selects = 0;
        let items: Vec<AnyElement> = self
            .items
            .into_iter()
            .map(|entry| match entry {
                Entry::Select(select) => {
                    let fit = collapsed.get(selects).copied().unwrap_or(Fit::Natural);
                    selects += 1;
                    match fit {
                        Fit::Icon => select.icon_only(true),
                        Fit::Shrunk(width) => select.width(width),
                        Fit::Natural => *select,
                    }
                    .into_any_element()
                }
                Entry::Other(element) => element,
            })
            .collect();
        div()
            .id(self.id)
            .test_support()
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .h(height)
            .mx(inset)
            .pl(pad_left)
            .pr(pad_right)
            .gap(gap)
            .rounded_t(radius)
            .bg(fill)
            .text_size(text.size)
            .line_height(text.line_height)
            .refine_style(&self.style)
            .child(tab_width.probe())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(gap)
                    .children(items),
            )
            .when_some(self.trailing, |this, trailing| {
                this.child(
                    div()
                        .relative()
                        .flex_shrink_0()
                        .child(trailing)
                        .child(trailing_width.probe()),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_percent_clamps() {
        assert_eq!(ContextMeter::new("m").percent(57.).percent_used(), 57.);
        assert_eq!(ContextMeter::new("m").percent(180.).percent_used(), 100.);
        assert_eq!(ContextMeter::new("m").percent(-1.).percent_used(), 0.);
    }
}
