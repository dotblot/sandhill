//! Sandhill's look, carried over from the Tauri app (`app/src/styles.css`,
//! from the UI mockup in `docs/design/ui/`): glass panes 3px apart on a warm
//! wall under a 31px title bar of capsules, the left pane directly on the
//! wall, one 12px corner radius, and Maple Mono NF for code. Every text size
//! is 2px larger than the Tauri app's.
//!
//! The colours are the Tauri app's tokens, light and dark. They reach
//! gpui-kit's widgets as a theme (built here, so the tokens are written once)
//! and Sandhill's own drawing through [`Palette`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
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
/// A tab's corners, and those of the strip around them (half a pill's).
pub const TAB_RADIUS: f32 = 4.5;
/// One corner radius for every rounded thing in the window.
pub const RADIUS: f32 = 12.;
/// The gap between panes, and between them and the window's top.
pub const GAP: f32 = 3.;
/// The room between the side panels and the window's left and right edges.
pub const EDGE: f32 = 8.;
pub const TITLE_BAR: f32 = 31.;
pub const STATUS_BAR: f32 = 26.;
/// A pane's tab strip.
pub const STRIP: f32 = 29.;
pub const MONO: &str = "Maple Mono NF";

/// The colour tokens of one appearance, as `0xRRGGBB`.
struct Tokens {
    bg: u32,
    chrome: u32,
    panel: u32,
    line: u32,
    text: u32,
    muted: u32,
    faint: u32,
    accent: u32,
    accent_soft: u32,
    accent_text: u32,
    busy: u32,
    busy_soft: u32,
    ok: u32,
    err: u32,
    err_soft: u32,
    chunk: u32,
    sel: u32,
    hover: u32,
    term: u32,
    // Syntax: keywords, strings, numbers, comments, functions, operators,
    // headings, fences and markers, argument names.
    k: u32,
    s: u32,
    n: u32,
    c: u32,
    f: u32,
    o: u32,
    h: u32,
    fm: u32,
    arg: u32,
    /// The wall: a glow at the top left, one at the bottom right, and the
    /// gradient under them.
    glow_top: u32,
    glow_bottom: u32,
    wall_from: u32,
    wall_to: u32,
    /// How opaque the glass panes are.
    glass: f32,
    /// `0xRRGGBBAA`.
    glass_edge: u32,
    /// The light along a pane's top edge, `0xRRGGBBAA`.
    glass_hi: u32,
    cap_bg: u32,
    cap_edge: u32,
    cap_on: u32,
    shadow: u32,
    /// The terminal's 16 colours (black, red, green, yellow, blue,
    /// magenta, cyan, white, then the bright ones), as in the Tauri app.
    ansi: [u32; 16],
}

const LIGHT: Tokens = Tokens {
    bg: 0xffffff,
    chrome: 0xeeebe7,
    panel: 0xf7f5f2,
    line: 0xe0dbd5,
    text: 0x1f1c1a,
    muted: 0x6b645e,
    faint: 0xa39c95,
    accent: 0xc2570c,
    accent_soft: 0xfbe6d4,
    accent_text: 0xffffff,
    busy: 0x2f6fbf,
    busy_soft: 0xdde9f7,
    ok: 0x2e7d4f,
    err: 0xcc3a2f,
    err_soft: 0xfbe3e0,
    chunk: 0xf8f6f3,
    sel: 0xf6dcc4,
    hover: 0xe8e4df,
    term: 0xfbfaf8,
    k: 0x8f3fa3,
    s: 0x2f7d32,
    n: 0xb0560a,
    c: 0x8b928d,
    f: 0x1f5fa8,
    o: 0x59605b,
    h: 0x1c1f1d,
    fm: 0x9aa19c,
    arg: 0x0e6f78,
    glow_top: 0xf5d2b2,
    glow_bottom: 0xc9e0da,
    wall_from: 0xeee7df,
    wall_to: 0xe3ddd6,
    glass: 0.76,
    glass_edge: 0x462d141f,
    glass_hi: 0xffffffbf,
    // The text colour at 6% over 40% of the background.
    cap_bg: 0xffffff5c,
    cap_edge: 0x3c281429,
    cap_on: 0xffffffff,
    shadow: 0x462d1447,
    ansi: [
        0x1f1c1a, 0xc4352b, 0x2f7d32, 0x9a6300, 0x1f5fa8, 0x8f3fa3, 0x0e6f78, 0xa39c95, 0x6b645e,
        0xd9473c, 0x3d9642, 0xb07a10, 0x2f72c0, 0xa552b8, 0x16838d, 0xc9c2bb,
    ],
};

