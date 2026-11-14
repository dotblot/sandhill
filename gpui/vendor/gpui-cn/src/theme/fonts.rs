//! Fonts gpui-cn can bring with it.
//!
//! The theme names fonts by family ([`ThemeFonts`](super::ThemeFonts));
//! the platform resolves them. A font the platform may not have is
//! registered with GPUI's text system first, by [`crate::init`]. With the
//! `jetbrains-mono` feature that is JetBrains Mono, the variable font with
//! every weight, from the `damascene-fonts-jetbrains-mono` crate under the
//! SIL Open Font License. The built-in themes then name it as the code
//! font. Without the feature the code font is the platform's monospace.
//! An application names another family with
//! `Theme::update(cx, |theme| theme.light.fonts.code = Some("Menlo".into()))`.

use gpui_kit::{App, SharedString};

/// The family name of JetBrains Mono as its font file declares it.
pub const JETBRAINS_MONO: &str = "JetBrains Mono";

/// The code fonts that ship and are registered by [`crate::init`], the
/// preferred one first.
pub fn bundled_code_fonts() -> Vec<SharedString> {
    if cfg!(feature = "jetbrains-mono") {
        vec![JETBRAINS_MONO.into()]
    } else {
        Vec::new()
    }
}

/// The code font the built-in themes name: the first bundled one, else
/// `None` for the platform default.
pub fn default_code_font() -> Option<SharedString> {
    bundled_code_fonts().into_iter().next()
}

/// Registers the bundled fonts with the text system. A no-op without a
/// font feature.
pub(crate) fn register(cx: &mut App) {
    #[cfg(feature = "jetbrains-mono")]
    {
        use std::borrow::Cow;
        let fonts = vec![
            Cow::Borrowed(damascene_fonts_jetbrains_mono::JETBRAINS_MONO_VARIABLE),
            Cow::Borrowed(damascene_fonts_jetbrains_mono::JETBRAINS_MONO_VARIABLE_ITALIC),
        ];
        if let Err(error) = cx.text_system().add_fonts(fonts) {
            eprintln!("gpui-cn could not register JetBrains Mono: {error}");
        }
    }
    #[cfg(not(feature = "jetbrains-mono"))]
    {
        let _ = cx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_code_font_is_the_first_bundled_one() {
        let bundled = bundled_code_fonts();
        assert_eq!(default_code_font(), bundled.first().cloned());
        if cfg!(feature = "jetbrains-mono") {
            assert_eq!(bundled.first().map(|f| f.as_ref()), Some(JETBRAINS_MONO));
        } else {
            assert!(bundled.is_empty());
        }
    }
}
