//! Sandhill's look: the window is frosted glass (on a Mac the system blurs
//! what is behind it, and a neutral tint goes over the blur), with panes
//! floating on it as cards 3px apart under a 31px title bar of capsules.
//! The editor is solid for reading code; the side panels and Console group
//! are translucent. One 12px corner radius, Maple Mono NF for code, and
//! every text size 2px larger than the Tauri app's.
//!
//! Kit supplies the neutral surfaces; the crane icon's navy supplies interactive
//! accents, with a lighter counterpart for dark mode. Code keeps Xcode's
//! Default syntax colours. Kit widgets and custom drawing share these colours.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::rc::Rc;
use std::sync::OnceLock;

use gpui_kit::component::highlighter::SyntaxColors;
use gpui_kit::component::{Colorize, Theme, ThemeColor, ThemeConfig, ThemeMode, ThemeRegistry};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use rust_embed::RustEmbed;
use serde_json::{Value, json};

/// Window chrome text: the Tauri app's 13px, 2px larger.
pub const CHROME: f32 = 15.;
/// Code, in Maple Mono NF: 13px there.
pub const CODE: f32 = 15.;
/// Kernel and resource rows, popups and drop-downs: 11px there.
pub const SMALL: f32 = 13.;
/// Pane tab titles and toolbar buttons: 1pt below the chrome's text.
pub const TAB_TEXT: f32 = 14.;
/// A tab: a pill in a 24px track, both fully round (Huan, 2026-11-14).
pub const TAB_H: f32 = 20.;
/// The track pane tabs sit in: the tab's height and 2px around it.
pub const TAB_TRACK: f32 = 24.;
/// Names the hover group of every tab, so a close button can show under the
/// pointer. Lookups find the nearest enclosing tab.
pub const TAB_GROUP: &str = "sandhill-tab";
/// One corner radius for every rounded thing in the window.
pub const RADIUS: f32 = 12.;
/// The gap between panes, and between them and the window's top.
pub const GAP: f32 = 3.;
/// The room between the side panels and the window's left and right edges.
pub const EDGE: f32 = 7.;
pub const TITLE_BAR: f32 = 31.;
pub const STATUS_BAR: f32 = 26.;
/// A pane's tab strip.
pub const STRIP: f32 = 29.;
pub const MONO: &str = "Maple Mono NF";

/// Sandhill's colours for drawing that isn't a GPUI Kit widget.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Palette {
    pub dark: bool,
    pub bg: Hsla,
    pub chrome: Hsla,
    pub panel: Hsla,
    pub line: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub primary_text: Hsla,
    pub danger_text: Hsla,
    pub busy: Hsla,
    pub busy_soft: Hsla,
    pub ok: Hsla,
    pub err: Hsla,
    pub err_soft: Hsla,
    pub chunk: Hsla,
    pub sel: Hsla,
    pub hover: Hsla,
    pub term: Hsla,
    pub warn: Hsla,
    /// The tint over the window's blur.
    pub wall: Hsla,
    pub glass: Hsla,
    pub glass_edge: Hsla,
    pub glass_hi: Hsla,
    pub cap_bg: Hsla,
    pub cap_on: Hsla,
    pub shadow: Hsla,
    pub ansi: [Hsla; 16],
    pub chart: [Hsla; 5],
}

impl Global for Palette {}

/// Derive interactive colours from the crane mark's navy, leaving semantic,
/// chart, terminal ANSI and neutral surface colours at Kit's defaults.
fn brand_colors(dark: bool) -> ThemeColor {
    let mut c = *if dark {
        ThemeColor::dark()
    } else {
        ThemeColor::light()
    };
    let color = |value| Hsla::from(rgb(value));
    let (primary, hover, active, foreground) = if dark {
        (0x8dacd5, 0xa3bcdf, 0x7198ca, 0x142844)
    } else {
        (0x142844, 0x203e64, 0x0d1d32, 0xffffff)
    };
    c.primary = color(primary);
    c.primary_hover = color(hover);
    c.primary_active = color(active);
    c.primary_foreground = color(foreground);
    c.button_primary = c.primary;
    c.button_primary_hover = c.primary_hover;
    c.button_primary_active = c.primary_active;
    c.button_primary_foreground = c.primary_foreground;
    c.accent = c
        .background
        .blend(c.primary.opacity(if dark { 0.16 } else { 0.08 }));
    c.accent_foreground = c.foreground;
    c.ring = c.primary;
    c.link = c.primary;
    c.link_hover = c.primary_hover;
    c.link_active = c.primary_active;
    c.progress_bar = c.primary;
    c.slider_bar = c.primary;
    c.slider_thumb = c.primary_foreground;
    c.selection = c.primary.opacity(if dark { 0.26 } else { 0.18 });
    c.list_active = c.accent;
    c.list_active_border = c.primary;
    c.list_hover = c
        .background
        .blend(c.primary.opacity(if dark { 0.08 } else { 0.04 }));
    c.sidebar_accent = c.accent;
    c.sidebar_accent_foreground = c.foreground;
    c.sidebar_primary = c.primary;
    c.sidebar_primary_foreground = c.primary_foreground;
    c.tab_active = c.accent;
    c.table_active = c.accent;
    c.table_active_border = c.primary;
    c
}

