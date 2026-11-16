use super::*;

/// IPython's experimental completion types (`function`, `class`…), by text.
fn kernel_kinds(metadata: &Value) -> HashMap<String, CompletionItemKind> {
    let Some(types) = metadata
        .get("_jupyter_types_experimental")
        .and_then(Value::as_array)
    else {
        return HashMap::new();
    };
    types
        .iter()
        .filter_map(|entry| {
            let kind = match entry.get("type")?.as_str()? {
                "function" => CompletionItemKind::FUNCTION,
                "class" => CompletionItemKind::CLASS,
                "module" => CompletionItemKind::MODULE,
                "keyword" => CompletionItemKind::KEYWORD,
                "instance" | "statement" | "param" => CompletionItemKind::VARIABLE,
                "property" => CompletionItemKind::PROPERTY,
                _ => return None,
            };
            Some((entry.get("text")?.as_str()?.to_string(), kind))
        })
        .collect()
}

/// Plain-text documentation (ty sends docstrings so) as markdown, so its
/// code shows as code and its prose keeps its sections and entries.
pub(super) fn plain_docs_as_markdown(item: &mut CompletionItem) {
    let plain = match &item.documentation {
        Some(Documentation::String(text)) => text.clone(),
        Some(Documentation::MarkupContent(content)) if content.kind == MarkupKind::PlainText => {
            content.value.clone()
        }
        _ => return,
    };
    item.documentation = Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: crate::editor::doc_text::docstring_markdown(&plain),
    }));
}

/// The kernel's matches as items. A Python item names its help topic: the
/// dotted name it completes (`sc.` + `AnnData`), asked once it is selected.
pub(super) fn kernel_items(
    text: &str,
    reply: CompletionReply,
    python: bool,
) -> Vec<CompletionItem> {
    let kinds = kernel_kinds(&reply.metadata);
    let scalar_byte = |scalar| {
        text.char_indices()
            .nth(scalar)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let start = scalar_byte(reply.cursor_start);
    let range = Range::new(
        component_position(text, start),
        component_position(text, scalar_byte(reply.cursor_end)),
    );
    let owner_start = text[..start]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_' || *ch == '.')
        .last()
        .map_or(start, |(at, _)| at);
    let owner = &text[owner_start..start];
    reply
        .matches
        .into_iter()
        .map(|label| CompletionItem {
            kind: kinds.get(&label).copied(),
            data: (python && !owner.starts_with(|ch: char| ch == '.' || ch.is_ascii_digit()))
                .then(|| json!({ HELP_TOPIC: format!("{owner}{label}") })),
            label: label.clone(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: label,
            })),
            ..Default::default()
        })
        .collect()
}

/// What an item's documentation is stored under: its label and the data
/// its source needs to find it again. Items without data can't be asked.
pub(super) fn docs_key(item: &CompletionItem) -> Option<String> {
    Some(format!("{}\u{0}{}", item.label, item.data.as_ref()?))
}

/// An item's documentation as markdown, if it has any.
pub(super) fn docs_markdown(item: &CompletionItem) -> Option<String> {
    let text = match item.documentation.as_ref()? {
        Documentation::String(text) => text.clone(),
        Documentation::MarkupContent(content) => content.value.clone(),
    };
    (!text.trim().is_empty()).then_some(text)
}

