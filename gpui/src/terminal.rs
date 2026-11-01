//! The Terminal pane: a shell that sandhilld keeps running, drawn with
//! `alacritty_terminal` (the library Zed's terminal is built on).
//!
//! The daemon owns the program and its output; this view parses the bytes
//! into a screen, draws it, and sends keys back.

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};
use base64::Engine;
use gpui_kit::component::ActiveTheme;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::look::Palette;

/// A screen line: its text and how each stretch of it looks.
type Row = (String, Vec<(std::ops::Range<usize>, HighlightStyle)>);

/// The Tauri app's 13px, 2px larger; lines as tall as the font's own, so
/// box-drawing lines join up.
pub const FONT_SIZE: f32 = crate::look::CODE;
pub const LINE_HEIGHT: f32 = 18.;

/// Replies the terminal owes the program (to "where is the cursor?" and
/// the like), sent back as input.
#[derive(Clone, Default)]
struct Replies(Arc<Mutex<Vec<String>>>);

impl EventListener for Replies {
    fn send_event(&self, event: TermEvent) {
        if let TermEvent::PtyWrite(text) = event {
            self.0.lock().unwrap().push(text);
        }
    }
}

struct Size {
    columns: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

pub struct TerminalView {
    pub id: String,
    term: Term<Replies>,
    parser: Processor,
    replies: Replies,
    /// Keys go out in order through one task.
    input: UnboundedSender<String>,
    resize: Box<dyn Fn(u16, u16)>,
    focus: FocusHandle,
    columns: usize,
    lines: usize,
    exited: Option<String>,
}

impl TerminalView {
    pub fn new(
        id: String,
        input: UnboundedSender<String>,
        resize: Box<dyn Fn(u16, u16)>,
        cx: &mut Context<Self>,
    ) -> Self {
        let replies = Replies::default();
        let size = Size {
            columns: 80,
            lines: 24,
        };
        TerminalView {
            id,
            term: Term::new(Config::default(), &size, replies.clone()),
            parser: Processor::new(),
            replies,
            input,
            resize,
            focus: cx.focus_handle(),
            columns: 80,
            lines: 24,
            exited: None,
        }
    }

    /// Output from the program, base64 as the daemon sends it.
    pub fn feed(&mut self, data: &str, cx: &mut Context<Self>) {
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
            return;
        };
        self.parser.advance(&mut self.term, &bytes);
        for reply in std::mem::take(&mut *self.replies.0.lock().unwrap()) {
            let _ = self.input.send(reply);
        }
        cx.notify();
    }

    pub fn exited(&mut self, how: String, cx: &mut Context<Self>) {
        self.exited = Some(how);
        cx.notify();
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn set_size(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let cell_width = cell_width(window, cx);
        let columns = ((bounds.size.width.as_f32() - 24.) / cell_width)
            .floor()
            .max(10.) as usize;
        let lines = ((bounds.size.height.as_f32() - 8.) / LINE_HEIGHT)
            .floor()
            .max(3.) as usize;
        if (columns, lines) == (self.columns, self.lines) {
            return;
        }
        self.columns = columns;
        self.lines = lines;
        self.term.resize(Size { columns, lines });
        (self.resize)(columns as u16, lines as u16);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            if keystroke.key == "v"
                && let Some(text) = cx.read_from_clipboard().and_then(|c| c.text())
            {
                let text = if self.term.mode().contains(TermMode::BRACKETED_PASTE) {
                    format!("\x1b[200~{text}\x1b[201~")
                } else {
                    text
                };
                self.send(text);
            }
            // Other ⌘ keys belong to the app.
            return;
        }
        if let Some(bytes) = encode_key(keystroke, self.term.mode()) {
            self.term.scroll_display(Scroll::Bottom);
            self.send(bytes);
            cx.stop_propagation();
        }
    }

