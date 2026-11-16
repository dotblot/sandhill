//! The call-signature hint: which call the cursor is in, the hint's text from
//! a language server or the kernel, and the small popup above the cursor.
use super::providers;
use crate::look::{self, Palette};
use crate::workbench::Workbench;
use gpui_kit::component::input::EditorState;
use gpui_kit::*;
use lsp_types::{ParameterLabel, SignatureHelp};
use serde_json::Value;
use std::ops::Range;

/// The call around the cursor, from the text before the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallSite {
    /// Byte offset of the call's `(`.
    pub open: usize,
    /// The callee as written: `getKEGG`, `np.mean`, `stats::lm`.
    pub name: String,
    /// Byte offset just after the callee, where the kernel inspects it.
    pub name_end: usize,
    /// The argument the cursor is in, counted from zero.
    pub index: usize,
    /// The argument's name once it reads `name =`.
    pub named: Option<String>,
}

/// Bracketed words that are syntax, not calls.
const KEYWORDS: &[&str] = &[
    "if", "elif", "while", "for", "function", "in", "not", "and", "or", "return", "yield",
    "lambda", "assert", "del", "with", "except", "import", "from", "repeat", "else",
];

/// The innermost call whose parentheses hold the end of `before`. Strings
/// keep their call (`library("gg`); a comment or a bracket has none.
pub(crate) fn call_site(before: &str, r: bool) -> Option<CallSite> {
    // (bracket, its byte, commas so far, where the current argument starts)
    let mut stack: Vec<(char, usize, usize, usize)> = Vec::new();
    let mut quote: Option<(char, bool)> = None;
    let mut comment = false;
    let mut chars = before.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if comment {
            comment = ch != '\n';
            continue;
        }
        if let Some((close, triple)) = quote {
            if ch == '\\' {
                chars.next();
            } else if ch == close {
                if !triple {
                    quote = None;
                } else if before[at..].starts_with(&close.to_string().repeat(3)) {
                    chars.next();
                    chars.next();
                    quote = None;
                }
            } else if ch == '\n' && !r && !triple {
                quote = None;
            }
            continue;
        }
        match ch {
            '#' => comment = true,
            '"' | '\'' => {
                let triple = !r && before[at..].starts_with(&ch.to_string().repeat(3));
                if triple {
                    chars.next();
                    chars.next();
                }
                quote = Some((ch, triple));
            }
            '`' if r => quote = Some(('`', false)),
            '(' | '[' | '{' => stack.push((ch, at, 0, at + 1)),
            ')' | ']' | '}' => {
                stack.pop();
            }
            ',' => {
                if let Some(top) = stack.last_mut() {
                    top.2 += 1;
                    top.3 = at + 1;
                }
            }
            _ => {}
        }
    }
    if comment {
        return None;
    }
    let &('(', open, index, start) = stack.last()? else {
        return None;
    };
    let head = before[..open].trim_end_matches([' ', '\t']);
    let callee = |ch: char| {
        ch.is_alphanumeric() || matches!(ch, '_' | '.') || (r && matches!(ch, ':' | '$' | '@'))
    };
    let name_start = head
        .char_indices()
        .rev()
        .take_while(|(_, ch)| callee(*ch))
        .last()
        .map(|(at, _)| at)?;
    let name = &head[name_start..];
    // `def f(` and `class C(` define rather than call.
    let defines = !r
        && head[..name_start]
            .trim_end()
            .rsplit(char::is_whitespace)
            .next()
            .is_some_and(|word| matches!(word, "def" | "class"));
    if defines || name.starts_with(|ch: char| ch.is_ascii_digit()) || KEYWORDS.contains(&name) {
        return None;
    }
    Some(CallSite {
        open,
        name: name.to_string(),
        name_end: head.len(),
        index,
        named: argument_name(&before[start..]),
    })
}

/// `organism = "hsa"` → `organism`; `x == 1` and `f(` → none.
fn argument_name(argument: &str) -> Option<String> {
    let argument = argument.trim_start();
    let end = argument
        .find(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '.')))
        .unwrap_or(argument.len());
    let rest = argument[end..].trim_start();
    (end > 0 && rest.starts_with('=') && !rest.starts_with("=="))
        .then(|| argument[..end].to_string())
}

/// A signature with its parameters as byte ranges of `label`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hint {
    pub label: String,
    pub params: Vec<Range<usize>>,
    pub active: Option<usize>,
}