impl Palette {
    fn new(dark: bool) -> Self {
        let colors = brand_colors(dark);
        let c = &colors;
        // Keep the existing glass/capsule opacities, using Kit's neutral
        // surfaces and foregrounds instead of a second colour palette.
        let alpha = |color: Hsla, a: f32| Hsla { a, ..color };
        Self {
            dark,
            bg: c.background,
            chrome: c.title_bar,
            panel: c.sidebar,
            line: c.border,
            text: c.foreground,
            muted: c.muted_foreground,
            faint: c.scrollbar_thumb,
            accent: c.primary,
            accent_soft: c.accent,
            primary_text: c.primary_foreground,
            danger_text: c.danger_foreground,
            busy: c.info,
            busy_soft: c.info_hover,
            ok: c.success,
            err: c.danger,
            err_soft: c.danger_hover,
            chunk: c.muted,
            sel: c.selection,
            hover: c.list_hover,
            term: c.background,
            warn: c.warning,
            wall: alpha(
                c.title_bar,
                if cfg!(target_os = "macos") {
                    166. / 255.
                } else {
                    1.
                },
            ),
            glass: alpha(c.background, 0.62),
            glass_edge: alpha(c.window_border, if dark { 140. / 255. } else { 26. / 255. }),
            glass_hi: alpha(
                if dark { c.foreground } else { c.background },
                if dark { 20. / 255. } else { 191. / 255. },
            ),
            // Tracks and title-bar capsules: a neutral wash with no edge; the
            // picked tab is a solid pill above it.
            cap_bg: alpha(c.foreground, if dark { 0.075 } else { 0.055 }),
            cap_on: if dark {
                alpha(c.foreground, 0.2)
            } else {
                c.background
            },
            shadow: alpha(
                if dark { c.background } else { c.foreground },
                if dark { 179. / 255. } else { 56. / 255. },
            ),
            // ANSI hues remain Kit defaults, independent of the brand accent.
            ansi: [
                if dark { c.background } else { c.foreground },
                c.red,
                c.green,
                c.yellow,
                c.blue,
                c.magenta,
                c.cyan,
                if dark {
                    c.foreground
                } else {
                    c.muted_foreground
                },
                c.muted_foreground,
                c.red_light,
                c.green_light,
                c.yellow_light,
                c.blue_light,
                c.magenta_light,
                c.cyan_light,
                if dark { c.foreground } else { c.background },
            ],
            chart: [c.chart_1, c.chart_2, c.chart_3, c.chart_4, c.chart_5],
        }
    }

    /// The text colour at `percent`% over what is behind: the Tauri app's
    /// `--ink-N`, for hovers and picked rows.
    pub fn ink(&self, percent: f32) -> Hsla {
        self.text.opacity(percent / 100.)
    }

    pub fn get(cx: &App) -> &Self {
        cx.global::<Self>()
    }
}

/// Only code syntax is app-owned: Xcode's Default themes, independent of
/// Kit's interface and editor chrome. Fixed captures keep import/call italics.
fn xcode_syntax(dark: bool) -> SyntaxColors {
    let [k, s, n, c, f, o, h, fm, arg, ty] = if dark {
        [
            0xfc5fa3, 0xfc6a5d, 0xd0bf69, 0x6c7986, 0x67b7a4, 0xdddddf, 0xffffff, 0x92a1b1,
            0x41a1c0, 0x5dd8ff,
        ]
    } else {
        [
            0x9b2393, 0xc41a16, 0x1c00cf, 0x5d6c79, 0x326d74, 0x262626, 0x000000, 0x4a5560,
            0x0f68a0, 0x0b4f79,
        ]
    };
    let hex = |v: u32| format!("#{v:06x}");
    let style = |v: u32| json!({ "color": hex(v) });
    serde_json::from_value(json!({
        "keyword": { "color": hex(k), "font_weight": 700 },
        "primary": { "color": hex(k), "font_weight": 700, "font_style": "italic" },
        "hint": { "color": hex(f), "font_style": "italic" },
        "string": style(s),
        "string.escape": style(s),
        "string.special": style(s),
        "string.special.symbol": style(s),
        "number": style(n),
        "boolean": style(n),
        "constant": style(n),
        "comment": style(c),
        "comment.doc": style(c),
        "function": style(f),
        "constructor": style(ty),
        "type": style(ty),
        "property": style(arg),
        "label": { "color": hex(arg), "font_style": "italic" },
        "operator": style(o),
        "punctuation": style(o),
        "punctuation.bracket": style(o),
        "punctuation.delimiter": style(o),
        "punctuation.special": style(fm),
        "punctuation.list_marker": style(fm),
        "title": { "color": hex(h), "font_weight": 700 },
        "emphasis": { "font_style": "italic" },
        "emphasis.strong": { "font_weight": 700 },
        "link_text": style(f),
        "link_uri": style(f),
        "text.literal": style(s),
        "embedded": style(o),
        "tag": style(k),
        "attribute": style(arg),
    }))
    .expect("the built-in Xcode syntax colours are valid")
}

