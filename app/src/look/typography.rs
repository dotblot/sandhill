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
/// Elsewhere than a Mac: the desktop's code and interface fonts (GNOME's
/// settings, else fontconfig's defaults), when installed; otherwise Kit's
/// monospaced default and GPUI's system font name.
#[cfg(not(target_os = "macos"))]
static OTHER_SYSTEM_MONO: OnceLock<SharedString> = OnceLock::new();
#[cfg(not(target_os = "macos"))]
static OTHER_SYSTEM_UI: OnceLock<SharedString> = OnceLock::new();

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

/// The system's code font: SF Mono on a Mac, the desktop's elsewhere.
pub fn system_mono() -> SharedString {
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

/// The interface font, which is always the system's.
pub fn ui_font() -> SharedString {
    #[cfg(target_os = "macos")]
    {
        SharedString::new_static(".SystemUIFont")
    }
    #[cfg(not(target_os = "macos"))]
    {
        OTHER_SYSTEM_UI
            .get()
            .cloned()
            .unwrap_or_else(|| SharedString::new_static(".SystemUIFont"))
    }
}

/// The code font for a web page's CSS: the picked font first, then the
/// system's (`ui-monospace` is SF Mono in WebKit; WebKitGTK knows neither
/// it nor Menlo, so there the desktop's font is named).
pub fn mono_css() -> String {
    let picked = CODE_FONT.read().ok().and_then(|font| font.clone());
    let system = if cfg!(target_os = "macos") {
        "ui-monospace, Menlo, monospace".to_string()
    } else {
        format!("\"{}\", monospace", system_mono().replace('"', ""))
    };
    match picked {
        Some(font) => format!("\"{}\", {system}", font.replace('"', "")),
        None => system,
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
        let installed = cx.text_system().all_font_names();
        let usable = |font: &Option<String>| {
            font.clone()
                .filter(|font| installed.iter().any(|name| name == font))
        };
        let mono = usable(&desktop_font("monospace-font-name", "monospace"))
            .map_or_else(|| cx.theme().mono_font_family.clone(), Into::into);
        let _ = OTHER_SYSTEM_MONO.set(mono);
        if let Some(ui) = usable(&desktop_font("font-name", "sans-serif")) {
            let _ = OTHER_SYSTEM_UI.set(ui.into());
        }
    }
    use_typography(crate::prefs::typography(), cx);
}

/// A font family the desktop names: GNOME's setting `key` (such as
/// `'Ubuntu Sans Mono 11'`, a family then a size), else fontconfig's match
/// for `generic`.
#[cfg(not(target_os = "macos"))]
fn desktop_font(key: &str, generic: &str) -> Option<String> {
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    run("gsettings", &["get", "org.gnome.desktop.interface", key])
        .and_then(|setting| gnome_family(&setting))
        .or_else(|| run("fc-match", &["--format", "%{family[0]}", generic]))
        .filter(|family| !family.is_empty())
}

/// The family in a GNOME font setting: quotes and the size dropped.
#[cfg(any(not(target_os = "macos"), test))]
fn gnome_family(setting: &str) -> Option<String> {
    let name = setting.trim().trim_matches('\'');
    let family = match name.rsplit_once(' ') {
        Some((family, size)) if size.parse::<f32>().is_ok() => family,
        _ => name,
    };
    Some(family.trim().to_string()).filter(|family| !family.is_empty())
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

/// Fontconfig's monospaced families with Latin letters (dual width too, as
/// CJK code fonts are; emoji fonts count as monospaced but have no
/// letters). Fontconfig works a font's spacing out from its letters' widths
/// when it scans it, so here too a font need not call itself monospaced.
#[cfg(not(target_os = "macos"))]
pub fn monospaced_families() -> Vec<String> {
    let mut families: Vec<String> = [":spacing=mono:lang=en", ":spacing=dual:lang=en"]
        .iter()
        .filter_map(|pattern| {
            std::process::Command::new("fc-list")
                .args(["--format", "%{family[0]}\n", pattern])
                .output()
                .ok()
        })
        .flat_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|name| name.trim().to_string())
                .collect::<Vec<_>>()
        })
        .filter(|name| !name.is_empty() && !name.starts_with('.'))
        .collect();
    families.sort_by_key(|name| name.to_lowercase());
    families.dedup();
    families
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

#[cfg(test)]
mod gnome_tests {
    use super::gnome_family;

    #[test]
    fn a_gnome_font_setting_gives_its_family() {
        assert_eq!(
            gnome_family("'Ubuntu Sans Mono 11'").as_deref(),
            Some("Ubuntu Sans Mono")
        );
        assert_eq!(
            gnome_family("'Cantarell 10.5'").as_deref(),
            Some("Cantarell")
        );
        // A family whose name ends in a word, with no size given.
        assert_eq!(
            gnome_family("'Source Code Pro'").as_deref(),
            Some("Source Code Pro")
        );
        assert_eq!(gnome_family("''"), None);
    }
}