/// `textDocument/signatureHelp`'s answer, if it has a signature.
pub(crate) fn from_lsp(value: Value, call: &CallSite) -> Option<Hint> {
    let help = serde_json::from_value::<Option<SignatureHelp>>(value).ok()??;
    let chosen = help.active_signature.unwrap_or(0) as usize;
    let signature = help
        .signatures
        .get(chosen)
        .or(help.signatures.first())?
        .clone();
    let label = signature.label;
    let mut params = Vec::new();
    let mut from = label.find('(').map_or(0, |at| at + 1);
    for parameter in signature.parameters.unwrap_or_default() {
        let range = match parameter.label {
            ParameterLabel::Simple(text) => label[from..]
                .find(&text)
                .map(|at| from + at..from + at + text.len()),
            ParameterLabel::LabelOffsets([start, end]) => {
                let byte = |units: u32| {
                    let mut seen = 0;
                    label
                        .char_indices()
                        .find(|(_, ch)| {
                            let past = seen >= units as usize;
                            seen += ch.len_utf16();
                            past
                        })
                        .map_or(label.len(), |(at, _)| at)
                };
                Some(byte(start)..byte(end))
            }
        };
        if let Some(range) = range.filter(|range| range.start <= range.end) {
            from = range.end;
            params.push(range);
        }
    }
    let server = signature
        .active_parameter
        .or(help.active_parameter)
        .map(|active| active as usize);
    let active = active_parameter(&label, &params, call, server);
    Some(Hint {
        label,
        params,
        active,
    })
}

/// A kernel's `inspect_reply` text: IPython's `Signature:` line, or an R
/// function's `function (…)` header.
pub(crate) fn from_inspect(text: &str, call: &CallSite) -> Option<Hint> {
    let text = strip_ansi(text);
    let label = if text.trim_start().starts_with("function") {
        let open = text.find('(')?;
        let close = open + matching(&text[open..])?;
        format!("{}{}", call.name, &text[open..=close])
    } else {
        let mut lines = text.lines();
        let first = lines.find_map(|line| {
            ["Signature:", "Init signature:", "Call signature:"]
                .iter()
                .find_map(|key| line.strip_prefix(key))
        })?;
        // IPython breaks a long signature over lines, one parameter each.
        let mut label = first.trim().to_string();
        let closed = |label: &str| {
            label
                .find('(')
                .is_some_and(|at| matching(&label[at..]).is_some())
        };
        for line in lines {
            if closed(&label) {
                break;
            }
            if !label.is_empty() && !label.ends_with('(') {
                label.push(' ');
            }
            label.push_str(line.trim());
        }
        label.replace(",)", ")").replace(", )", ")")
    };
    let open = label.find('(')?;
    let close = open + matching(&label[open..])?;
    let mut params = Vec::new();
    let mut depth = 0;
    let mut quote = None;
    let mut start = open + 1;
    for (at, ch) in label[open + 1..close]
        .char_indices()
        .map(|(i, c)| (open + 1 + i, c))
    {
        match (quote, ch) {
            (Some(q), _) if ch == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                push_param(&label, start..at, &mut params);
                start = at + 1;
            }
            _ => {}
        }
    }
    push_param(&label, start..close, &mut params);
    let active = active_parameter(&label, &params, call, None);
    Some(Hint {
        label,
        params,
        active,
    })
}

fn push_param(label: &str, range: Range<usize>, params: &mut Vec<Range<usize>>) {
    let text = &label[range.clone()];
    let start = range.start + (text.len() - text.trim_start().len());
    let end = range.end - (text.len() - text.trim_end().len());
    // Python's bare `/` and `*` only mark how the others may be passed.
    if start < end && !matches!(&label[start..end], "/" | "*") {
        params.push(start..end);
    }
}

/// Bytes from an opening `(` to its `)`.
fn matching(text: &str) -> Option<usize> {
    let mut depth = 0;
    for (at, ch) in text.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// A named argument picks its parameter; otherwise the server's choice or
/// the position, with extra arguments falling to a trailing `...`/`*args`.
fn active_parameter(
    label: &str,
    params: &[Range<usize>],
    call: &CallSite,
    server: Option<usize>,
) -> Option<usize> {
    if let Some(name) = &call.named
        && let Some(found) = params.iter().position(|range| {
            let text = label[range.clone()].trim_start_matches('*');
            text.strip_prefix(name.as_str()).is_some_and(|rest| {
                !rest.starts_with(|ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '.'))
            })
        })
    {
        return Some(found);
    }
    let index = server.unwrap_or(call.index);
    if index < params.len() {
        return Some(index);
    }
    params
        .iter()
        .rposition(|range| {
            let text = &label[range.clone()];
            text.starts_with("...") || (text.starts_with('*') && !text.starts_with("**"))
        })
        .filter(|_| call.named.is_none())
}