/// Kit's neutral base with shared brand accents, existing transparency,
/// typography, geometry and independent Xcode code syntax.
fn theme_config(default: &ThemeConfig) -> Rc<ThemeConfig> {
    let mut config = default.clone();
    let dark = config.mode.is_dark();
    let palette = Palette::new(dark);
    let colors = brand_colors(dark);
    config.name = if dark {
        "Sandhill Dark"
    } else {
        "Sandhill Light"
    }
    .into();
    config.font_size = Some(CHROME);
    config.mono_font_family = Some(MONO.into());
    config.mono_font_size = Some(CODE);
    config.radius = Some(RADIUS as usize);
    config.radius_lg = Some(RADIUS as usize);
    config.shadow = Some(true);
    for (target, value) in [
        (&mut config.colors.primary, colors.primary),
        (&mut config.colors.primary_hover, colors.primary_hover),
        (&mut config.colors.primary_active, colors.primary_active),
        (
            &mut config.colors.primary_foreground,
            colors.primary_foreground,
        ),
        (&mut config.colors.button_primary, colors.button_primary),
        (
            &mut config.colors.button_primary_hover,
            colors.button_primary_hover,
        ),
        (
            &mut config.colors.button_primary_active,
            colors.button_primary_active,
        ),
        (
            &mut config.colors.button_primary_foreground,
            colors.button_primary_foreground,
        ),
        (&mut config.colors.accent, colors.accent),
        (
            &mut config.colors.accent_foreground,
            colors.accent_foreground,
        ),
        (&mut config.colors.ring, colors.ring),
        (&mut config.colors.link, colors.link),
        (&mut config.colors.link_hover, colors.link_hover),
        (&mut config.colors.link_active, colors.link_active),
        (&mut config.colors.progress_bar, colors.progress_bar),
        (&mut config.colors.slider_bar, colors.slider_bar),
        (&mut config.colors.slider_thumb, colors.slider_thumb),
        (&mut config.colors.selection, colors.selection),
        (&mut config.colors.list_active, colors.list_active),
        (
            &mut config.colors.list_active_border,
            colors.list_active_border,
        ),
        (&mut config.colors.list_hover, colors.list_hover),
        (&mut config.colors.sidebar_accent, colors.sidebar_accent),
        (
            &mut config.colors.sidebar_accent_foreground,
            colors.sidebar_accent_foreground,
        ),
        (&mut config.colors.sidebar_primary, colors.sidebar_primary),
        (
            &mut config.colors.sidebar_primary_foreground,
            colors.sidebar_primary_foreground,
        ),
        (&mut config.colors.tab_active, colors.tab_active),
        (&mut config.colors.table_active, colors.table_active),
        (
            &mut config.colors.table_active_border,
            colors.table_active_border,
        ),
    ] {
        let rgba = Rgba::from(value);
        *target = Some(
            format!(
                "#{:02X}{:02X}{:02X}{:02X}",
                (rgba.r * 255.).round() as u8,
                (rgba.g * 255.).round() as u8,
                (rgba.b * 255.).round() as u8,
                (rgba.a * 255.).round() as u8,
            )
            .into(),
        );
    }
    // These surfaces were already transparent so the existing window blur
    // and custom glass panes, rather than opaque widget layers, show through.
    for color in [
        &mut config.colors.background,
        &mut config.colors.list,
        &mut config.colors.list_even,
        &mut config.colors.sidebar,
        &mut config.colors.tab,
        &mut config.colors.tab_bar,
        &mut config.colors.title_bar,
        &mut config.colors.title_bar_border,
        &mut config.colors.status_bar,
        &mut config.colors.status_bar_border,
        &mut config.colors.scrollbar,
    ] {
        *color = Some("#00000000".into());
    }
    config.colors.group_box = Some(colors.group_box.opacity(0.62).to_hex().into());
    config.colors.tab_bar_segmented = Some(palette.cap_bg.to_hex().into());
    config.colors.window_border = Some(palette.glass_edge.to_hex().into());
    // Kit's switch thumb defaults to `background`, transparent above.
    config.colors.switch_thumb = Some(palette.bg.to_hex().into());
    let style = config
        .highlight
        .as_mut()
        .expect("Kit's default themes include editor styles");
    style.syntax = xcode_syntax(dark);
    // Kit tints diagnostic popovers over `background`, which is transparent
    // here for the glass, so code showed through the message. Tint the solid
    // editor surface instead.
    let mut status = serde_json::to_value(&style.status).unwrap_or_default();
    for (name, fallback) in [
        ("error", palette.err),
        ("warning", palette.warn),
        ("info", palette.accent),
        ("success", palette.ok),
        ("hint", palette.muted),
    ] {
        let tint = serde_json::from_value::<Hsla>(status[name].clone()).unwrap_or(fallback);
        status[format!("{name}.background")] =
            serde_json::to_value(palette.bg.blend(tint.alpha(0.2))).unwrap_or_default();
    }
    if let Ok(status) = serde_json::from_value(status) {
        style.status = status;
    }
    Rc::new(config)
}

