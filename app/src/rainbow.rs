//! Rainbow brackets for R and Python editors: `()`, `[]` and `{}` take a
//! colour by nesting depth, ignoring brackets inside strings and comments.
//! Colours are decorations over the syntax colours; the text is never changed.
//! The language is read from the editor at each pass, so the Console input
//! follows its kernel's language.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{EditorState, Rope, TextDecoration, TextDecorationCollection};
use gpui_kit::*;

use crate::look::Palette;

/// Larger texts get no bracket colours, as `bio_text` does.
const LIMIT: usize = 262_144;
/// Texts up to this size are coloured at once; larger ones off the UI thread.
const SYNC_BYTES: usize = 32_768;
const DEBOUNCE: Duration = Duration::from_millis(60);
const MAX_MARKS: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    R,
    Python,
}

/// One bracket: its byte offset, and its nesting depth, or `None` for a
/// closer that closes nothing open.
#[derive(Debug, PartialEq, Eq)]
struct Bracket {
    at: usize,
    depth: Option<usize>,
}

fn lang(name: &str) -> Option<Lang> {
    match name {
        "r" => Some(Lang::R),
        "python" => Some(Lang::Python),
        _ => None,
    }
}

/// Past a string or backtick name opened by `quote` at `start`. Backslash
/// escapes the next byte. Python's single-quoted strings end at a line break.
fn skip_quoted(b: &[u8], start: usize, quote: u8, ends_at_line: bool) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'\n' if ends_at_line => return i,
            c if c == quote => return i + 1,
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// Past a Python triple-quoted string opened at `start`.
fn skip_triple(b: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 3;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i..].starts_with(&[quote; 3]) {
            return i + 3;
        }
        i += 1;
    }
    b.len()
}

fn r_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c >= 0x80
}

