use gpui_kit::{Hsla, SharedString};

use super::color::hex;

/// The inputs a theme is built from. Every token components read is derived
/// from these, so this is the whole customization surface for colors.
///
/// A surface, an ink, a brand accent, and a contrast that says how far
/// secondary surfaces and borders move from the surface toward the ink.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ThemeConfig {
    /// The window background.
    pub surface: Hsla,
    /// Text and strong strokes.
    pub ink: Hsla,
    /// The brand color: focus ring, links, toggles, sliders, selection.
    pub accent: Hsla,
    /// `0..=100`. 50 gives the shadcn neutral ratios.
    pub contrast: u8,
    /// Font families. `None` uses the platform default.
    pub fonts: ThemeFonts,
    /// Colors that carry a meaning of their own.
    pub semantic: SemanticColors,
}

/// Font families for a theme. `None` means the platform default.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ThemeFonts {
    /// Interface text.
    pub ui: Option<SharedString>,
    /// Code, identifiers, and aligned numbers.
    pub code: Option<SharedString>,
}

impl Default for ThemeFonts {
    /// The platform interface font, and the bundled code font when one
    /// ships (see [`super::fonts`]).
    fn default() -> Self {
        Self {
            ui: None,
            code: super::fonts::default_code_font(),
        }
    }
}

/// Colors with a fixed meaning, independent of surface and ink.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SemanticColors {
    /// Completed, added, positive.
    pub success: Hsla,
    /// Destructive actions, removed, errors.
    pub destructive: Hsla,
    /// Needs attention but not an error.
    pub warning: Hsla,
    /// Skills, plugins, and other extension surfaces.
    pub skill: Hsla,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self::light()
    }
}

impl Default for SemanticColors {
    fn default() -> Self {
        Self::light()
    }
}

impl ThemeConfig {
    /// The built-in light theme.
    pub fn light() -> Self {
        Self {
            surface: hex("#ffffff"),
            ink: hex("#1a1c1f"),
            accent: hex("#339cff"),
            contrast: 45,
            fonts: ThemeFonts::default(),
            semantic: SemanticColors::light(),
        }
    }

    /// The built-in dark theme.
    pub fn dark() -> Self {
        Self {
            surface: hex("#181818"),
            ink: hex("#ffffff"),
            // The reference lifts its accent on a dark surface: its
            // toggles and checkboxes sample as #539af8, not #339cff.
            accent: hex("#539af8"),
            contrast: 60,
            fonts: ThemeFonts::default(),
            semantic: SemanticColors::dark(),
        }
    }
}

impl SemanticColors {
    /// The built-in light values.
    pub fn light() -> Self {
        Self {
            success: hex("#00a240"),
            destructive: hex("#ba2623"),
            warning: hex("#b45309"),
            skill: hex("#924ff7"),
        }
    }

    /// The built-in dark values.
    pub fn dark() -> Self {
        Self {
            success: hex("#40c977"),
            destructive: hex("#fa423e"),
            warning: hex("#f59e0b"),
            skill: hex("#ad7bf9"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_differ_in_surface_and_accent() {
        let light = ThemeConfig::light();
        let dark = ThemeConfig::dark();
        assert_ne!(light.accent, dark.accent);
        assert_ne!(light.surface, dark.surface);
        assert_eq!(light.contrast, 45);
        assert_eq!(dark.contrast, 60);
        assert_eq!(ThemeConfig::default(), light);
    }
}