/// Loads fonts and Kit's light/dark defaults once, then follows the selected
/// appearance. Call after `gpui_kit::init`.
pub fn install(cx: &mut App) {
    add_fonts(cx);
    let registry = ThemeRegistry::global(cx);
    let light = theme_config(registry.default_light_theme());
    let dark = theme_config(registry.default_dark_theme());
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    set_appearance(cx.window_appearance(), cx);
}

/// Switches the theme and the palette to the window's appearance, or to
/// `SANDHILL_APPEARANCE` (`light` or `dark`) when that is set, or to the
/// theme picked with the title bar's button.
pub fn set_appearance(appearance: WindowAppearance, cx: &mut App) {
    let picked = std::env::var("SANDHILL_APPEARANCE")
        .ok()
        .or_else(crate::prefs::appearance);
    let dark = match picked.as_deref() {
        Some("dark") => true,
        Some("light") => false,
        _ => matches!(
            appearance,
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ),
    };
    cx.set_global(Palette::new(dark));
    LIGHT_ICONS.store(!dark, std::sync::atomic::Ordering::Relaxed);
    // gpui-cn (the terminal's) keeps a theme of its own; it goes first, so
    // Kit's supported change API synchronizes components, tokens and Base last.
    // The shared root stays transparent; terminal colours are set separately
    // in `terminal::colors`.
    if cx.has_global::<gpui_cn::Theme>() {
        gpui_cn::Theme::update(cx, |theme| {
            theme.light.surface = transparent_black();
            theme.dark.surface = transparent_black();
            theme.mode = if dark {
                gpui_cn::ThemeMode::Dark
            } else {
                gpui_cn::ThemeMode::Light
            };
        });
    }
    Theme::change(
        if dark {
            ThemeMode::Dark
        } else {
            ThemeMode::Light
        },
        None,
        cx,
    );
}

/// The title bar's theme button: light to dark and back, kept for next
/// time.
pub fn toggle_appearance(window: &Window, cx: &mut App) {
    let dark = Palette::get(cx).dark;
    crate::prefs::save_appearance(if dark { "light" } else { "dark" });
    set_appearance(window.appearance(), cx);
    cx.refresh_windows();
}

const FONTS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/MapleMono-NF-Regular.ttf.gz"),
    include_bytes!("../assets/fonts/MapleMono-NF-Italic.ttf.gz"),
    include_bytes!("../assets/fonts/MapleMono-NF-SemiBold.ttf.gz"),
    include_bytes!("../assets/fonts/MapleMono-NF-Bold.ttf.gz"),
];

fn add_fonts(cx: &mut App) {
    let fonts = FONTS
        .iter()
        .filter_map(|gz| {
            let mut ttf = Vec::new();
            flate2::read::GzDecoder::new(*gz)
                .read_to_end(&mut ttf)
                .map_err(|err| tracing::warn!("a bundled font did not unpack: {err}"))
                .ok()?;
            Some(Cow::Owned(ttf))
        })
        .collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("Maple Mono did not load: {err:#}");
    }
}

/// The icons drawn in `docs/design/ui/icons` (by its `build.py`), served
/// under `sandhill/`, ahead of gpui-kit's own.
#[derive(RustEmbed)]
#[folder = "../docs/design/ui/icons/"]
#[include = "*/*.svg"]
#[include = "registry.json"]
#[exclude = "vendor/**"]
struct Icons;

pub struct Assets;

const PREFIX: &str = "sandhill/";

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix(PREFIX) else {
            return gpui_kit::assets::Assets.load(path);
        };
        Ok(Icons::get(rest).map(|file| file.data))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        match path.strip_prefix(PREFIX) {
            Some(rest) => Ok(Icons::iter()
                .filter(|p| p.starts_with(rest))
                .map(|p| format!("{PREFIX}{p}").into())
                .collect()),
            None => gpui_kit::assets::Assets.list(path),
        }
    }
}

/// A one-colour icon from `system/`, drawn in the text colour around it
/// (unless given one): `name` as the registry calls it, such as `run` or
/// `chevron.right`.
pub fn icon(name: &str) -> Icon {
    // The registry draws arrows up only; down is the same turned over.
    let (name, turned) = match name {
        "arrow.down" => ("arrow.up", true),
        name => (name, false),
    };
    Icon {
        path: format!("{PREFIX}system/{name}.svg").into(),
        size: px(16.),
        color: None,
        turned,
    }
}

/// A one-colour mark outside `system/` (`path` such as `agents/openscience`),
/// drawn in the text colour around it so it reads on light and dark.
pub fn ink_picture(path: &str) -> Icon {
    Icon {
        path: format!("{PREFIX}{path}.svg").into(),
        size: px(16.),
        color: None,
        turned: false,
    }
}

