//! The code font and the two text sizes picked on Welcome → General (Huan,
//! 2026-11-18). The interface always uses the system font. Views read the
//! values through `look`'s size functions, so a change shows in every
//! window at the next frame.

use std::ops::RangeInclusive;
#[cfg(not(target_os = "macos"))]
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};

use gpui_kit::*;

/// The Mac's own code font, SF Mono. CoreText offers it under this hidden
/// family name with all its weights and italics, as it offers the
/// interface font as `.SystemUIFont`; no font file is needed.
#[cfg(target_os = "macos")]
const SYSTEM_MONO: &str = ".AppleSystemUIFontMonospaced";

/// Both sizes start here: the Tauri app's 13px, 2px larger.
pub const DEFAULT_SIZE: f32 = 15.;
pub const CODE_SIZES: RangeInclusive<f32> = 10.0..=24.0;
pub const UI_SIZES: RangeInclusive<f32> = 12.0..=18.0;

static CODE_SIZE: AtomicU32 = AtomicU32::new(DEFAULT_SIZE.to_bits());
static UI_SIZE: AtomicU32 = AtomicU32::new(DEFAULT_SIZE.to_bits());
/// The code font picked, `None` for the system's.
static CODE_FONT: RwLock<Option<SharedString>> = RwLock::new(None);
/// Elsewhere than a Mac: Kit's own monospaced default, which it has already
/// checked is installed.
#[cfg(not(target_os = "macos"))]
static OTHER_SYSTEM_MONO: OnceLock<SharedString> = OnceLock::new();

/// What General shows and saves: the code font (`None` for the system's)
/// and the two sizes. A global, so views that keep their own copy of the
/// font (the terminal) can observe changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Typography {
    pub code_font: Option<String>,
    pub code_size: f32,
    pub ui_size: f32,
    /// A saved code font that is no longer installed; code uses the system
    /// font instead, and General says so.
    pub missing: Option<String>,
}

impl Default for Typography {
    fn default() -> Self {
        Typography {
            code_font: None,
            code_size: DEFAULT_SIZE,
            ui_size: DEFAULT_SIZE,
            missing: None,
        }
    }
}

impl Global for Typography {}

/// The code font's family, as GPUI looks it up.
pub fn mono() -> SharedString {
    if let Some(font) = CODE_FONT.read().ok().and_then(|font| font.clone()) {
        return font;
    }
    system_mono()
}

fn system_mono() -> SharedString {
    #[cfg(target_os = "macos")]
    {
        SharedString::new_static(SYSTEM_MONO)
    }
    #[cfg(not(target_os = "macos"))]
    {
        OTHER_SYSTEM_MONO
            .get()
            .cloned()
            .unwrap_or_else(|| SharedString::new_static(".ZedMono"))
    }
}

/// The code font for a web page's CSS: the picked font first, then the
/// system's (`ui-monospace` is SF Mono in WebKit).
pub fn mono_css() -> String {
    let picked = CODE_FONT.read().ok().and_then(|font| font.clone());
    match picked {
        Some(font) => format!(
            "\"{}\", ui-monospace, Menlo, monospace",
            font.replace('"', "")
        ),
        None => "ui-monospace, Menlo, monospace".into(),
    }
}

/// Code text size.
pub fn code() -> f32 {
    f32::from_bits(CODE_SIZE.load(Ordering::Relaxed))
}

/// How far the interface size is from the default: every interface text
/// size, and the heights of rows, tabs and buttons, move by this much.
pub fn shift() -> f32 {
    f32::from_bits(UI_SIZE.load(Ordering::Relaxed)) - DEFAULT_SIZE
}

/// Reads the saved choice and uses it. A code font no longer installed is
/// not used (GPUI panics on the first line it lays out in a missing
/// family) but stays saved, in case it is installed again.
pub(super) fn load(cx: &mut App) {
    #[cfg(not(target_os = "macos"))]
    {
        use gpui_kit::component::ActiveTheme;
        let _ = OTHER_SYSTEM_MONO.set(cx.theme().mono_font_family.clone());
    }
    use_typography(crate::prefs::typography(), cx);
}

