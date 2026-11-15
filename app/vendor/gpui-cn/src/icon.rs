use gpui_kit::{
    App, IntoElement, Radians, RenderOnce, SharedString, StyleRefinement, Styled, Svg,
    Transformation, Window, base::StyledExt as _, prelude::FluentBuilder as _, svg,
};

/// Where an icon's SVG comes from.
#[derive(Clone, Debug)]
pub enum IconSource {
    /// A path resolved by the application's `AssetSource`, such as
    /// `icons/check.svg`.
    Path(SharedString),
    /// SVG bytes embedded in the binary, with no asset lookup.
    Bytes(&'static [u8]),
}

/// An SVG icon sized on the rem scale and painted with the text color.
///
/// The bundled Lucide `IconName` already renders on its own at the font
/// size. This type is for icons an application brings itself (a path
/// or embedded bytes) and for sizing independent of the font, which a
/// button needs. Defaults to `size_4` (16px at the default font size). Any
/// `Styled` method refines it:
///
/// ```
/// use gpui_cn::Icon;
/// use gpui_kit::Styled as _;
///
/// const DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8"/></svg>"#;
///
/// let _ = Icon::new("icons/check.svg").size_5();
/// let _ = Icon::from_bytes(DOT);
/// let _ = Icon::from(DOT);
/// ```
///
/// An `IconName` from the Lucide catalog in `gpui_kit::assets` converts
/// into an icon: `Icon::from(IconName::Check)`. The application registers
/// the asset source that holds the SVG files, `gpui_kit::assets::Assets`.
#[derive(Clone, IntoElement)]
pub struct Icon {
    source: IconSource,
    style: StyleRefinement,
    rotation: Option<Radians>,
}

impl Icon {
    /// An icon from an asset path.
    pub fn new(path: impl Into<SharedString>) -> Self {
        Self::from_source(IconSource::Path(path.into()))
    }

    /// An icon from embedded SVG bytes.
    pub fn from_bytes(bytes: &'static [u8]) -> Self {
        Self::from_source(IconSource::Bytes(bytes))
    }

    fn from_source(source: IconSource) -> Self {
        Self {
            source,
            style: StyleRefinement::default(),
            rotation: None,
        }
    }

    /// Turns the icon about its center, clockwise. A chevron that turns
    /// with a fold uses it.
    pub fn rotate(mut self, angle: impl Into<Radians>) -> Self {
        self.rotation = Some(angle.into());
        self
    }

    /// Where the SVG comes from.
    pub fn source(&self) -> &IconSource {
        &self.source
    }
}

impl From<&'static str> for Icon {
    fn from(path: &'static str) -> Self {
        Self::new(path)
    }
}

impl From<SharedString> for Icon {
    fn from(path: SharedString) -> Self {
        Self::new(path)
    }
}

impl From<&'static [u8]> for Icon {
    fn from(bytes: &'static [u8]) -> Self {
        Self::from_bytes(bytes)
    }
}

/// `include_bytes!` yields an array reference, which does not coerce
/// through `impl Into<Icon>`, so arrays convert directly.
impl<const N: usize> From<&'static [u8; N]> for Icon {
    fn from(bytes: &'static [u8; N]) -> Self {
        Self::from_bytes(bytes)
    }
}

impl From<gpui_kit::assets::IconName> for Icon {
    fn from(name: gpui_kit::assets::IconName) -> Self {
        Self::new(name.path())
    }
}

impl Styled for Icon {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let text_style = window.text_style();
        let element: Svg = match &self.source {
            IconSource::Path(path) => svg().path(path.clone()),
            IconSource::Bytes(bytes) => svg().data(bytes),
        };
        element
            .flex_shrink_0()
            .size_4()
            .text_color(text_style.color)
            .when_some(self.rotation, |this, angle| {
                this.with_transformation(Transformation::rotate(angle))
            })
            .refine_style(&self.style)
    }
}