#[derive(IntoElement)]
pub struct Icon {
    path: SharedString,
    size: Pixels,
    color: Option<Hsla>,
    turned: bool,
}

impl Icon {
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn text_color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        svg()
            .path(self.path)
            .size(self.size)
            .flex_none()
            .text_color(self.color.unwrap_or_else(|| window.text_style().color))
            .when(self.turned, |s| {
                s.with_transformation(Transformation::rotate(radians(std::f32::consts::PI)))
            })
    }
}

/// File and folder icons use their light-theme versions (`*.light.svg`,
/// darkened for light panes) while the light theme is on.
static LIGHT_ICONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A coloured icon (file types, languages, folders, agents): `path` such
/// as `lang/r` or `bio/fastq`. In the light theme, its light version when
/// it has one.
pub fn picture(path: &str) -> Img {
    let light = format!("{path}.light.svg");
    let file =
        if LIGHT_ICONS.load(std::sync::atomic::Ordering::Relaxed) && Icons::get(&light).is_some() {
            light
        } else {
            format!("{path}.svg")
        };
    img(SharedString::from(format!("{PREFIX}{file}")))
        .size(px(16.))
        .flex_none()
}

/// How a file's or folder's icon is picked, from the registry `build.py`
/// writes (the Tauri app reads the same rules).
struct Resolve {
    compressed: Vec<String>,
    names: HashMap<String, String>,
    prefixes: HashMap<String, String>,
    extensions: HashMap<String, String>,
    fallback: String,
    folders: HashMap<String, String>,
    /// Folders that are one data set, by extension: shown with a file icon.
    folder_extensions: HashMap<String, String>,
    folder_fallback: String,
}

fn resolve() -> &'static Resolve {
    static RESOLVE: OnceLock<Resolve> = OnceLock::new();
    RESOLVE.get_or_init(|| {
        let registry: Value = Icons::get("registry.json")
            .and_then(|f| serde_json::from_slice(&f.data).ok())
            .unwrap_or_default();
        let r = &registry["resolve"];
        let map = |v: &Value| -> HashMap<String, String> {
            v.as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                        .collect()
                })
                .unwrap_or_default()
        };
        Resolve {
            compressed: r["compressed"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            names: map(&r["names"]),
            prefixes: map(&r["prefixes"]),
            extensions: map(&r["extensions"]),
            fallback: r["fallback"].as_str().unwrap_or("file").to_string(),
            folders: map(&r["folders"]),
            folder_extensions: map(&r["folderExtensions"]),
            folder_fallback: r["folderFallback"].as_str().unwrap_or("other").to_string(),
        }
    })
}

/// The icon for a file (`bio/…`), by its name.
pub fn file_icon(path: &str) -> String {
    let r = resolve();
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if let Some(icon) = r.names.get(&name) {
        return format!("bio/{icon}");
    }
    if let Some((_, icon)) = r
        .prefixes
        .iter()
        .find(|(p, _)| name.starts_with(p.as_str()))
    {
        return format!("bio/{icon}");
    }
    let mut parts: Vec<&str> = name.split('.').skip(1).collect();
    // sample.fastq.gz is a FASTQ file.
    if parts.len() > 1
        && parts
            .last()
            .is_some_and(|e| r.compressed.iter().any(|c| c == e))
    {
        parts.pop();
    }
    let icon = parts
        .last()
        .and_then(|ext| r.extensions.get(*ext))
        .unwrap_or(&r.fallback);
    format!("bio/{icon}")
}

/// The icon for a file among those of its folder. PLINK's binary genotypes
/// are `.bed` files too: what tells them from BED intervals is the `.bim`
/// or `.fam` of the same name beside them. `beside` says whether a path
/// (as `path` is given) is there.
pub fn file_icon_beside(path: &str, beside: impl Fn(&str) -> bool) -> String {
    let stem = path
        .get(path.len().saturating_sub(4)..)
        .filter(|end| end.eq_ignore_ascii_case(".bed"))
        .map(|_| &path[..path.len() - 4]);
    if let Some(stem) = stem
        && [".bim", ".fam"]
            .iter()
            .any(|ext| beside(&format!("{stem}{ext}")))
    {
        return file_icon("genotypes.bim");
    }
    file_icon(path)
}

/// The icon for a folder, by its name (`folders/…`); a folder that is one
/// data set, such as a Zarr store, has its format's file icon (`bio/…`).
pub fn folder_icon(name: &str) -> String {
    let r = resolve();
    let name = name.to_lowercase();
    if let Some(icon) = name
        .rsplit_once('.')
        .and_then(|(_, ext)| r.folder_extensions.get(ext))
    {
        return format!("bio/{icon}");
    }
    let icon = r.folders.get(&name).unwrap_or(&r.folder_fallback);
    format!("folders/{icon}")
}