/// A Python help page as markdown: how it is called, then its docstring.
pub(super) fn help_markdown(page: &HelpPage) -> Option<String> {
    let mut parts = vec![];
    if let Some(signature) = page.signature.as_deref().filter(|sig| !sig.is_empty()) {
        parts.push(crate::editor::doc_text::python_code_markdown(
            &crate::panes::help::signature_lines(&page.topic, signature, 60),
        ));
    }
    if let Some(text) = page.text.as_deref().filter(|text| !text.trim().is_empty()) {
        parts.push(crate::editor::doc_text::docstring_markdown(text));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Moves a server item into cell coordinates. A primary edit that leaves the
/// cell but spans the cursor (Ark's argument items can) replaces the word
/// being typed instead; one that misses the cursor drops the item.
pub(super) fn map_completion(
    item: &mut CompletionItem,
    document: &Document,
    cell: usize,
    cursor: Position,
    token: Range,
) -> bool {
    let map = |range| document.component_range(cell, range);
    let spans = |range: Range| range.start <= cursor && cursor <= range.end;
    if let Some(edit) = &mut item.text_edit {
        match edit {
            CompletionTextEdit::Edit(edit) => {
                edit.range = match map(edit.range) {
                    Some(range) => range,
                    None if spans(edit.range) => token,
                    None => return false,
                };
            }
            CompletionTextEdit::InsertAndReplace(edit) => {
                match (map(edit.insert), map(edit.replace)) {
                    (Some(insert), Some(replace)) => {
                        edit.insert = insert;
                        edit.replace = replace;
                    }
                    _ if spans(edit.insert) || spans(edit.replace) => {
                        edit.insert = token;
                        edit.replace = token;
                    }
                    _ => return false,
                }
            }
        }
    }
    if let Some(edits) = &mut item.additional_text_edits {
        for edit in edits {
            let Some(range) = map(edit.range) else {
                return false;
            };
            edit.range = range;
        }
    }
    true
}

/// Kit inserts the primary edit only. Fold same-cell import edits into that
/// replacement so accepting a suggestion never silently loses its imports.
/// A snippet primary keeps its syntax; the plain text around it is escaped.
pub(super) fn coalesce_edits(item: &mut CompletionItem, text: &str) -> bool {
    let Some(edits) = item.additional_text_edits.take() else {
        return true;
    };
    if edits.is_empty() {
        return true;
    }
    let primary = match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => edit.clone(),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => TextEdit {
            range: edit.replace,
            new_text: edit.new_text.clone(),
        },
        None => return false,
    };
    let snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET);
    let plain = |text: &str| {
        if snippet {
            text.replace('\\', "\\\\")
                .replace('$', "\\$")
                .replace('}', "\\}")
        } else {
            text.to_string()
        }
    };
    let mut edits: Vec<(TextEdit, bool)> = edits.into_iter().map(|edit| (edit, false)).collect();
    edits.push((primary, true));
    edits.sort_by_key(|(edit, _)| edit.range.start);
    if edits
        .windows(2)
        .any(|pair| pair[0].0.range.end > pair[1].0.range.start)
    {
        return false;
    }
    let offset = |position| component_byte(text, position);
    let range = Range::new(
        edits.first().unwrap().0.range.start,
        edits.last().unwrap().0.range.end,
    );
    let mut cursor = offset(range.start);
    let mut replacement = String::new();
    for (edit, primary) in edits {
        let start = offset(edit.range.start);
        let end = offset(edit.range.end);
        if start < cursor || end < start {
            return false;
        }
        replacement.push_str(&plain(&text[cursor..start]));
        if primary {
            replacement.push_str(&edit.new_text);
        } else {
            replacement.push_str(&plain(&edit.new_text));
        }
        cursor = end;
    }
    item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
        range,
        new_text: replacement,
    }));
    true
}

/// The byte at a component position (line, Unicode column) of `text`.
fn component_byte(text: &str, position: Position) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(position.line as usize)
        .map(str::len)
        .sum::<usize>();
    let row = text[start..].split('\n').next().unwrap_or("");
    start
        + row
            .char_indices()
            .nth(position.character as usize)
            .map_or(row.len(), |(byte, _)| byte)
}

/// Where the name being typed at `offset` starts; R names include dots.
pub(super) fn word_start(text: &str, offset: usize, r: bool) -> usize {
    text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_' || (r && *ch == '.'))
        .last()
        .map_or(offset, |(byte, _)| byte)
}

fn inserted_text(item: &CompletionItem) -> &str {
    match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => &edit.new_text,
        Some(CompletionTextEdit::InsertAndReplace(edit)) => &edit.new_text,
        None => item.insert_text.as_deref().unwrap_or(&item.label),
    }
}

/// Where the item's replacement starts, when that is on the cursor's line.
fn edit_start(item: &CompletionItem, text: &str, offset: usize) -> Option<usize> {
    let range = match item.text_edit.as_ref()? {
        CompletionTextEdit::Edit(edit) => edit.range,
        CompletionTextEdit::InsertAndReplace(edit) => edit.insert,
    };
    Some(component_byte(text, range.start))
        .filter(|start| *start <= offset && !text[*start..offset].contains('\n'))
}

/// `organism = ` (Ark) or `axis=` (`ty`, the kernel): a call's argument.
fn is_argument(text: &str) -> bool {
    text.trim_end()
        .strip_suffix('=')
        .map(str::trim_end)
        .is_some_and(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.'))
        })
}