const DARK: Tokens = Tokens {
    bg: 0x1e1b19,
    chrome: 0x161412,
    panel: 0x221f1d,
    line: 0x34302c,
    text: 0xebe6e1,
    muted: 0xa49c95,
    faint: 0x6c655f,
    accent: 0xf29a45,
    accent_soft: 0x43291a,
    accent_text: 0x1e1206,
    busy: 0x7fb4ff,
    busy_soft: 0x1d2b3d,
    ok: 0x6fcf8f,
    err: 0xff6b5e,
    err_soft: 0x3d2120,
    chunk: 0x252220,
    sel: 0x4a3020,
    hover: 0x2a2624,
    term: 0x1c1917,
    k: 0xd19ae8,
    s: 0x9fd28f,
    n: 0xf2b373,
    c: 0x737b76,
    f: 0x7fb4ff,
    o: 0xc3c9c5,
    h: 0xf2f4f2,
    fm: 0x5f6763,
    arg: 0x72d0d8,
    glow_top: 0x4b2a14,
    glow_bottom: 0x14322d,
    wall_from: 0x1b1816,
    wall_to: 0x110f0e,
    glass: 0.78,
    glass_edge: 0x0000008c,
    glass_hi: 0xffffff12,
    cap_bg: 0xffffff0f,
    cap_edge: 0xffffff1a,
    cap_on: 0xffffff21,
    shadow: 0x000000b3,
    ansi: [
        0x3a3531, 0xff6b5e, 0x9fd28f, 0xf2c46d, 0x7fb4ff, 0xd19ae8, 0x72d0d8, 0xd8d2cc, 0x6c655f,
        0xff8a7f, 0xb8e3aa, 0xf7d592, 0xa3c9ff, 0xe2b8f2, 0x9ae0e6, 0xf5f1ed,
    ],
};

/// Sandhill's colours for the current appearance, for drawing that isn't a
/// gpui-kit widget. All of the Tauri app's tokens, some not used yet.
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
    pub accent_text: Hsla,
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
    pub glow_top: Hsla,
    pub glow_bottom: Hsla,
    pub wall_from: Hsla,
    pub wall_to: Hsla,
    pub glass: Hsla,
    pub glass_edge: Hsla,
    pub glass_hi: Hsla,
    pub cap_bg: Hsla,
    pub cap_edge: Hsla,
    pub cap_on: Hsla,
    pub shadow: Hsla,
    pub ansi: [Hsla; 16],
}

impl Global for Palette {}

impl Palette {
    fn of(t: &Tokens, dark: bool) -> Self {
        let c = |v: u32| -> Hsla { rgb(v).into() };
        let a = |v: u32| -> Hsla { rgba(v).into() };
        Self {
            dark,
            bg: c(t.bg),
            chrome: c(t.chrome),
            panel: c(t.panel),
            line: c(t.line),
            text: c(t.text),
            muted: c(t.muted),
            faint: c(t.faint),
            accent: c(t.accent),
            accent_soft: c(t.accent_soft),
            accent_text: c(t.accent_text),
            busy: c(t.busy),
            busy_soft: c(t.busy_soft),
            ok: c(t.ok),
            err: c(t.err),
            err_soft: c(t.err_soft),
            chunk: c(t.chunk),
            sel: c(t.sel),
            hover: c(t.hover),
            term: c(t.term),
            warn: c(t.n),
            glow_top: c(t.glow_top),
            glow_bottom: c(t.glow_bottom),
            wall_from: c(t.wall_from),
            wall_to: c(t.wall_to),
            glass: c(t.bg).opacity(t.glass),
            glass_edge: a(t.glass_edge),
            glass_hi: a(t.glass_hi),
            cap_bg: a(t.cap_bg),
            cap_edge: a(t.cap_edge),
            cap_on: a(t.cap_on),
            shadow: a(t.shadow),
            ansi: t.ansi.map(c),
        }
    }