#[derive(Default)]
pub(crate) struct State {
    shown: Option<Shown>,
    /// The last call asked about: editor, `(`, argument and its name.
    asked: Option<(EntityId, usize, usize, Option<String>)>,
    /// Escape closed the hint for this call; it stays closed there.
    dismissed: Option<(EntityId, usize)>,
    task: Option<Task<()>>,
}

struct Shown {
    editor: WeakEntity<EditorState>,
    id: EntityId,
    open: usize,
    hint: Hint,
}

impl Workbench {
    /// Follows an editor's text and cursor: a `(` or `,` asks for the
    /// call's signature, moving within the call updates it, and leaving the
    /// call (or closing it with `)`) hides it.
    pub(super) fn follow_signature(
        &mut self,
        editor: &Entity<EditorState>,
        console: bool,
        cx: &mut Context<Self>,
    ) {
        let id = editor.entity_id();
        let state = &self.language.signature;
        let mine = state.shown.as_ref().is_some_and(|shown| shown.id == id)
            || state.asked.as_ref().is_some_and(|asked| asked.0 == id);
        let input = editor.read(cx);
        let language = input.language_name();
        let r = language.eq_ignore_ascii_case("r");
        if !r && !language.eq_ignore_ascii_case("python") {
            if mine {
                self.hide_signature(cx);
            }
            return;
        }
        let selection = input.selected_range();
        let cursor = input.cursor();
        let text = input.text().clone();
        if !selection.is_empty() || cursor > text.len() || !text.is_char_boundary(cursor) {
            if mine {
                self.hide_signature(cx);
            }
            return;
        }
        let trigger = text
            .chars_at(cursor)
            .reversed()
            .find(|ch| !ch.is_whitespace())
            .is_some_and(|ch| matches!(ch, '(' | ','));
        if !mine && !trigger {
            return;
        }
        let before = text.slice(0..cursor).to_string();
        let Some(call) = call_site(&before, r) else {
            if mine {
                self.hide_signature(cx);
            }
            return;
        };
        let state = &mut self.language.signature;
        if state.dismissed == Some((id, call.open)) {
            return;
        }
        let same_call = state
            .shown
            .as_ref()
            .is_some_and(|shown| shown.id == id && shown.open == call.open);
        if mine && !same_call && state.shown.is_some() {
            state.shown = None;
            cx.notify();
        }
        if !same_call && !trigger {
            return;
        }
        let key = (id, call.open, call.index, call.named.clone());
        if state.asked.as_ref() == Some(&key) {
            return;
        }
        state.asked = Some(key.clone());
        let text = text.to_string();
        let work = providers::signature_help(
            cx.entity().downgrade(),
            editor.downgrade(),
            console,
            text,
            cursor,
            call.clone(),
            cx,
        );
        let editor = editor.downgrade();
        state.task = Some(cx.spawn(async move |this, cx| {
            let hint = work.await;
            let _ = this.update(cx, |this, cx| {
                let state = &mut this.language.signature;
                if state.asked.as_ref() != Some(&key) {
                    return;
                }
                match hint {
                    Some(hint) => {
                        state.shown = Some(Shown {
                            editor,
                            id,
                            open: call.open,
                            hint,
                        })
                    }
                    // Keep the last answer for this call rather than flicker.
                    None if state
                        .shown
                        .as_ref()
                        .is_some_and(|shown| shown.id == id && shown.open == call.open) => {}
                    None => state.shown = None,
                }
                cx.notify();
            });
        }));
    }

    /// The editor lost focus: its hint goes.
    pub(super) fn blur_signature(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let state = &self.language.signature;
        if state.shown.as_ref().is_some_and(|shown| shown.id == id)
            || state.asked.as_ref().is_some_and(|asked| asked.0 == id)
        {
            self.hide_signature(cx);
        }
    }

    /// Hides the hint without remembering the call (it may return).
    pub(super) fn hide_signature(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.language.signature;
        state.asked = None;
        state.task = None;
        if state.shown.take().is_some() {
            cx.notify();
        }
    }

    /// Escape: hides the hint and keeps it closed for this call.
    pub(crate) fn dismiss_signature(&mut self, cx: &mut Context<Self>) -> bool {
        let state = &mut self.language.signature;
        let Some(shown) = state.shown.take() else {
            return false;
        };
        state.dismissed = Some((shown.id, shown.open));
        state.asked = None;
        state.task = None;
        cx.notify();
        true
    }