/// How `query` matches `name`, best first: exact-case prefix, any-case
/// prefix, word starts (`rN` → `rowNames`), then any subsequence.
fn match_tier(query: &str, name: &str) -> Option<u8> {
    if name.starts_with(query) {
        return Some(0);
    }
    let lower = |text: &str| {
        text.chars()
            .flat_map(char::to_lowercase)
            .collect::<Vec<_>>()
    };
    let (typed, name_lower) = (lower(query), lower(name));
    if name_lower.starts_with(&typed) {
        return Some(1);
    }
    let typed: Vec<char> = typed
        .into_iter()
        .filter(|ch| ch.is_alphanumeric())
        .collect();
    if typed.is_empty() {
        return None;
    }
    if abbreviates(&typed, &words(name)) {
        return Some(2);
    }
    let mut rest = name_lower.iter();
    typed
        .iter()
        .all(|ch| rest.any(|candidate| candidate == ch))
        .then_some(3)
}

/// A name's lowercase words: split at punctuation and lower-to-upper case.
fn words(name: &str) -> Vec<Vec<char>> {
    let mut words: Vec<Vec<char>> = Vec::new();
    let mut previous: Option<char> = None;
    for ch in name.chars() {
        if !ch.is_alphanumeric() {
            previous = None;
            continue;
        }
        if previous.is_none_or(|previous| ch.is_uppercase() && !previous.is_uppercase()) {
            words.push(Vec::new());
        }
        if let Some(word) = words.last_mut() {
            word.extend(ch.to_lowercase());
        }
        previous = Some(ch);
    }
    words
}

/// Whether `typed` reads as prefixes of successive words, in order.
fn abbreviates(typed: &[char], words: &[Vec<char>]) -> bool {
    if typed.is_empty() {
        return true;
    }
    words.iter().enumerate().any(|(at, word)| {
        let common = word.iter().zip(typed).take_while(|(a, b)| a == b).count();
        (1..=common)
            .rev()
            .any(|take| abbreviates(&typed[take..], &words[at + 1..]))
    })
}

/// Keeps the items that match what is typed and orders them by match
/// quality; a call's arguments (in `library(`, packages) come first among
/// prefix matches and among looser ones; then the server's `sortText`, then
/// shorter names. One item per resulting text, the language server's first.
pub(super) fn rank(
    items: Vec<CompletionItem>,
    text: &str,
    offset: usize,
    start: usize,
    call: bool,
    library: bool,
) -> Vec<CompletionItem> {
    let mut seen = HashSet::new();
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let from = edit_start(&item, text, offset).unwrap_or(start);
            let line = text[..from].rfind('\n').map_or(0, |at| at + 1);
            let inserted = inserted_text(&item);
            // Ark's `library($0)` and the kernel's `library` are one item.
            let plain = match item.insert_text_format {
                Some(InsertTextFormat::SNIPPET) => snippet_text(inserted).0,
                _ => inserted.to_string(),
            };
            let plain = plain.strip_suffix("()").unwrap_or(&plain);
            if !seen.insert(format!("{}{plain}", &text[line..from])) {
                return None;
            }
            // An item that only inserts at the cursor (Ark's `...`) still has
            // to match the name typed before it.
            let query = &text[from.min(start)..offset];
            let name = item.filter_text.as_deref().unwrap_or(&item.label);
            let tier = match_tier(query, name).or_else(|| {
                let word = query.trim_start_matches(|ch: char| !ch.is_alphanumeric() && ch != '_');
                (word.len() < query.len())
                    .then(|| match_tier(word, name))
                    .flatten()
            })?;
            let first = if library {
                item.kind == Some(CompletionItemKind::MODULE)
            } else {
                call && (is_argument(&item.label) || is_argument(inserted))
            };
            Some(((tier >= 2, !first, tier), item))
        })
        .collect();
    ranked.sort_by(|(left, a), (right, b)| {
        left.cmp(right)
            // LSP: an item without `sortText` sorts by its label.
            .then_with(|| {
                let a_order = a.sort_text.as_deref().unwrap_or(&a.label);
                a_order.cmp(b.sort_text.as_deref().unwrap_or(&b.label))
            })
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(&b.label))
    });
    ranked.into_iter().map(|(_, item)| item).collect()
}