/// The built-in folder and file icons a project can use instead of its
/// folder icon (`folders/…`, then `bio/…`), by path without `.svg`.
pub fn project_icon_choices() -> Vec<String> {
    let mut folders = Vec::new();
    let mut files = Vec::new();
    for path in Icons::iter() {
        // `*.light.svg` are the light theme's versions of the same icons.
        let Some(name) = path
            .strip_suffix(".svg")
            .filter(|name| !name.ends_with(".light"))
        else {
            continue;
        };
        if name.starts_with("folders/") {
            folders.push(name.to_string());
        } else if name.starts_with("bio/") {
            files.push(name.to_string());
        }
    }
    folders.sort();
    files.sort();
    folders.extend(files);
    folders
}

/// A project's icon on `host`: the one chosen for it, else the project
/// folder's (Huan, 2026-11-14), whatever the folder is called.
pub fn project_icon(host: &str, path: &str, _name: &str) -> String {
    crate::prefs::project_icon(host, path).unwrap_or_else(|| "folders/project".into())
}

/// The icon for a kernel's language (`lang/…`), when there is one.
pub fn language_icon(language: &str) -> Option<&'static str> {
    match language.to_lowercase().as_str() {
        "r" => Some("lang/r"),
        "python" => Some("lang/python"),
        "bash" | "sh" | "shell" => Some("lang/shell"),
        _ => None,
    }
}

/// The window between the panes: a neutral tint over the blur of what is
/// behind the window (see `app_window::open_window`).
pub fn wall(p: &Palette) -> Div {
    div().relative().bg(p.wall)
}

/// A solid pane floating on the glass, for the editor and dialogs.
/// A hairline edge and a soft shadow.
pub fn pane(p: &Palette) -> Div {
    card(p, p.bg).shadow(vec![
        BoxShadow {
            color: p.glass_hi,
            offset: point(px(0.), px(1.)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: true,
        },
        BoxShadow {
            color: p.shadow.opacity(0.13),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        },
        BoxShadow {
            color: p.shadow,
            offset: point(px(0.), px(10.)),
            blur_radius: px(26.),
            spread_radius: px(-12.),
            inset: false,
        },
    ])
}

/// A translucent pane floating on the glass, for the side panels: the
/// window's blur shows through it. Only a close shadow, which would
/// otherwise darken it from beneath.
pub fn side_pane(p: &Palette) -> Div {
    card(p, p.glass).shadow(vec![
        BoxShadow {
            color: p.glass_hi,
            offset: point(px(0.), px(1.)),
            blur_radius: px(0.),
            spread_radius: px(0.),
            inset: true,
        },
        BoxShadow {
            color: p.shadow.opacity(0.13),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        },
    ])
}

/// Headers use the surface's RGB, not its existing alpha: multiplying the
/// glass's 0.62 alpha by 0.82 would leave the toolbar less opaque than its pane.
/// The remaining 18% lets scrolling content show through without tinting text.
pub(crate) fn floating_header(
    p: &Palette,
    top: f32,
    solid: bool,
    header: impl IntoElement,
) -> AnyElement {
    let surface = if solid { p.bg } else { p.glass };
    div()
        .absolute()
        .top(px(top))
        .left_0()
        .right_0()
        .flex()
        .flex_col()
        .bg(Hsla { a: 0.82, ..surface })
        .border_b(px(0.5))
        .border_color(p.glass_edge)
        .occlude()
        .child(header)
        .into_any_element()
}

/// A scroll viewport fills the pane, including the area beneath its header.
/// Padding is inside the scrolling content; the scrollbar is a separate,
/// inset overlay, so neither its track nor hitbox occupies the header.
#[derive(IntoElement)]
pub(crate) struct PaneScroll {
    id: ElementId,
    inset: f32,
    padding: f32,
    body: Stateful<Div>,
    handle: Option<ScrollHandle>,
}

impl PaneScroll {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        inset: f32,
        padding: f32,
        body: Stateful<Div>,
    ) -> Self {
        Self {
            id: id.into(),
            inset,
            padding,
            body,
            handle: None,
        }
    }

    pub(crate) fn handle(mut self, handle: &ScrollHandle) -> Self {
        self.handle = Some(handle.clone());
        self
    }
}

impl RenderOnce for PaneScroll {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handle = self.handle.unwrap_or_else(|| {
            window
                .use_keyed_state(self.id.clone(), cx, |_, _| ScrollHandle::new())
                .read(cx)
                .clone()
        });
        div()
            .id((self.id, "viewport"))
            .relative()
            .flex_1()
            .min_h_0()
            .size_full()
            .overflow_hidden()
            .child(
                self.body
                    .size_full()
                    .pt(px(self.inset + self.padding))
                    .overflow_y_scroll()
                    .track_scroll(&handle),
            )
            .child(inset_scrollbar(&handle, self.inset, cx))
    }
}

pub(crate) fn inset_scrollbar<H: gpui_kit::component::scroll::ScrollbarHandle + Clone>(
    handle: &H,
    inset: f32,
    cx: &App,
) -> Div {
    let thumb = Palette::get(cx).faint;
    div()
        .absolute()
        .top(px(inset))
        .bottom_0()
        .left_0()
        .right_0()
        .child(
            gpui_kit::component::scroll::Scrollbar::vertical(handle)
                .viewport_from_layout()
                .styles(move |styles| styles.thumb(move |style| style.bg(thumb))),
        )
}