    pub(crate) fn render_signature(&self, cx: &App) -> Option<AnyElement> {
        let shown = self.language.signature.shown.as_ref()?;
        let hint = &shown.hint;
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        Some(
            deferred(AboveCaret {
                editor: shown.editor.clone(),
                label: hint.label.clone().into(),
                active: hint
                    .active
                    .and_then(|active| hint.params.get(active))
                    .map(|range| (range.clone(), bold)),
                palette: *Palette::get(cx),
                content: None,
            })
            .with_priority(2)
            .into_any_element(),
        )
    }
}

/// The widest a hint grows before its signature wraps.
const MAX_HINT_WIDTH: Pixels = px(640.);
/// The height below the caret line at which Kit's completion menu may open
/// above the caret instead (its list's 240px, the popover's padding and
/// border, and its 4px gap; `caret_anchored.rs` flips when the menu doesn't
/// fit below and there is more room above).
const MENU_ROOM: Pixels = px(256.);
/// The hint never covers the window's title bar.
const TOP: f32 = look::TITLE_BAR + 4.;

/// Where a hint `height` tall goes for a caret line at `caret_top`: its
/// bottom just above the line, else just below it. With the completion
/// menu open it stays above (the menu opens below), and hides when the
/// menu may flip above or there is no room above.
fn hint_top(
    caret_top: Pixels,
    line_height: Pixels,
    height: Pixels,
    viewport_height: Pixels,
    menu_open: bool,
) -> Option<Pixels> {
    let below = caret_top + line_height + px(4.);
    let above = caret_top - px(4.);
    let fits_above = above - height >= px(TOP);
    if menu_open {
        let room_below = viewport_height - below;
        let menu_may_flip = room_below < MENU_ROOM && above > room_below;
        return (fits_above && !menu_may_flip).then(|| above - height);
    }
    Some(if fits_above { above - height } else { below })
}

/// Places the hint just above the editor's caret line, from the caret
/// geometry of the editor's current frame (it runs in deferred prepaint).
/// The popup is built here at its text's width (capped), so a wrapped
/// signature is measured at the width it paints at and grows upward.
struct AboveCaret {
    editor: WeakEntity<EditorState>,
    label: SharedString,
    active: Option<(Range<usize>, HighlightStyle)>,
    palette: Palette,
    content: Option<AnyElement>,
}