/// Past an R raw string (`r"(…)"`, `R'---[…]---'`) whose quote is at
/// `quote_at`, or `None` when the text there is not one.
fn skip_r_raw(b: &[u8], quote_at: usize) -> Option<usize> {
    let prefix = quote_at.checked_sub(1)?;
    if !matches!(b[prefix], b'r' | b'R') || prefix.checked_sub(1).is_some_and(|i| r_name_byte(b[i]))
    {
        return None;
    }
    let quote = b[quote_at];
    let dashes = b[quote_at + 1..].iter().take_while(|&&c| c == b'-').count();
    let open_at = quote_at + 1 + dashes;
    let close = match b.get(open_at)? {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut end = Vec::with_capacity(dashes + 2);
    end.push(close);
    end.extend(std::iter::repeat_n(b'-', dashes));
    end.push(quote);
    let body = open_at + 1;
    Some(
        b[body..]
            .windows(end.len())
            .position(|w| w == end.as_slice())
            .map_or(b.len(), |at| body + at + end.len()),
    )
}

/// Every bracket outside strings and comments, with its depth. A closer
/// takes the depth of the opener it closes; one that does not match the
/// innermost open bracket closes nothing and leaves it open.
fn brackets(text: &str, lang: Lang) -> Vec<Bracket> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut open: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'#' => {
                i = b[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(b.len(), |n| i + n);
                continue;
            }
            b'"' | b'\'' => {
                i = match lang {
                    Lang::R => skip_r_raw(b, i).unwrap_or_else(|| skip_quoted(b, i, c, false)),
                    Lang::Python if b[i..].starts_with(&[c; 3]) => skip_triple(b, i, c),
                    Lang::Python => skip_quoted(b, i, c, true),
                };
                continue;
            }
            b'`' if lang == Lang::R => {
                i = skip_quoted(b, i, c, false);
                continue;
            }
            // R's `%in%`-style operators stay on one line.
            b'%' if lang == Lang::R => {
                if let Some(n) = b[i + 1..].iter().position(|&c| c == b'%' || c == b'\n')
                    && b[i + 1 + n] == b'%'
                {
                    i += n + 2;
                    continue;
                }
            }
            b'(' | b'[' | b'{' => {
                out.push(Bracket {
                    at: i,
                    depth: Some(open.len()),
                });
                open.push(c);
            }
            b')' | b']' | b'}' => {
                let opener = match c {
                    b')' => b'(',
                    b']' => b'[',
                    _ => b'{',
                };
                let depth = if open.last() == Some(&opener) {
                    open.pop();
                    Some(open.len())
                } else {
                    None
                };
                out.push(Bracket { at: i, depth });
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn decorations(marks: &[Bracket], p: &Palette) -> Vec<TextDecoration> {
    // Kit's chart series is one blue in five shades; the ANSI hues are
    // distinct and readable on both the light and dark editor.
    let cycle = [p.ansi[3], p.ansi[5], p.ansi[4]];
    marks
        .iter()
        .take(MAX_MARKS)
        .map(|mark| {
            let color = mark.depth.map_or(p.err, |depth| cycle[depth % cycle.len()]);
            TextDecoration::new(
                mark.at..mark.at + 1,
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

/// One editor's bracket colours.
struct Rainbow {
    collection: TextDecorationCollection,
    /// The text and language of the latest pass.
    seen: RefCell<Option<(Rope, SharedString)>>,
    /// Counts passes, so a newer one supersedes one still waiting.
    generation: Cell<u64>,
}

/// Colour the brackets in `editor` now, or soon for a large text, unless
/// neither its text nor its language changed since the last pass (`force`
/// recolours anyway, for a new theme).
fn refresh(editor: &Entity<EditorState>, rainbow: &Rc<Rainbow>, force: bool, cx: &mut App) {
    let state = editor.read(cx);
    let text = state.text();
    let language = state.language_name();
    if !force
        && rainbow
            .seen
            .borrow()
            .as_ref()
            .is_some_and(|(seen, name)| *name == language && seen == text)
    {
        return;
    }
    let text = text.clone();
    *rainbow.seen.borrow_mut() = Some((text.clone(), language.clone()));
    let pass = rainbow.generation.get() + 1;
    rainbow.generation.set(pass);
    let Some(language) = lang(&language).filter(|_| text.len() <= LIMIT) else {
        rainbow.collection.clear(cx);
        return;
    };
    if text.len() <= SYNC_BYTES {
        let marks = brackets(&text.to_string(), language);
        rainbow
            .collection
            .set(decorations(&marks, Palette::get(cx)), cx);
        return;
    }
    let rainbow = rainbow.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(DEBOUNCE).await;
        if rainbow.generation.get() != pass {
            return;
        }
        let marks = cx
            .background_spawn(async move { brackets(&text.to_string(), language) })
            .await;
        if rainbow.generation.get() == pass {
            cx.update(|cx| {
                rainbow
                    .collection
                    .set(decorations(&marks, Palette::get(cx)), cx)
            });
        }
    })
    .detach();
}

/// Keep `editor`'s brackets coloured through edits, language and theme
/// changes, for as long as the editor lives. Edits are seen through the
/// editor's notifications: `set_value` (Console history, clearing) sends no
/// `InputEvent::Change`.
pub(crate) fn attach(editor: &Entity<EditorState>, cx: &mut App) {
    let rainbow = Rc::new(Rainbow {
        collection: editor.update(cx, |state, cx| {
            state.create_decorations_collection(Vec::new(), cx)
        }),
        seen: RefCell::new(None),
        generation: Cell::new(0),
    });
    refresh(editor, &rainbow, false, cx);
    // Both end with the editor. The theme observer runs while the editor is
    // being updated; recolour once that is over.
    let theme = rainbow.clone();
    editor.update(cx, |_, cx| {
        cx.observe_global::<Palette>(move |_, cx| {
            let editor = cx.entity();
            let rainbow = theme.clone();
            cx.defer(move |cx| refresh(&editor, &rainbow, true, cx));
        })
        .detach();
    });
    cx.observe(editor, move |editor, cx| {
        refresh(&editor, &rainbow, false, cx)
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{Bracket, Lang, brackets};

    /// The depths of the brackets, in order, as text: digits for depths,
    /// `x` for a closer that closes nothing.
    fn depths(text: &str, lang: Lang) -> String {
        brackets(text, lang)
            .iter()
            .map(|Bracket { depth, .. }| depth.map_or('x', |d| char::from(b'0' + d as u8)))
            .collect()
    }

    #[test]
    fn pairs_share_their_depth() {
        assert_eq!(depths("f(x[1], {g(2)})", Lang::R), "01112210");
        let marks = brackets("a(b)", Lang::Python);
        assert_eq!(marks.iter().map(|m| m.at).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn unmatched_closers_close_nothing() {
        assert_eq!(depths(")(]", Lang::R), "x0x");
        // A wrong closer leaves the open bracket for its real closer.
        assert_eq!(depths("f(x])", Lang::Python), "0x0");
        assert_eq!(depths("((", Lang::R), "01");
    }

    #[test]
    fn strings_and_comments_hide_brackets() {
        assert_eq!(depths("f(\"(\", '[') # ) {\ng()", Lang::R), "0000");
        assert_eq!(depths("f(\"a\\\"(\")", Lang::R), "00");
        assert_eq!(depths("`a(b`(1)", Lang::R), "00");
        assert_eq!(depths("x %in% y; a %)% b; 5 %% (2)", Lang::R), "00");
        assert_eq!(depths("s = '(' # (\nf(\"[\")", Lang::Python), "00");
    }

    #[test]
    fn r_raw_strings_end_at_their_own_delimiter() {
        assert_eq!(depths(r#"f(r"(a")b)", 1)"#, Lang::R), "00");
        assert_eq!(depths(r#"f(R'--[x]-'y]--', 1)"#, Lang::R), "00");
        assert_eq!(depths(r#"r"{(}"(1)"#, Lang::R), "00");
        // `r` ending a longer name is not a raw-string prefix.
        assert_eq!(depths(r#"var"(" )"#, Lang::R), "x");
    }

    #[test]
    fn python_triple_quotes_span_lines() {
        assert_eq!(depths("f(\"\"\"\n(\n\"\"\")[0]", Lang::Python), "0000");
        assert_eq!(depths("'''a ' ( '''(1)", Lang::Python), "00");
        // An unclosed single-quoted string ends at its line.
        assert_eq!(depths("s = '(\nf(1)", Lang::Python), "00");
    }
}