/// Saves and uses what General picked, then rebuilds the theme with it,
/// which redraws every window.
pub fn choose_typography(
    code_font: Option<String>,
    code_size: f32,
    ui_size: f32,
    window: &Window,
    cx: &mut App,
) {
    let picked = clamped(Typography {
        code_font,
        code_size,
        ui_size,
        missing: None,
    });
    crate::prefs::save_typography(&picked);
    use_typography(picked, cx);
    super::build_themes(cx);
    super::set_appearance(window.appearance(), cx);
}

fn clamped(mut picked: Typography) -> Typography {
    picked.code_size = picked
        .code_size
        .clamp(*CODE_SIZES.start(), *CODE_SIZES.end());
    picked.ui_size = picked.ui_size.clamp(*UI_SIZES.start(), *UI_SIZES.end());
    picked
}

fn use_typography(picked: Typography, cx: &mut App) {
    let mut picked = clamped(picked);
    let font = picked.code_font.clone().filter(|font| {
        let installed = cx
            .text_system()
            .all_font_names()
            .iter()
            .any(|name| name == font);
        if !installed {
            tracing::warn!("the code font {font:?} is not installed; using the system's");
            picked.missing = Some(font.clone());
        }
        installed
    });
    if let Ok(mut code_font) = CODE_FONT.write() {
        *code_font = font.map(SharedString::from);
    }
    CODE_SIZE.store(picked.code_size.to_bits(), Ordering::Relaxed);
    UI_SIZE.store(picked.ui_size.to_bits(), Ordering::Relaxed);
    cx.set_global(picked);
}

/// The installed monospaced font families, by name, for General's list.
/// A font counts when a narrow, a wide and a punctuation letter are equally
/// wide, so it need not call itself monospaced. Slow (it opens every
/// family): call off the main thread.
#[cfg(target_os = "macos")]
pub fn monospaced_families() -> Vec<String> {
    use core_text::font_collection;

    let mut families: Vec<String> = font_collection::get_family_names()
        .iter()
        .map(|name| name.to_string())
        .filter(|name| !name.starts_with('.') && monospaced(name))
        .collect();
    families.sort_by_key(|name| name.to_lowercase());
    families.dedup();
    families
}

#[cfg(not(target_os = "macos"))]
pub fn monospaced_families() -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn monospaced(family: &str) -> bool {
    use core_graphics::geometry::CGSize;
    use core_text::font;
    use core_text::font_collection;
    use core_text::font_descriptor::kCTFontOrientationDefault;

    let Some(face) = font_collection::create_for_family(family)
        .and_then(|collection| collection.get_descriptors())
        .and_then(|faces| faces.get(0).map(|face| (*face).clone()))
    else {
        return false;
    };
    let font = font::new_from_descriptor(&face, 16.);
    let letters = ['i' as u16, 'M' as u16, '.' as u16];
    let mut glyphs = [0u16; 3];
    // SAFETY: both buffers hold `letters.len()` items, as CoreText is told.
    let found = unsafe {
        font.get_glyphs_for_characters(letters.as_ptr(), glyphs.as_mut_ptr(), letters.len() as _)
    };
    if !found {
        return false;
    }
    let mut advances = [CGSize::new(0., 0.); 3];
    // SAFETY: as above, `glyphs.len()` glyphs in and advances out.
    unsafe {
        font.get_advances_for_glyphs(
            kCTFontOrientationDefault,
            glyphs.as_ptr(),
            advances.as_mut_ptr(),
            glyphs.len() as _,
        );
    }
    let first = advances[0].width;
    first > 0.
        && advances
            .iter()
            .all(|advance| (advance.width - first).abs() < 0.01)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::monospaced;

    #[test]
    fn tells_monospaced_fonts_from_others() {
        assert!(monospaced("Menlo"));
        assert!(monospaced("Courier New"));
        assert!(!monospaced("Helvetica"));
        assert!(!monospaced("Times New Roman"));
        assert!(!monospaced("No Such Font Anywhere"));
    }
}