impl Element for AboveCaret {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let editor = editor.read(cx);
        let Some(geometry) = editor.prepaint_caret_geometry() else {
            return;
        };
        let menu_open = editor.completion_menu_state().open;
        let viewport = window.viewport_size();
        let mut style = window.text_style();
        style.font_family = look::MONO.into();
        let text_width = window
            .text_system()
            .shape_line(
                self.label.clone(),
                px(look::SMALL),
                &[style.to_run(self.label.len())],
                None,
            )
            .width;
        // Padding (8px a side), the popup's border, and a pixel of rounding.
        let width = (text_width + px(19.))
            .min(MAX_HINT_WIDTH)
            .min(viewport.width - px(16.))
            .max(px(0.));
        let mut content = crate::agents::composer_view::popup(&self.palette)
            .id("signature-hint")
            .w(width)
            .px(px(8.))
            .py(px(4.))
            .font_family(look::MONO)
            .text_size(px(look::SMALL))
            .child(StyledText::new(self.label.clone()).with_highlights(self.active.clone()))
            .into_any_element();
        let size = content.layout_as_root(
            Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(viewport.height),
            },
            window,
            cx,
        );
        let caret = geometry.cursor_bounds().origin;
        let Some(y) = hint_top(
            caret.y + geometry.scroll_offset().y,
            geometry.line_height(),
            size.height,
            viewport.height,
            menu_open,
        ) else {
            return;
        };
        let x = (caret.x - px(8.))
            .min(viewport.width - size.width - px(8.))
            .max(px(8.));
        content.prepaint_at(point(x, y), window, cx);
        self.content = Some(content);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = self.content.as_mut() {
            content.paint(window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CallSite, call_site, from_inspect, hint_top};
    use gpui_kit::px;

    #[test]
    fn hint_sits_above_the_caret_line_and_avoids_the_menu() {
        let at =
            |caret: f32, menu| hint_top(px(caret), px(18.), px(40.), px(800.), menu).map(f32::from);
        // Its bottom 4px above the caret line, however tall it is.
        assert_eq!(at(100., false), Some(56.));
        // No room above: below the caret line.
        assert_eq!(at(20., false), Some(42.));
        // Room above only by covering the title bar: below.
        assert_eq!(at(70., false), Some(92.));
        // The menu opens below; the hint stays above, or hides.
        assert_eq!(at(100., true), Some(56.));
        assert_eq!(at(20., true), None);
        // Near the bottom the menu may open above: no hint.
        assert_eq!(at(700., true), None);
        assert_eq!(at(700., false), Some(656.));
    }

    fn call(before: &str, r: bool) -> Option<(String, usize, Option<String>)> {
        call_site(before, r).map(|call| (call.name, call.index, call.named))
    }

    #[test]
    fn finds_the_innermost_call_and_argument() {
        assert_eq!(call("getKEGG(", true), Some(("getKEGG".into(), 0, None)));
        assert_eq!(call("getKEGG(org", true), Some(("getKEGG".into(), 0, None)));
        assert_eq!(
            call("f(x, g(1, 2), organism = \"h", true),
            Some(("f".into(), 2, Some("organism".into())))
        );
        assert_eq!(
            call("np.mean(a, axis=", false),
            Some(("np.mean".into(), 1, Some("axis".into())))
        );
        assert_eq!(
            call("stats::lm(y ~ x, ", true),
            Some(("stats::lm".into(), 1, None))
        );
        assert_eq!(call("f(a, \"x, (y\", ", true), Some(("f".into(), 2, None)));
        assert_eq!(
            call("print('''a,\n(''', ", false),
            Some(("print".into(), 1, None))
        );
        assert_eq!(call("f(x == ", true), Some(("f".into(), 0, None)));
        let site = call_site("x <- getKEGG (", true).unwrap();
        assert_eq!(site.open, 13);
        assert_eq!(site.name_end, 12);
    }

    #[test]
    fn closed_calls_brackets_and_syntax_are_not_calls() {
        assert_eq!(call("f(x)", true), None);
        assert_eq!(call("f(x) + g(1)", true), None);
        assert_eq!(call("x[1, ", true), None);
        assert_eq!(call("f([1, ", false), None);
        assert_eq!(call("(a + ", true), None);
        assert_eq!(call("if (x > ", true), None);
        assert_eq!(call("function(x, ", true), None);
        assert_eq!(call("def f(a, ", false), None);
        assert_eq!(
            call("x = undef(a, ", false),
            Some(("undef".into(), 1, None))
        );
        assert_eq!(call("f(x, # note (a, ", true), None);
        assert_eq!(call("f(x, # note\n", true), Some(("f".into(), 1, None)));
        assert_eq!(call("f(x)\ng(", true), Some(("g".into(), 0, None)));
    }

    #[test]
    fn kernel_signatures_mark_the_current_parameter() {
        let site = |index, named: Option<&str>| CallSite {
            open: 0,
            name: "f".into(),
            name_end: 0,
            index,
            named: named.map(str::to_string),
        };
        let text = "\u{1b}[31mSignature:\u{1b}[39m f(alpha, beta=\u{1b}[32m2\u{1b}[39m, *args, gamma=None)\n\u{1b}[31mDocstring:\u{1b}[39m x";
        let hint = from_inspect(text, &site(1, None)).unwrap();
        assert_eq!(hint.label, "f(alpha, beta=2, *args, gamma=None)");
        let param = |hint: &super::Hint| {
            hint.active
                .map(|i| hint.label[hint.params[i].clone()].to_string())
        };
        assert_eq!(param(&hint).as_deref(), Some("beta=2"));
        assert_eq!(
            param(&from_inspect(text, &site(5, None)).unwrap()).as_deref(),
            Some("*args")
        );
        assert_eq!(
            param(&from_inspect(text, &site(0, Some("gamma"))).unwrap()).as_deref(),
            Some("gamma=None")
        );
        // IPython puts a long signature one parameter per line.
        let long = "Signature:\nnp.mean(\n    a,\n    axis=None,\n    *,\n    where=<no value>,\n)\nDocstring: x";
        let hint = from_inspect(long, &site(1, None)).unwrap();
        assert_eq!(hint.label, "np.mean(a, axis=None, *, where=<no value>)");
        assert_eq!(hint.params.len(), 3);
        assert_eq!(param(&hint).as_deref(), Some("axis=None"));
        let r = from_inspect(
            "function (x, trim = 0, na.rm = FALSE, ...) \nNULL",
            &site(1, Some("na.rm")),
        )
        .unwrap();
        assert_eq!(r.label, "f(x, trim = 0, na.rm = FALSE, ...)");
        assert_eq!(param(&r).as_deref(), Some("na.rm = FALSE"));
    }
}