    fn send(&self, text: String) {
        let _ = self.input.send(text);
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let lines = event.delta.pixel_delta(px(LINE_HEIGHT)).y.as_f32() / LINE_HEIGHT;
        let lines = lines.round() as i32;
        if lines != 0 {
            self.term.scroll_display(Scroll::Delta(lines));
            cx.notify();
        }
    }
}

/// What a key sends to the program, as xterm encodes it.
fn encode_key(keystroke: &Keystroke, mode: &TermMode) -> Option<String> {
    let m = &keystroke.modifiers;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let arrow = |c: char| {
        Some(if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        })
    };
    let named = match keystroke.key.as_str() {
        "enter" => Some("\r".to_string()),
        "backspace" => Some(if m.alt { "\x1b\x7f" } else { "\x7f" }.to_string()),
        "tab" if m.shift => Some("\x1b[Z".to_string()),
        "tab" => Some("\t".to_string()),
        "escape" => Some("\x1b".to_string()),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => Some("\x1b[H".to_string()),
        "end" => Some("\x1b[F".to_string()),
        "pageup" => Some("\x1b[5~".to_string()),
        "pagedown" => Some("\x1b[6~".to_string()),
        "delete" => Some("\x1b[3~".to_string()),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if m.control {
        let key = keystroke.key.as_str();
        let mut chars = key.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                return Some(((c as u8 - b'a' + 1) as char).to_string());
            }
            return match c {
                '[' => Some("\x1b".into()),
                '\\' => Some("\x1c".into()),
                ']' => Some("\x1d".into()),
                ' ' | '@' | '2' => Some("\0".into()),
                _ => None,
            };
        }
        return None;
    }
    let text = match &keystroke.key_char {
        Some(text) => text.clone(),
        None if keystroke.key == "space" => " ".into(),
        None if keystroke.key.chars().count() == 1 => keystroke.key.clone(),
        None => return None,
    };
    if m.alt {
        Some(format!("\x1b{text}"))
    } else {
        Some(text)
    }
}

fn cell_width(window: &mut Window, cx: &App) -> f32 {
    let font = font(cx.theme().mono_font_family.clone());
    let text_system = window.text_system();
    let id = text_system.resolve_font(&font);
    text_system
        .advance(id, px(FONT_SIZE), 'm')
        .map(|size| size.width.as_f32())
        .unwrap_or(FONT_SIZE * 0.6)
}

/// The palette's terminal colours, then xterm's 256, with the palette's
/// foreground and background.
fn rgb_of(color: Color, term: &Term<Replies>, p: &Palette) -> Hsla {
    let (fg, bg) = (p.text, p.term);
    let named = |n: NamedColor| -> Hsla {
        match n {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => fg,
            NamedColor::DimForeground => fg.opacity(0.7),
            NamedColor::Background => bg,
            other => {
                let index = other as usize;
                let index = if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize)
                    .contains(&index)
                {
                    index - NamedColor::DimBlack as usize
                } else {
                    index
                };
                p.ansi
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| indexed(index as u8))
            }
        }
    };
    match color {
        Color::Named(n) => match term.colors()[n] {
            Some(rgb) => to_hsla(rgb),
            None => named(n),
        },
        Color::Spec(rgb) => to_hsla(rgb),
        Color::Indexed(i) => match term.colors()[i as usize] {
            Some(rgb) => to_hsla(rgb),
            None => p
                .ansi
                .get(i as usize)
                .copied()
                .unwrap_or_else(|| indexed(i)),
        },
    }
}

fn indexed(i: u8) -> Hsla {
    const BASE: [u32; 16] = [
        0x000000, 0xc23621, 0x25bc24, 0xadad27, 0x492ee1, 0xd338d3, 0x33bbc8, 0xcbcccd, 0x818383,
        0xfc391f, 0x31e722, 0xeaec23, 0x5833ff, 0xf935f8, 0x14f0f0, 0xe9ebeb,
    ];
    let hex = match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v as u32 * 40 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        _ => {
            let v = 8 + (i as u32 - 232) * 10;
            (v << 16) | (v << 8) | v
        }
    };
    rgb(hex).into()
}