/// Minimal scroll to expose a target below an overlaid header. Oversize cells
/// align at the safe top instead of hiding their beginning behind the toolbar.
pub(crate) fn header_scroll_offset(
    offset: Pixels,
    target_top: Pixels,
    target_bottom: Pixels,
    viewport: Pixels,
    inset: Pixels,
    maximum: Pixels,
) -> Pixels {
    let top = target_top + offset;
    let bottom = target_bottom + offset;
    let next = if top < inset || target_bottom - target_top > viewport - inset {
        inset - target_top
    } else if bottom > viewport {
        viewport - target_bottom
    } else {
        offset
    };
    next.clamp(-maximum, px(0.))
}

#[cfg(test)]
mod header_tests {
    use super::header_scroll_offset;
    use gpui_kit::px;

    #[test]
    fn header_scroll_offsets_keep_targets_in_the_visible_body() {
        let scroll = |offset, top, bottom, viewport, max| {
            header_scroll_offset(
                px(offset),
                px(top),
                px(bottom),
                px(viewport),
                px(29.),
                px(max),
            )
        };
        assert_eq!(scroll(-100., 110., 160., 300., 600.), px(-81.));
        assert_eq!(scroll(-100., 150., 200., 300., 600.), px(-100.));
        assert_eq!(scroll(-100., 380., 430., 300., 600.), px(-130.));
        assert_eq!(scroll(-100., 150., 650., 300., 600.), px(-121.));
        assert_eq!(scroll(0., 10., 50., 300., 600.), px(0.));
        assert_eq!(scroll(-500., 900., 950., 300., 600.), px(-600.));
        for inset in [29. + 37., 29. + 65.] {
            assert_eq!(
                header_scroll_offset(px(-100.), px(110.), px(160.), px(300.), px(inset), px(600.),),
                px(inset - 110.),
            );
        }
    }
}

fn card(p: &Palette, bg: Hsla) -> Div {
    div()
        .flex()
        .flex_col()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .bg(bg)
        .border_1()
        .border_color(p.glass_edge)
        .rounded(px(RADIUS))
}

/// A capsule, as in the title bar and around a pane's tabs: a pill with a
/// light wash and no edge, so only the picked tab stands out.
pub fn capsule(p: &Palette, height: f32) -> Div {
    div()
        .flex()
        .items_center()
        .flex_none()
        .h(px(height))
        .px(px(2.))
        .rounded_full()
        .bg(p.cap_bg)
}

/// The line between groups in a capsule.
pub fn capsule_divider(p: &Palette) -> Div {
    div()
        .w(px(1.))
        .h(px(14.))
        .mx(px(3.))
        .flex_none()
        .bg(p.ink(16.))
}

/// The picked tab in a capsule: a solid pill on a soft shadow, no edge.
pub fn raised(d: Stateful<Div>, p: &Palette) -> Stateful<Div> {
    let shade = |a: f32, y: f32, blur: f32, spread: f32| BoxShadow {
        color: Hsla { a, ..p.shadow },
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    };
    d.bg(p.cap_on).text_color(p.text).shadow(if p.dark {
        vec![shade(0.35, 1., 2., 0.)]
    } else {
        vec![shade(0.06, 0., 0., 0.5), shade(0.1, 1., 3., 0.)]
    })
}

/// The track a pane's tabs sit in.
pub fn tab_capsule(p: &Palette) -> Div {
    capsule(p, TAB_TRACK)
}

/// A tab's close button: a 16px circle. `shown` draws the cross always (the
/// picked tab, after its label); otherwise only while the pointer is over
/// the enclosing [`TAB_GROUP`], in the icon's place (see [`tab_lead`]).
pub fn tab_close(id: impl Into<ElementId>, shown: bool, p: &Palette) -> Stateful<Div> {
    close_circle(id, p).child(
        div()
            .flex()
            .when(!shown, |d| {
                d.invisible().group_hover(TAB_GROUP, |s| s.visible())
            })
            .child(icon("close").size(px(12.))),
    )
}

/// The empty 16px circle of a tab's close button, lit under the pointer.
/// The cross is muted, not faint, so it reads on hovered tabs in both
/// themes (Huan, 2026-11-14).
pub fn close_circle(id: impl Into<ElementId>, p: &Palette) -> Stateful<Div> {
    let hover = p.ink(10.);
    let text = p.text;
    div()
        .id(id)
        .size(px(16.))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(p.muted)
        .cursor_pointer()
        .hover(move |d| d.bg(hover).text_color(text))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// An unpicked tab's icon, with its close button over it: under the pointer
/// the cross takes the icon's place, as in Finder and Xcode, so hidden
/// buttons reserve no room and nothing moves.
pub fn tab_lead(lead: impl IntoElement, close: Stateful<Div>) -> AnyElement {
    div()
        .relative()
        .flex()
        .flex_none()
        .size(px(16.))
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .group_hover(TAB_GROUP, |s| s.invisible())
                .child(lead),
        )
        .child(close.absolute().top_0().left_0())
        .into_any_element()
}