/// Moves the names the user has, in the Variables pane or assigned in the
/// file, ahead of everything else, keeping [`rank`]'s order within each
/// group (Huan, 2026-11-16).
pub(super) fn variables_first(items: &mut [CompletionItem], names: &HashSet<String>) {
    items.sort_by_key(|item| !names.contains(item.label.as_str()));
}

/// The names `code` assigns: `x <- …`, `x = …`, `… -> x`, `a, b = …`,
/// `def x`, `class x`, `import … as x`, `from m import x`, loop and `with`
/// variables. Line by line, as written; good enough to order a menu.
pub(super) fn assigned_names(code: &str, r: bool) -> HashSet<String> {
    let name_end = |s: &str| {
        s.find(|c: char| !(c.is_alphanumeric() || c == '_' || (r && c == '.')))
            .unwrap_or(s.len())
    };
    let whole = |s: &str| {
        let s = s
            .trim()
            .trim_start_matches('*')
            .trim_matches(['(', ')', '[', ']', ' ']);
        let s = if r { s.trim_matches('`') } else { s };
        // An annotated Python name, `x: int`.
        let s = s.split(':').next().unwrap_or(s).trim();
        (!s.is_empty() && name_end(s) == s.len() && !s.starts_with(|c: char| c.is_ascii_digit()))
            .then(|| s.to_string())
    };
    let mut names = HashSet::new();
    let add_all = |list: &str, names: &mut HashSet<String>| {
        names.extend(list.split(',').filter_map(whole));
    };
    for line in code.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let word = |prefix: &str| {
            line.strip_prefix(prefix)
                .map(|rest| &rest[..name_end(rest)])
        };
        if !r {
            for prefix in ["def ", "async def ", "class "] {
                if let Some(name) = word(prefix).filter(|n| !n.is_empty()) {
                    names.insert(name.to_string());
                }
            }
            if let Some(rest) = line.strip_prefix("import ") {
                for part in rest.split(',') {
                    let part = part.trim();
                    let name = match part.split_once(" as ") {
                        Some((_, alias)) => alias,
                        None => part.split('.').next().unwrap_or(part),
                    };
                    names.extend(whole(name));
                }
            }
            if let Some((_, rest)) = line
                .strip_prefix("from ")
                .and_then(|l| l.split_once(" import "))
            {
                for part in rest.trim_matches(['(', ')']).split(',') {
                    let part = part.trim();
                    names.extend(whole(part.split_once(" as ").map_or(part, |(_, a)| a)));
                }
            }
            if (line.starts_with("with ") || line.starts_with("except "))
                && let Some((_, alias)) = line.rsplit_once(" as ")
            {
                names.extend(whole(alias.trim_end_matches(':')));
            }
        }
        if let Some(rest) = line.strip_prefix(if r { "for (" } else { "for " })
            && let Some((targets, _)) = rest.split_once(" in ")
        {
            add_all(targets, &mut names);
        }
        if r {
            if let Some((left, _)) = line.split_once("<-") {
                names.extend(whole(left.trim_end_matches('<')));
            }
            if let Some((_, right)) = line.rsplit_once("->") {
                names.extend(whole(right.trim_start_matches('>')));
            }
        }
        // `x = …` at the line's top level, not `==`, `<=`, `!=` or `f(a = 1)`.
        let bytes = line.as_bytes();
        let mut depth = 0i32;
        for (at, &b) in bytes.iter().enumerate() {
            match b {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'"' | b'\'' => break,
                b'=' if depth == 0 => {
                    let before = at.checked_sub(1).map(|i| bytes[i]);
                    let after = bytes.get(at + 1).copied();
                    if after != Some(b'=')
                        && !matches!(
                            before,
                            Some(
                                b'=' | b'<'
                                    | b'>'
                                    | b'!'
                                    | b'+'
                                    | b'-'
                                    | b'*'
                                    | b'/'
                                    | b'%'
                                    | b'|'
                                    | b'&'
                                    | b':'
                            )
                        )
                    {
                        add_all(&line[..at], &mut names);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    names
}

/// Whether typing `inserted` after `line` (the current line up to it)
/// should ask for completions: `$ @ . [ :: ( ,` and every letter of a name,
/// from the first (`pauses` may hold the first two back); not in numbers
/// or comments.
pub(super) fn trigger(line: &str, inserted: &str) -> bool {
    let Some(last) = inserted.chars().last() else {
        return false;
    };
    let typed = format!("{line}{inserted}");
    if in_comment(&typed) {
        return false;
    }
    let numeric = |text: &str| {
        text.chars()
            .rev()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .last()
            .is_some_and(|first| first.is_ascii_digit())
    };
    match last {
        '$' | '@' | '[' | '(' | ',' => true,
        ':' => typed.ends_with("::"),
        '.' => !numeric(&typed[..typed.len() - 1]),
        ch if ch.is_alphanumeric() || ch == '_' => !numeric(&typed),
        _ => false,
    }
}

fn in_comment(line: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in line.chars() {
        match quote {
            Some(_) if escaped => escaped = false,
            Some(_) if ch == '\\' => escaped = true,
            Some(close) if ch == close => quote = None,
            Some(_) => {}
            None if ch == '#' => return true,
            None if matches!(ch, '"' | '\'' | '`') => quote = Some(ch),
            None => {}
        }
    }
    false
}

/// Whether a letter typed after `line` waits for a pause before the menu:
/// the first two letters of a word, unless the word follows `. $ @ :: ( , [`
/// (`np.l` refines at once). Tab skips the wait.
pub(super) fn pauses(line: &str, inserted: &str) -> bool {
    if !inserted
        .chars()
        .last()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
    {
        return false;
    }
    let typed = format!("{line}{inserted}");
    let word: Vec<char> = typed
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    if word.len() >= 3 {
        return false;
    }
    let bytes: usize = word.iter().map(|ch| ch.len_utf8()).sum();
    let before = typed[..typed.len() - bytes].trim_end_matches([' ', '\t']);
    !(before.ends_with(['.', '$', '@', '(', ',', '[']) || before.ends_with("::"))
}

/// A snippet's text without its syntax, and the cursor's byte in it: at
/// `$0`, else at the lowest-numbered tab stop. Placeholders keep their
/// default, choices their first option, variables their default (if any).
fn snippet_text(snippet: &str) -> (String, Option<usize>) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut text = String::new();
    let mut stops = Vec::new();
    let mut at = 0;
    snippet_part(&chars, &mut at, &mut text, &mut stops, false);
    let cursor = stops
        .iter()
        .find(|(stop, _)| *stop == 0)
        .or_else(|| stops.iter().filter(|(stop, _)| *stop > 0).min())
        .map(|(_, byte)| *byte);
    (text, cursor)
}

/// Text up to the end of `chars` (or a placeholder's `}` when `nested`).
fn snippet_part(
    chars: &[char],
    at: &mut usize,
    text: &mut String,
    stops: &mut Vec<(u32, usize)>,
    nested: bool,
) {
    while let Some(&ch) = chars.get(*at) {
        match ch {
            '\\' if matches!(chars.get(*at + 1), Some('$' | '}' | '\\')) => {
                text.push(chars[*at + 1]);
                *at += 2;
            }
            '}' if nested => {
                *at += 1;
                return;
            }
            '$' => {
                *at += 1;
                if !snippet_element(chars, at, text, stops) {
                    text.push('$');
                }
            }
            _ => {
                text.push(ch);
                *at += 1;
            }
        }
    }
}

/// `$1`, `${1}`, `${1:default}`, `${1|a,b|}`, `$name`, `${name:default}`
/// after a `$`; false (nothing consumed) when it is a literal `$`.
fn snippet_element(
    chars: &[char],
    at: &mut usize,
    text: &mut String,
    stops: &mut Vec<(u32, usize)>,
) -> bool {
    let start = *at;
    let scan = |at: &mut usize, keep: fn(&char) -> bool| {
        let from = *at;
        while chars.get(*at).is_some_and(keep) {
            *at += 1;
        }
        chars[from..*at].iter().collect::<String>()
    };
    let name_start = |ch: &char| ch.is_ascii_alphabetic() || *ch == '_';
    let name_char = |ch: &char| ch.is_ascii_alphanumeric() || *ch == '_';
    match chars.get(*at) {
        Some(ch) if ch.is_ascii_digit() => {
            let stop = scan(at, char::is_ascii_digit).parse().unwrap_or(u32::MAX);
            stops.push((stop, text.len()));
            true
        }
        Some(ch) if name_start(ch) => {
            scan(at, name_char);
            true
        }
        Some('{') => {
            *at += 1;
            let stop = match chars.get(*at) {
                Some(ch) if ch.is_ascii_digit() => {
                    Some(scan(at, char::is_ascii_digit).parse().unwrap_or(u32::MAX))
                }
                Some(ch) if name_start(ch) => {
                    scan(at, name_char);
                    None
                }
                _ => {
                    *at = start;
                    return false;
                }
            };
            match chars.get(*at) {
                Some('}' | ':') => {}
                Some('|') if stop.is_some() => {}
                _ => {
                    *at = start;
                    return false;
                }
            }
            if let Some(stop) = stop {
                stops.push((stop, text.len()));
            }
            match chars[*at] {
                '}' => *at += 1,
                ':' => {
                    *at += 1;
                    snippet_part(chars, at, text, stops, true);
                }
                _ => {
                    *at += 1;
                    let mut first = true;
                    while let Some(&ch) = chars.get(*at) {
                        match ch {
                            '\\' if matches!(
                                chars.get(*at + 1),
                                Some(',' | '|' | '\\' | '$' | '}')
                            ) =>
                            {
                                if first {
                                    text.push(chars[*at + 1]);
                                }
                                *at += 2;
                                continue;
                            }
                            ',' => first = false,
                            '|' if chars.get(*at + 1) == Some(&'}') => {
                                *at += 2;
                                break;
                            }
                            _ if first => text.push(ch),
                            _ => {}
                        }
                        *at += 1;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// Makes an item insert plain text and says where the cursor belongs: a
/// snippet loses its syntax; a Python function, method or constructor gets
/// `()` (not in `import`, `def`, `class` or decorator lines); neither adds
/// `()` before an existing `(`.
pub(super) fn finish(item: &mut CompletionItem, text: &str, python: bool) -> Option<Accept> {
    let range = match item.text_edit.as_ref()? {
        CompletionTextEdit::Edit(edit) => edit.range,
        CompletionTextEdit::InsertAndReplace(edit) => edit.replace,
    };
    let start = component_byte(text, range.start);
    let end = component_byte(text, range.end).max(start);
    let (mut plain, mut cursor) = if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
        item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
        snippet_text(inserted_text(item))
    } else {
        (inserted_text(item).to_string(), None)
    };
    let next = text[end..].chars().next();
    let line = &text[text[..start].rfind('\n').map_or(0, |at| at + 1)..start];
    if python
        && cursor.is_none()
        && matches!(
            item.kind,
            Some(
                CompletionItemKind::FUNCTION
                    | CompletionItemKind::METHOD
                    | CompletionItemKind::CONSTRUCTOR
            )
        )
        && !plain.contains('(')
        && !next.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        && !["import ", "from ", "def ", "class ", "@"]
            .iter()
            .any(|prefix| line.trim_start().starts_with(prefix))
    {
        plain.push_str("()");
        cursor = Some(plain.len() - 1);
    }
    if next == Some('(') && plain.ends_with("()") && cursor == Some(plain.len() - 1) {
        plain.truncate(plain.len() - 2);
        cursor = None;
    }
    match item.text_edit.as_mut()? {
        CompletionTextEdit::Edit(edit) => edit.new_text.clone_from(&plain),
        CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.clone_from(&plain),
    }
    let cursor = cursor.filter(|cursor| *cursor < plain.len())?;
    Some(Accept {
        start,
        end,
        text: plain,
        cursor,
    })
}

/// Where the cursor goes when the edit from `old` to `new` (cursor at
/// `cursor`) is Kit inserting one of `accepts`; clears them when it is.
pub(super) fn accepted(
    accepts: &mut Vec<Accept>,
    old: &Rope,
    new: &Rope,
    cursor: usize,
) -> Option<usize> {
    let found = accepts.iter().find(|accept| {
        let end = accept.start + accept.text.len();
        accept.end <= old.len()
            && new.len() + (accept.end - accept.start) == old.len() + accept.text.len()
            && cursor == end
            && end <= new.len()
            && new.is_char_boundary(accept.start)
            && new.is_char_boundary(end)
            && new.slice(accept.start..end) == accept.text
    })?;
    let at = found.start + found.cursor;
    accepts.clear();
    Some(at)
}

/// Kit's query runs from where the menu opened (`.`, `(`, `$`…); paint the
/// typed name's characters where they matched.
pub(super) fn highlight(query: &str, item: &CompletionItem) -> Vec<std::ops::Range<usize>> {
    let start = query
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || matches!(ch, '_' | '.'))
        .last()
        .map_or(query.len(), |(at, _)| at);
    highlight_characters(query[start..].trim_start_matches('.'), item)
}

#[cfg(test)]
mod tests;
