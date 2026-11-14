use gpui_kit::base::StyledExt as _;
use gpui_kit::{
    AnyElement, App, ElementId, ImageSource, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StyleRefinement, Styled, TestSupportExt as _,
    Window, base, div, prelude::FluentBuilder as _,
};

use crate::{ActiveTheme as _, Icon};

/// The diameter of an [`Avatar`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AvatarSize {
    /// 24px, for dense rows and chips.
    Sm,
    /// 32px, shadcn's default.
    #[default]
    Default,
    /// 40px, for headers and cards.
    Lg,
}

/// A shadcn-style avatar: a circle that shows an image, or the initials
/// of a name, or an icon when there is no image.
///
/// The image paints over the fallback, so the initials or the icon show
/// while the image loads and stay when it fails, as Radix's avatar does.
/// The icon is half the circle across.
/// The fallback fill is the theme's `selected` step with `foreground`
/// text. shadcn's `bg-muted` is the page's own hover step here, too faint
/// to read as a circle.
///
/// ```
/// use gpui_cn::{Avatar, AvatarSize};
///
/// let _ = Avatar::new("ada").name("Ada Lovelace").size(AvatarSize::Lg);
/// ```
#[derive(IntoElement)]
pub struct Avatar {
    id: ElementId,
    style: StyleRefinement,
    size: AvatarSize,
    src: Option<ImageSource>,
    text: Option<SharedString>,
    icon: Option<Icon>,
    ring: bool,
}

impl Avatar {
    /// An avatar with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            size: AvatarSize::Default,
            src: None,
            text: None,
            icon: None,
            ring: false,
        }
    }

    /// The image to show.
    pub fn src(mut self, source: impl Into<ImageSource>) -> Self {
        self.src = Some(source.into());
        self
    }

    /// The name whose initials show when there is no image.
    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.text = Some(initials(&name.into()));
        self
    }

    /// The icon that shows when there is no image and no name.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The diameter.
    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }

    /// Text to show as it is, such as the `+N` of a group.
    fn text(mut self, text: SharedString) -> Self {
        self.text = Some(text);
        self
    }

    /// Paints a ring in the window background around the circle, so it
    /// parts from an avatar it overlaps. [`AvatarGroup`] sets it.
    fn ring(mut self) -> Self {
        self.ring = true;
        self
    }
}

/// The initials of `name`: the first letter of its first and last words,
/// in capitals.
fn initials(name: &str) -> SharedString {
    let mut words = name.split_whitespace();
    let first = words.next().and_then(|word| word.chars().next());
    let last = words.last().and_then(|word| word.chars().next());
    first
        .into_iter()
        .chain(last)
        .flat_map(char::to_uppercase)
        .collect::<String>()
        .into()
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Avatar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let (diameter, text_size) = match self.size {
            AvatarSize::Sm => (metrics.avatar_sm, metrics.avatar_initials_sm),
            AvatarSize::Default => (metrics.avatar_md, metrics.avatar_initials_md),
            AvatarSize::Lg => (metrics.avatar_lg, metrics.avatar_initials_lg),
        };
        let (fill, ink, ring, ring_width) = (
            theme.selected,
            theme.foreground(),
            theme.background(),
            metrics.avatar_group_ring,
        );

        let content: Option<AnyElement> = match (self.text, self.icon) {
            (Some(text), _) => Some(text.into_any_element()),
            (None, Some(icon)) => Some(icon.size(diameter / 2.).into_any_element()),
            (None, None) => None,
        };
        let fallback = base::AvatarFallback::new()
            .size_full()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(fill)
            .text_color(ink)
            .text_size(text_size)
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .relative()
            .children(content)
            .children(self.src.map(|src| {
                base::AvatarImage::new(src)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .rounded_full()
            }));
        let avatar = base::Avatar::new()
            .size_full()
            .rounded_full()
            .fallback(fallback);

        div()
            .id(self.id)
            .test_support()
            .flex_none()
            .size(diameter)
            .rounded_full()
            .when(self.ring, |this| this.border(ring_width).border_color(ring))
            .child(avatar)
            .refine_style(&self.style)
    }
}

/// A row of overlapping avatars, shadcn's `*:ring-2 -space-x-2`, with a
/// `+N` avatar for the ones past its limit.
///
/// ```
/// use gpui_cn::{Avatar, AvatarGroup};
///
/// let _ = AvatarGroup::new("team")
///     .child(Avatar::new("ada").name("Ada Lovelace"))
///     .child(Avatar::new("alan").name("Alan Turing"))
///     .limit(3);
/// ```
#[derive(IntoElement)]
pub struct AvatarGroup {
    id: ElementId,
    style: StyleRefinement,
    size: AvatarSize,
    avatars: Vec<Avatar>,
    limit: Option<usize>,
}

impl AvatarGroup {
    /// A group with a stable id.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            size: AvatarSize::Default,
            avatars: Vec::new(),
            limit: None,
        }
    }

    /// Adds an avatar.
    pub fn child(mut self, avatar: Avatar) -> Self {
        self.avatars.push(avatar);
        self
    }

    /// Adds avatars.
    pub fn children(mut self, avatars: impl IntoIterator<Item = Avatar>) -> Self {
        self.avatars.extend(avatars);
        self
    }

    /// Shows at most `limit` avatars and a `+N` avatar for the rest.
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The diameter of every avatar in the group.
    pub fn size(mut self, size: AvatarSize) -> Self {
        self.size = size;
        self
    }
}

impl Styled for AvatarGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for AvatarGroup {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let overlap = cx.theme().metrics.avatar_group_overlap;
        let total = self.avatars.len();
        let shown = self.limit.unwrap_or(total).min(total);
        let rest = total - shown;
        let size = self.size;
        let more = (rest > 0).then(|| {
            Avatar::new(ElementId::NamedChild(
                std::sync::Arc::new(self.id.clone()),
                "more".into(),
            ))
            .text(format!("+{rest}").into())
        });
        let avatars = self
            .avatars
            .into_iter()
            .take(shown)
            .chain(more)
            .enumerate()
            .map(move |(index, avatar)| {
                let avatar = avatar.size(size).ring();
                if index == 0 {
                    avatar
                } else {
                    avatar.ml(-overlap)
                }
            });
        base::h_flex()
            .id(self.id)
            .test_support()
            .children(avatars)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_the_first_and_last_words() {
        assert_eq!(initials("Ada Lovelace"), "AL");
        assert_eq!(initials("  grace brewster hopper "), "GH");
        assert_eq!(initials("plato"), "P");
        assert_eq!(initials(""), "");
    }
}