/// Puts a tab's close button after its label when `trailing` (the picked
/// tab, or an unsaved one with its dot), otherwise over its icon.
pub fn place_close(
    lead: AnyElement,
    close: Stateful<Div>,
    trailing: bool,
) -> (AnyElement, Option<Stateful<Div>>) {
    if trailing {
        (lead, Some(close))
    } else {
        (tab_lead(lead, close), None)
    }
}

/// A small icon button, as in a pane's head or the title bar.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: &str,
    p: &Palette,
    round: bool,
) -> Stateful<Div> {
    let hover = p.ink(7.);
    let text = p.text;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(if round { 28. } else { 22. }))
        .h(px(if round { 21. } else { 22. }))
        .when(round, |d| d.rounded_full())
        .when(!round, |d| d.rounded(px(RADIUS)))
        .text_color(p.muted)
        .cursor_pointer()
        .hover(move |d| d.bg(hover).text_color(text))
        .child(icon(name))
}

/// A button of words in a bar, such as Run All: muted until pointed at.
pub fn bar_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon_name: Option<&str>,
    p: &Palette,
) -> Stateful<Div> {
    let hover = p.ink(8.);
    let text = p.text;
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(5.))
        .flex_none()
        .h(px(22.))
        .px(px(8.))
        .rounded(px(RADIUS))
        .text_size(px(TAB_TEXT))
        .text_color(p.muted)
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(move |d| d.bg(hover).text_color(text))
        .when_some(icon_name, |d, name| d.child(icon(name).size(px(14.))))
        .child(label.into())
}

/// A coloured dot: a kernel's or a connection's state.
pub fn dot(color: Hsla) -> Div {
    div().size(px(7.)).flex_none().rounded_full().bg(color)
}

#[cfg(test)]
mod tests {
    use super::{Icons, file_icon, file_icon_beside, folder_icon, language_icon, resolve};

    fn luminance(color: gpui_kit::Hsla) -> f32 {
        let color = gpui_kit::Rgba::from(color);
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    #[test]
    fn brand_controls_keep_readable_text_in_both_modes_and_press_states() {
        for dark in [false, true] {
            let colors = super::brand_colors(dark);
            for background in [colors.primary, colors.primary_hover, colors.primary_active] {
                let a = luminance(background);
                let b = luminance(colors.primary_foreground);
                assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
            }
            for link in [colors.link, colors.link_hover, colors.link_active] {
                let a = luminance(colors.background);
                let b = luminance(link);
                assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
            }
        }
    }

    #[test]
    fn picks_icons() {
        assert_eq!(file_icon("analysis/de.qmd"), "bio/quarto");
        assert_eq!(file_icon("reads/S1_R1.fastq.gz"), "bio/fastq");
        assert_eq!(file_icon("README.md"), "bio/readme");
        assert_eq!(file_icon("Snakefile"), "bio/snakemake");
        assert_eq!(file_icon("notes.unknown"), "bio/file");
        assert_eq!(file_icon("figures/umap.pdf"), "bio/pdf");
        assert_eq!(file_icon("figures/umap.svg"), "bio/svg");
        assert_eq!(folder_icon("Data"), "folders/data");
        assert_eq!(folder_icon("misc"), "folders/other");
        // A Zarr store is a folder that is one data set.
        assert_eq!(folder_icon("atlas.zarr"), "bio/hdf5");
        // PLINK genotypes are told from BED intervals by their companions.
        let beside = |path: &str| path == "gwas/Cohort.fam";
        assert_eq!(file_icon_beside("gwas/Cohort.BED", beside), "bio/variants");
        assert_eq!(file_icon_beside("gwas/peaks.bed", beside), "bio/intervals");
        assert_eq!(file_icon_beside("gwas/Cohort.fam", beside), "bio/variants");
        assert_eq!(language_icon("R"), Some("lang/r"));
        // Every icon these name is bundled.
        for path in [
            "bio/quarto",
            "bio/fastq",
            "bio/file",
            "folders/data",
            "lang/r",
            "system/run",
        ] {
            assert!(Icons::get(&format!("{path}.svg")).is_some(), "{path}");
        }
        // Every file and folder icon the rules can name has its light-theme
        // version, so none falls back to the dark artwork on light panes.
        let r = resolve();
        let named = r
            .extensions
            .values()
            .chain(r.names.values())
            .chain([&r.fallback])
            .chain(r.folder_extensions.values())
            .map(|icon| format!("bio/{icon}"))
            .chain(
                r.folders
                    .values()
                    .chain([&r.folder_fallback])
                    .map(|icon| format!("folders/{icon}")),
            );
        for path in named {
            assert!(Icons::get(&format!("{path}.light.svg")).is_some(), "{path}");
        }
    }
}