fn to_hsla(c: Rgb) -> Hsla {
    rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into()
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let bg = p.term;
        let mono = cx.theme().mono_font_family.clone();
        let focused = self.focus.is_focused(window);
        let content = self.term.renderable_content();
        let cursor = content.cursor.point;
        let offset = content.display_offset as i32;
        let mut rows: Vec<Row> = vec![(String::new(), Vec::new()); self.lines];
        for indexed in content.display_iter {
            let line = (indexed.point.line.0 + offset) as usize;
            let Some((text, styles)) = rows.get_mut(line) else {
                continue;
            };
            let cell = &indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let mut cell_fg = rgb_of(cell.fg, &self.term, &p);
            let mut cell_bg = rgb_of(cell.bg, &self.term, &p);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut cell_fg, &mut cell_bg);
            }
            let at_cursor = indexed.point == cursor && offset == 0;
            if at_cursor && focused && self.exited.is_none() {
                std::mem::swap(&mut cell_fg, &mut cell_bg);
                if cell_bg == bg {
                    cell_bg = p.accent;
                    cell_fg = p.bg;
                }
            }
            let start = text.len();
            let c = if cell.c == '\0' { ' ' } else { cell.c };
            text.push(c);
            let style = HighlightStyle {
                color: Some(cell_fg),
                background_color: (cell_bg != bg).then_some(cell_bg),
                font_weight: cell.flags.contains(Flags::BOLD).then_some(FontWeight::BOLD),
                ..Default::default()
            };
            match styles.last_mut() {
                Some((range, last)) if *last == style && range.end == start => {
                    range.end = text.len()
                }
                _ => styles.push((start..text.len(), style)),
            }
        }
        let entity = cx.entity();
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::key_down))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| this.scroll(ev, cx)))
            .on_click(cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)))
            .relative()
            .size_full()
            .p_1()
            .overflow_hidden()
            .bg(bg)
            .font_family(mono)
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        entity.update(cx, |this, cx| this.set_size(bounds, window, cx))
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .children(rows.into_iter().map(|(text, styles)| {
                div()
                    .h(px(LINE_HEIGHT))
                    .whitespace_nowrap()
                    .child(StyledText::new(text).with_highlights(styles))
            }))
            .when_some(self.exited.clone(), |d, how| {
                d.child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("[{how}]")),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings in GPUI's own `test` macro.
    use super::{TermMode, encode_key, indexed};
    use gpui_kit::{Keystroke, Modifiers, rgb};

    fn key(key: &str, control: bool, key_char: Option<&str>) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control,
                ..Default::default()
            },
            key: key.into(),
            key_char: key_char.map(Into::into),
        }
    }

    #[test]
    fn encodes_keys_as_xterm_does() {
        let normal = TermMode::empty();
        assert_eq!(
            encode_key(&key("a", false, Some("a")), &normal).as_deref(),
            Some("a")
        );
        assert_eq!(
            encode_key(&key("c", true, None), &normal).as_deref(),
            Some("\x03")
        );
        assert_eq!(
            encode_key(&key("enter", false, None), &normal).as_deref(),
            Some("\r")
        );
        assert_eq!(
            encode_key(&key("up", false, None), &normal).as_deref(),
            Some("\x1b[A")
        );
        assert_eq!(
            encode_key(&key("up", false, None), &TermMode::APP_CURSOR).as_deref(),
            Some("\x1bOA")
        );
        assert_eq!(encode_key(&key("shift", false, None), &normal), None);
    }

    #[test]
    fn has_the_256_colour_palette() {
        assert_eq!(indexed(16), rgb(0x000000).into());
        assert_eq!(indexed(231), rgb(0xffffff).into());
        assert_eq!(indexed(232), rgb(0x080808).into());
    }
}
