use gpui_kit::base::StyledExt as _;
use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, StyleRefinement, Styled, TestSupportExt as _, Window, div, relative,
};

use crate::{ActiveTheme as _, Icon, TagVariant};

/// What a [`Badge`] shows at the corner of its element.
#[derive(Clone, Default)]
enum Mark {
    /// A count, hidden at zero.
    #[default]
    Count,
    /// A dot.
    Dot,
    /// An icon in a circle.
    Icon(Box<Icon>),
}

/// A count, a dot, or an icon at the corner of another element, such as
/// unread messages on an avatar. gpui-kit's `Badge`.
///
/// A count sits over the top trailing corner and hides at zero; past the
/// maximum it shows `max+`. A dot sits inside that corner. An icon sits on
/// the bottom trailing corner with a ring in the window background. The
/// fill is the primary color, as quiet as the rest of the window. A
/// status variant, such as [`Badge::danger`] for an alert, paints the
/// theme's solid status fill, which keeps its text at WCAG AA. For a
/// labeled pill, use [`Tag`](crate::Tag).
///
/// ```
/// use gpui_cn::{Avatar, Badge};
/// use gpui_kit::ParentElement as _;
///
/// let _ = Badge::new("inbox").count(3).child(Avatar::new("ada").name("Ada Lovelace"));
/// ```
#[derive(IntoElement)]
pub struct Badge {
    id: ElementId,
    style: StyleRefinement,
    mark: Mark,
    count: usize,
    max: usize,
    variant: TagVariant,
    children: Vec<AnyElement>,
}

impl Badge {
    /// A badge with a stable id. The mark carries the id; the element it
    /// sits on is a child.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            mark: Mark::Count,
            count: 0,
            max: 99,
            variant: TagVariant::Primary,
            children: Vec::new(),
        }
    }

    /// Shows a count. A count of zero shows nothing.
    pub fn count(mut self, count: usize) -> Self {
        self.mark = Mark::Count;
        self.count = count;
        self
    }

    /// The largest count to show as a number: 99 unless set. A larger
    /// count shows as `max+`.
    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }

    /// Shows a dot.
    pub fn dot(mut self) -> Self {
        self.mark = Mark::Dot;
        self
    }

    /// Shows an icon.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.mark = Mark::Icon(Box::new(icon.into()));
        self
    }

    /// The color of the mark, from the same set as a [`Tag`](crate::Tag).
    pub fn variant(mut self, variant: TagVariant) -> Self {
        self.variant = variant;
        self
    }

    /// [`TagVariant::Secondary`].
    pub fn secondary(self) -> Self {
        self.variant(TagVariant::Secondary)
    }

    /// [`TagVariant::Danger`].
    pub fn danger(self) -> Self {
        self.variant(TagVariant::Danger)
    }

    /// [`TagVariant::Success`].
    pub fn success(self) -> Self {
        self.variant(TagVariant::Success)
    }

    /// [`TagVariant::Warning`].
    pub fn warning(self) -> Self {
        self.variant(TagVariant::Warning)
    }

    /// [`TagVariant::Info`].
    pub fn info(self) -> Self {
        self.variant(TagVariant::Info)
    }
}

impl Styled for Badge {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Badge {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Badge {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let (fill, ink) = match self.variant {
            TagVariant::Primary => (theme.primary(), theme.primary_foreground()),
            TagVariant::Secondary => (theme.selected, theme.foreground()),
            TagVariant::Danger => (theme.destructive_solid, theme.solid_foreground),
            TagVariant::Success => (theme.success_solid, theme.solid_foreground),
            TagVariant::Warning => (theme.warning_solid, theme.solid_foreground),
            TagVariant::Info => (theme.info_solid, theme.solid_foreground),
        };
        let ring = theme.background();
        let offset = -metrics.badge_count_offset;
        let (count, count_text, dot, icon_size) = (
            metrics.badge_count,
            metrics.badge_count_text,
            metrics.badge_dot,
            metrics.badge_icon,
        );

        let mark = div()
            .id(self.id)
            .test_support()
            .absolute()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(fill)
            .text_color(ink);
        let mark = match self.mark {
            Mark::Count if self.count == 0 => None,
            Mark::Count => {
                let text = if self.count > self.max {
                    format!("{}+", self.max)
                } else {
                    self.count.to_string()
                };
                Some(
                    mark.top(offset)
                        .right(offset)
                        .h(count)
                        .min_w(count)
                        .px_1()
                        .text_size(count_text)
                        .line_height(relative(1.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(text),
                )
            }
            Mark::Dot => Some(mark.top_0().right_0().size(dot)),
            Mark::Icon(icon) => Some(
                mark.bottom_0()
                    .right_0()
                    .size(icon_size)
                    .border_1()
                    .border_color(ring)
                    .child(icon.size(icon_size / 2.)),
            ),
        };

        div()
            .relative()
            .flex_none()
            .children(self.children)
            .children(mark)
            .refine_style(&self.style)
    }
}
