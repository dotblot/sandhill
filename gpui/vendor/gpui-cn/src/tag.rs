use gpui_kit::base::StyledExt as _;
use gpui_kit::{
    AnyElement, App, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement, RenderOnce, SharedString, StyleRefinement, Styled, TestSupportExt as _, Window,
    div, prelude::FluentBuilder as _,
};

use crate::{ActiveTheme as _, Icon, theme::ThemeTokens};

/// The color of a [`Tag`], after gpui-kit's tag variants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TagVariant {
    /// The primary fill with the primary foreground, shadcn's default badge.
    #[default]
    Primary,
    /// The selected step with the foreground, the fill an avatar uses.
    Secondary,
    /// The destructive color.
    Danger,
    /// The success color.
    Success,
    /// The warning color.
    Warning,
    /// The info color.
    Info,
}

impl TagVariant {
    /// The fill, the text color, and the border.
    ///
    /// A status color paints on its theme tint with the color as text. Outline drops the fill: a neutral tag
    /// takes the input hairline, a status tag its own color.
    fn colors(self, outline: bool, theme: &ThemeTokens) -> (Hsla, Hsla, Hsla) {
        let clear = gpui_kit::transparent_black();
        let status = match self {
            Self::Primary | Self::Secondary => None,
            Self::Danger => Some((theme.destructive(), theme.destructive_tint)),
            Self::Success => Some((theme.success, theme.success_tint)),
            Self::Warning => Some((theme.warning, theme.warning_tint)),
            Self::Info => Some((theme.info, theme.info_tint)),
        };
        match (status, outline) {
            (Some((color, _)), true) => (clear, color, color),
            (Some((color, tint)), false) => (tint, color, clear),
            (None, true) => (clear, theme.foreground(), theme.input()),
            (None, false) if self == Self::Primary => {
                (theme.primary(), theme.primary_foreground(), clear)
            }
            (None, false) => (theme.selected, theme.foreground(), clear),
        }
    }
}

/// A small pill with a label and an optional icon, for a status or a
/// category. gpui-kit's `Tag`, drawn as shadcn's badge.
///
/// It is 20px tall on the rem scale with `px_2` at the sides, `text-xs`
/// at medium weight, and a 12px icon. It is not interactive; put it in a
/// button to make it one. For a count or a dot on another element, use
/// [`Badge`](crate::Badge).
///
/// ```
/// use gpui_cn::{Tag, gpui_kit::assets::IconName};
///
/// let _ = Tag::new("status").label("Verified").icon(IconName::CircleCheck).success();
/// let _ = Tag::new("draft").label("Draft").secondary().outline();
/// ```
#[derive(IntoElement)]
pub struct Tag {
    id: ElementId,
    style: StyleRefinement,
    variant: TagVariant,
    outline: bool,
    icon: Option<Icon>,
    label: Option<SharedString>,
    children: Vec<AnyElement>,
}

impl Tag {
    /// A tag with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            variant: TagVariant::default(),
            outline: false,
            icon: None,
            label: None,
            children: Vec::new(),
        }
    }

    /// The text.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The icon before the text.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The color.
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

    /// Draws a hairline and no fill.
    pub fn outline(mut self) -> Self {
        self.outline = true;
        self
    }
}

impl Styled for Tag {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl ParentElement for Tag {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Tag {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (fill, ink, border) = self.variant.colors(self.outline, theme);
        let (height, icon_size) = (theme.metrics.tag_height, theme.metrics.tag_icon);
        let text = theme.base.typography.xs;

        div()
            .id(self.id)
            .test_support()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .h(height)
            .px_2()
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded_full()
            .border_1()
            .border_color(border)
            .bg(fill)
            .text_color(ink)
            .text_size(text.size)
            .line_height(text.line_height)
            .font_weight(FontWeight::MEDIUM)
            .when_some(self.icon, |this, icon| this.child(icon.size(icon_size)))
            .when_some(self.label, |this, label| this.child(label))
            .children(self.children)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ThemeConfig, to_hex};
    use gpui_kit::base::ThemeAppearance;

    #[test]
    fn variants_paint_from_the_theme() {
        let dark = crate::theme::test_tokens(&ThemeConfig::dark(), ThemeAppearance::Dark);
        let light = crate::theme::test_tokens(&ThemeConfig::light(), ThemeAppearance::Light);
        let hex =
            |(fill, ink, border): (Hsla, Hsla, Hsla)| [to_hex(fill), to_hex(ink), to_hex(border)];
        assert_eq!(
            hex(TagVariant::Primary.colors(false, &dark)),
            ["#dfdfdf", "#2d2d2d", "#00000000"].map(String::from),
        );
        assert_eq!(
            hex(TagVariant::Secondary.colors(false, &light)),
            ["#e8e8e8", "#1a1c1f", "#00000000"].map(String::from),
        );
        assert_eq!(
            hex(TagVariant::Success.colors(false, &dark)),
            ["#25372a", "#40c977", "#00000000"].map(String::from),
        );
        assert_eq!(
            hex(TagVariant::Info.colors(true, &light)),
            ["#00000000", "#339cff", "#339cff"].map(String::from),
        );
        assert_eq!(
            hex(TagVariant::Primary.colors(true, &light)),
            ["#00000000", "#1a1c1f", "#e5e5e6"].map(String::from),
        );
    }
}