    pub fn light() -> Self {
        Self::of(&LIGHT, false)
    }

    pub fn dark() -> Self {
        Self::of(&DARK, true)
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

fn hex(v: u32) -> String {
    format!("#{v:06x}")
}

fn hexa(v: u32) -> String {
    format!("#{v:08x}")
}

/// gpui-kit's theme for one appearance, from the same tokens.
fn theme_config(name: &str, mode: &str, t: &Tokens) -> Value {
    let glass = (t.glass * 255.) as u32;
    let style = |v: u32| json!({ "color": hex(v) });
    json!({
        "name": name,
        "mode": mode,
        "font.size": CHROME,
        "mono_font.family": MONO,
        "mono_font.size": CODE,
        "radius": RADIUS as usize,
        "radius.lg": RADIUS as usize,
        "shadow": true,
        "colors": {
            "background": hex(t.bg),
            "foreground": hex(t.text),
            "border": hex(t.line),
            "caret": hex(t.text),
            "ring": hex(t.accent),
            "selection.background": hex(t.sel),
            "muted.background": hex(t.panel),
            "muted.foreground": hex(t.muted),
            "accent.background": hex(t.hover),
            "accent.foreground": hex(t.text),
            "primary.background": hex(t.accent),
            "primary.hover.background": hex(t.accent),
            "primary.active.background": hex(t.accent),
            "primary.foreground": hex(t.accent_text),
            "secondary.background": hex(t.panel),
            "secondary.hover.background": hex(t.hover),
            "secondary.active.background": hex(t.hover),
            "secondary.foreground": hex(t.text),
            "danger.background": hex(t.err),
            "danger.foreground": "#ffffff",
            "success.background": hex(t.ok),
            "success.foreground": "#ffffff",
            "warning.background": hex(t.n),
            "warning.foreground": "#ffffff",
            "info.background": hex(t.busy),
            "info.foreground": "#ffffff",
            "input.border": hex(t.line),
            "link.foreground": hex(t.accent),
            "link.hover.foreground": hex(t.accent),
            "link.active.foreground": hex(t.accent),
            "list.background": "#00000000",
            "list.hover.background": hex(t.hover),
            "list.active.background": hex(t.sel),
            "list.active.border": hex(t.accent),
            "list.even.background": "#00000000",
            "list.head.background": hex(t.panel),
            "table.background": hex(t.bg),
            "table.head.background": hex(t.panel),
            "table.head.foreground": hex(t.muted),
            "table.hover.background": hex(t.hover),
            "table.active.background": hex(t.sel),
            "table.active.border": hex(t.accent),
            "table.even.background": hex(t.bg),
            "table.row.border": hex(t.line),
            "popover.background": hex(t.bg),
            "popover.foreground": hex(t.text),
            "sidebar.background": "#00000000",
            "sidebar.foreground": hex(t.text),
            "sidebar.border": hex(t.line),
            "sidebar.accent.background": hex(t.hover),
            "sidebar.accent.foreground": hex(t.text),
            "sidebar.primary.background": hex(t.accent),
            "sidebar.primary.foreground": hex(t.accent_text),
            "tab.background": "#00000000",
            "tab.foreground": hex(t.muted),
            "tab.active.background": hex(t.bg),
            "tab.active.foreground": hex(t.text),
            "tab_bar.background": "#00000000",
            "tab_bar.segmented.background": hexa(t.cap_bg),
            "title_bar.background": "#00000000",
            "title_bar.border": "#00000000",
            "status_bar.background": "#00000000",
            "status_bar.border": "#00000000",
            "scrollbar.background": "#00000000",
            "scrollbar.thumb.background": hex(t.faint),
            "scrollbar.thumb.hover.background": hex(t.muted),
            "skeleton.background": hex(t.hover),
            "switch.background": hex(t.line),
            "slider.bar.background": hex(t.accent),
            "slider.thumb.background": hex(t.bg),
            "progress_bar.background": hex(t.accent),
            "drag_border": hex(t.accent),
            "drop_target.background": format!("#{:06x}40", t.accent),
            "overlay": "#0000000d",
            "window.border": hexa(t.glass_edge),
            "group_box.background": format!("#{:06x}{glass:02x}", t.bg),
            "group_box.foreground": hex(t.text),
        },
        "highlight": {
            "editor.background": hex(t.chunk),
            "editor.foreground": hex(t.text),
            "editor.active_line.background": "#00000000",
            "editor.line_number": hex(t.faint),
            "editor.active_line_number": hex(t.muted),
            "editor.invisible": hex(t.faint),
            "syntax": {
                "keyword": style(t.k),
                "string": style(t.s),
                "string.escape": style(t.s),
                "string.special": style(t.s),
                "string.special.symbol": style(t.s),
                "number": style(t.n),
                "boolean": style(t.n),
                "constant": style(t.n),
                "comment": { "color": hex(t.c), "font_style": "italic" },
                "comment.doc": { "color": hex(t.c), "font_style": "italic" },
                "function": style(t.f),
                "constructor": style(t.f),
                "type": style(t.f),
                "property": style(t.arg),
                "variable.parameter": style(t.arg),
                "operator": style(t.o),
                "punctuation": style(t.o),
                "punctuation.bracket": style(t.o),
                "punctuation.delimiter": style(t.o),
                "punctuation.special": style(t.fm),
                "punctuation.list_marker": style(t.fm),
                "title": { "color": hex(t.h), "font_weight": 700 },
                "emphasis": { "font_style": "italic" },
                "emphasis.strong": { "font_weight": 700 },
                "link_text": style(t.f),
                "link_uri": style(t.f),
                "text.literal": style(t.s),
                "embedded": style(t.text),
                "tag": style(t.k),
                "attribute": style(t.arg),
            },
        },
    })
}

const LIGHT_NAME: &str = "Sandhill Light";
const DARK_NAME: &str = "Sandhill Dark";

/// Loads the fonts and the light and dark themes, and follows the system's
/// appearance. Call after `gpui_kit::init`.
pub fn install(cx: &mut App) {
    add_fonts(cx);
    let set = json!({
        "name": "Sandhill",
        "author": "Sandhill",
        "themes": [
            theme_config(LIGHT_NAME, "light", &LIGHT),
            theme_config(DARK_NAME, "dark", &DARK),
        ],
    });
    let registry = ThemeRegistry::global_mut(cx);
    if let Err(err) = registry.load_themes_from_str(&set.to_string()) {
        tracing::warn!("the Sandhill theme did not load: {err:#}");
        return;
    }
    let themes = registry.themes().clone();
    let theme = Theme::global_mut(cx);
    if let Some(light) = themes.get(LIGHT_NAME) {
        theme.light_theme = light.clone();
    }
    if let Some(dark) = themes.get(DARK_NAME) {
        theme.dark_theme = dark.clone();
    }
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
    cx.set_global(if dark {
        Palette::dark()
    } else {
        Palette::light()
    });
    // gpui-cn (the terminal's) keeps a theme of its own; it goes first, so
    // gpui-kit's, set last, is the one the shared base layer draws with.
    if cx.has_global::<gpui_cn::Theme>() {
        gpui_cn::Theme::change(
            if dark {
                gpui_cn::ThemeMode::Dark
            } else {
                gpui_cn::ThemeMode::Light
            },
            cx,
        );
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
struct Icons;

pub struct Assets;

const PREFIX: &str = "sandhill/";

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(rest) = path.strip_prefix(PREFIX) else {
            return gpui_kit::assets::Assets.load(path);
        };
        Ok(Icons::get(rest).map(|file| {
            // The folder icons cut their emblem out with a CSS variable,
            // which the SVG renderer doesn't read.
            match std::str::from_utf8(&file.data) {
                Ok(svg) if svg.contains("var(--tv-cut") => Cow::Owned(
                    svg.replace("var(--tv-cut, #fff)", "#fff")
                        .replace("var(--tv-cut)", "#fff")
                        .into_bytes(),
                ),
                _ => file.data,
            }
        }))
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

/// A coloured icon (file types, languages, folders, agents): `path` such
/// as `lang/r` or `bio/fastq`.
pub fn picture(path: &str) -> Img {
    img(SharedString::from(format!("{PREFIX}{path}.svg")))
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

/// The icon for a folder (`folders/…`), by its name.
pub fn folder_icon(name: &str) -> String {
    let r = resolve();
    let icon = r
        .folders
        .get(&name.to_lowercase())
        .unwrap_or(&r.folder_fallback);
    format!("folders/{icon}")
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

/// The warm wall the panes sit on: a gradient, with a glow at the top left
/// and one at the bottom right (radial gradients in the Tauri app, soft
/// shadows here, which GPUI blurs).
pub fn wall(p: &Palette) -> Div {
    let glow = |color: Hsla| {
        div()
            .absolute()
            .w(px(700.))
            .h(px(420.))
            .rounded_full()
            .shadow(vec![BoxShadow {
                color,
                offset: point(px(0.), px(0.)),
                blur_radius: px(320.),
                spread_radius: px(40.),
                inset: false,
            }])
    };
    div()
        .relative()
        .bg(linear_gradient(
            160.,
            linear_color_stop(p.wall_from, 0.),
            linear_color_stop(p.wall_to, 1.),
        ))
        .child(glow(p.glow_top).left(px(-420.)).top(px(-360.)))
        .child(glow(p.glow_bottom).right(px(-400.)).bottom(px(-380.)))
}

/// A glass pane: translucent, with a hairline edge and a soft shadow.
pub fn pane(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .bg(p.glass)
        .border_1()
        .border_color(p.glass_edge)
        .rounded(px(RADIUS))
        .shadow(vec![
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

/// A capsule, as in the title bar and over a pane's tabs.
pub fn capsule(p: &Palette, height: f32) -> Div {
    div()
        .flex()
        .items_center()
        .flex_none()
        .h(px(height))
        .px(px(2.))
        .rounded_full()
        .bg(p.cap_bg)
        .border(px(0.5))
        .border_color(p.cap_edge)
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.07),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }])
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

/// The picked tab in a capsule: raised.
pub fn raised(d: Stateful<Div>, p: &Palette) -> Stateful<Div> {
    d.bg(p.cap_on)
        .text_color(p.text)
        .border(px(0.5))
        .border_color(p.cap_edge)
        .shadow(vec![BoxShadow {
            color: hsla(0., 0., 0., 0.1),
            offset: point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
            inset: false,
        }])
}

/// The rounded strip a pane's tabs sit in.
pub fn tab_capsule(p: &Palette) -> Div {
    capsule(p, 22.).rounded(px(TAB_RADIUS + 1.))
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
    use super::{Icons, file_icon, folder_icon, language_icon};

    #[test]
    fn picks_icons() {
        assert_eq!(file_icon("analysis/de.qmd"), "bio/quarto");
        assert_eq!(file_icon("reads/S1_R1.fastq.gz"), "bio/fastq");
        assert_eq!(file_icon("README.md"), "bio/readme");
        assert_eq!(file_icon("Snakefile"), "bio/snakemake");
        assert_eq!(file_icon("notes.unknown"), "bio/file");
        assert_eq!(folder_icon("Data"), "folders/data");
        assert_eq!(folder_icon("misc"), "folders/other");
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
    }
}
