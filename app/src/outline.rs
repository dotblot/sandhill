//! Local structural navigation. Line numbers and fold ranges are cell-relative.
use crate::look::{self, Palette, icon_button};
use crate::workbench::Workbench;
use gpui_kit::base::ElementExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState, Position};
use gpui_kit::component::{Sizable, h_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub line: usize,
    pub level: usize,
    pub title: String,
}

fn section(line: &str) -> Option<&str> {
    let line = line.trim();
    if !line.starts_with('#') {
        return None;
    }
    let marker = line.chars().last()?;
    if !matches!(marker, '-' | '=' | '#') {
        return None;
    }
    let title = line.trim_end_matches(marker);
    if line.len() - title.len() < 4 {
        return None;
    }
    let title = title.trim_start_matches('#').trim();
    (!title.is_empty()).then_some(title)
}

/// Markdown headings inside fenced examples are not document headings.
pub(crate) fn entries(text: &str, language: &str) -> Vec<Entry> {
    let mut result = Vec::new();
    let markdown = language == "markdown";
    let comments = if markdown {
        Vec::new()
    } else {
        crate::statements::comment_lines(text, language == "python")
    };
    let mut fence: Option<(char, usize)> = None;
    for (line, source) in text.lines().enumerate() {
        let trimmed = source.trim();
        if markdown && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            let marker = trimmed.chars().next().unwrap();
            let width = trimmed.chars().take_while(|c| *c == marker).count();
            if let Some((ch, len)) = fence {
                if ch == marker && width >= len {
                    fence = None;
                }
            } else {
                fence = Some((marker, width));
                if let Some(info) = trimmed[width..]
                    .strip_prefix('{')
                    .and_then(|s| s.strip_suffix('}'))
                {
                    let mut words = info.split_whitespace();
                    words.next();
                    if let Some(label) = words.next() {
                        result.push(Entry {
                            line,
                            level: 0,
                            title: label.trim_end_matches(',').into(),
                        });
                    }
                }
            }
            continue;
        }
        if markdown {
            if fence.is_some() {
                continue;
            }
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            if (1..=6).contains(&level) && trimmed[level..].starts_with(char::is_whitespace) {
                result.push(Entry {
                    line,
                    level: level - 1,
                    title: trimmed[level..].trim().trim_end_matches('#').trim().into(),
                });
            }
        } else if comments[line]
            && let Some(title) = section(source)
        {
            result.push(Entry {
                line,
                level: 0,
                title: title.into(),
            });
        } else if comments[line]
            && let Some(label) = trimmed
                .strip_prefix("#|")
                .and_then(|s| s.trim().strip_prefix("label:"))
        {
            let title = label.trim().trim_matches(['\'', '"']);
            if !title.is_empty() {
                result.push(Entry {
                    line,
                    level: 0,
                    title: title.into(),
                });
            }
        }
    }
    if language == "python" {
        result.extend(
            crate::statements::python_declarations(text)
                .into_iter()
                .map(|(line, title)| Entry {
                    line,
                    level: 0,
                    title,
                }),
        );
    }
    if language == "r" {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&tree_sitter_r::LANGUAGE.into()).is_ok()
            && let Some(tree) = parser.parse(text, None)
        {
            let root = tree.root_node();
            let mut cursor = root.walk();
            for node in root.named_children(&mut cursor) {
                if node.kind() == "binary_operator"
                    && node
                        .child_by_field_name("operator")
                        .is_some_and(|operator| operator.kind() == "<-")
                    && node
                        .child_by_field_name("rhs")
                        .is_some_and(|rhs| rhs.kind() == "function_definition")
                    && let Some(name) = node.child_by_field_name("lhs")
                    && name.kind() == "identifier"
                {
                    result.push(Entry {
                        line: node.start_position().row,
                        level: 0,
                        title: text[name.byte_range()].into(),
                    });
                }
            }
        }
    }
    result.sort_by_key(|entry| entry.line);
    result
}

pub(crate) fn fold_ranges(
    text: &str,
    language: &str,
) -> Vec<gpui_kit::component::input::FoldRange> {
    use gpui_kit::component::input::FoldRange;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut ranges = Vec::new();
    let headings: Vec<_> = entries(text, language)
        .into_iter()
        .filter(|entry| {
            (language == "markdown" && lines[entry.line].trim_start().starts_with('#'))
                || (language != "markdown" && section(lines[entry.line]).is_some())
        })
        .collect();
    for (ix, entry) in headings.iter().enumerate() {
        let end = headings[ix + 1..]
            .iter()
            .find(|next| next.level <= entry.level)
            .map_or(lines.len(), |next| next.line);
        if end > entry.line + 1 {
            ranges.push(FoldRange::new(entry.line, end));
        }
    }
    if language == "r" {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&tree_sitter_r::LANGUAGE.into()).is_ok()
            && let Some(tree) = parser.parse(text, None)
        {
            fn visit(node: tree_sitter::Node<'_>, ranges: &mut Vec<FoldRange>) {
                let mut cursor = node.walk();
                let mut start = None;
                for child in node.children(&mut cursor) {
                    if child.kind() == "{" {
                        start = Some(child.start_position().row);
                    }
                    if child.kind() == "}"
                        && let Some(from) = start
                    {
                        let to = child.start_position().row;
                        if to > from + 1 {
                            ranges.push(FoldRange::new(from, to));
                        }
                    }
                    if child.is_named() {
                        visit(child, ranges);
                    }
                }
            }
            visit(tree.root_node(), &mut ranges);
        }
    } else if language == "python" {
        // Reuse the statement scanner, which excludes comments and tracks
        // multiline strings, so example code in docstrings cannot create folds.
        ranges.extend(
            crate::statements::python_folds(text)
                .into_iter()
                .map(|(start, end)| FoldRange::new(start, end)),
        );
    }
    ranges.sort_by_key(|range| (range.start_line, std::cmp::Reverse(range.end_line)));
    ranges.dedup_by_key(|range| range.start_line);
    ranges
}

pub(crate) struct Outline {
    pub open: bool,
    pub filter: Entity<InputState>,
}

pub(crate) fn new_outline(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Outline, Subscription) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter outline"));
    let subscription = cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify());
    (
        Outline {
            open: false,
            filter,
        },
        subscription,
    )
}

impl Workbench {
    pub(crate) fn outline_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let button = icon_button("outline-trigger", "outline", &p, false)
            .when(self.outline.open, |d| d.bg(p.ink(10.)))
            .on_prepaint(self.popup_triggers.track("outline"))
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Outline").build(w, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.outline.open = !this.outline.open;
                if this.outline.open {
                    this.outline
                        .filter
                        .update(cx, |input, cx| input.focus(window, cx));
                }
                cx.notify();
            }));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when(self.outline.open, |d| {
                let filter = self.outline.filter.read(cx).value().to_lowercase();
                let mut rows = Vec::new();
                if let Some(open) = &self.open {
                    for (cell, editor) in open.editors.iter().enumerate() {
                        let language = open.notebook.cell_language(cell).unwrap_or("markdown");
                        let text = editor.read(cx).value();
                        let mut items = entries(&text, language);
                        if let Some(label) = open.notebook.chunk_label(cell)
                            && !text.lines().any(|line| {
                                line.trim_start()
                                    .strip_prefix("#|")
                                    .is_some_and(|option| option.trim_start().starts_with("label:"))
                            })
                            && !items.iter().any(|entry| entry.title == label)
                        {
                            items.insert(
                                0,
                                Entry {
                                    line: 0,
                                    level: 0,
                                    title: label.into(),
                                },
                            );
                        }
                        for entry in items
                            .into_iter()
                            .filter(|entry| entry.title.to_lowercase().contains(&filter))
                        {
                            let line = entry.line;
                            rows.push(
                                h_flex()
                                    .id(("outline-entry", rows.len()))
                                    .w_full()
                                    .py(px(5.))
                                    .pl(px(8. + entry.level as f32 * 12.))
                                    .pr(px(10.))
                                    .cursor_pointer()
                                    .hover(|d| d.bg(p.ink(8.)))
                                    .child(entry.title)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.outline.open = false;
                                        this.jump_to_source(cell, line, None, window, cx);
                                    }))
                                    .into_any_element(),
                            );
                        }
                    }
                }
                let popup = crate::agents::composer_view::popup(&p)
                    .id("outline-popup")
                    .occlude()
                    .w(px(300.))
                    .text_size(px(look::SMALL))
                    .child(Input::new(&self.outline.filter).small())
                    .child(
                        div()
                            .id("outline-items")
                            .max_h(px(360.))
                            .overflow_y_scroll()
                            .children(rows),
                    )
                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if !this.popup_triggers.contains("outline", event.position) {
                            this.outline.open = false;
                            cx.notify();
                        }
                    }));
                d.child(
                    div().absolute().top(px(25.)).left_0().child(
                        deferred(anchored().snap_to_window_with_margin(px(8.)).child(popup))
                            .with_priority(3),
                    ),
                )
            })
            .into_any_element()
    }

    pub(crate) fn jump_to_source(
        &mut self,
        cell: usize,
        line: usize,
        range: Option<std::ops::Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(editor) = open.editors.get(cell).cloned() else {
            return;
        };
        open.markdown_preview = false;
        open.current = Some(cell);
        open.editing.insert(open.notebook.cells[cell].id.clone());
        let script = open.single_editor();
        let path = open.notebook.path.clone();
        let position = Position::new(line as u32, 0);
        editor.update(cx, |editor, cx| {
            editor.unfold_at(position, cx);
            editor.set_cursor_position(position, window, cx);
            if let Some(range) = range {
                editor.set_selected_range(range, cx);
            }
            editor.focus(window, cx);
        });
        self.scroll_to_cell(cell);
        if !script {
            let this = cx.entity().downgrade();
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |_, cx| {
                    let _ = this.update(cx, |this, cx| {
                        let Some(open) = &this.open else {
                            return;
                        };
                        if open.notebook.path != path {
                            return;
                        }
                        let Some(editor) = open.editors.get(cell) else {
                            return;
                        };
                        let child = Self::child_of_cell(open, cell);
                        let scroll = &this.notebook_scroll;
                        if let Some(bounds) = scroll.bounds_for_item(child) {
                            let row = editor
                                .read(cx)
                                .display_map()
                                .buffer_line_to_display_row(line);
                            let top = bounds.top() - scroll.bounds().top() + px(row as f32 * 20.);
                            let mut offset = scroll.offset();
                            offset.y = (px(this.notebook_header_height()) - top)
                                .clamp(-scroll.max_offset().y, px(0.));
                            scroll.set_offset(offset);
                            cx.notify();
                        }
                    });
                });
            });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{entries, fold_ranges};
    #[test]
    fn headings_ignore_fences_and_keep_levels() {
        let items = entries("# One\n```r\n# Not a heading\n```\n### Three", "markdown");
        assert_eq!(
            items
                .iter()
                .map(|e| (e.line, e.level, e.title.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, 0, "One"), (4, 2, "Three")]
        );
    }
    #[test]
    fn r_sections_functions_and_braces() {
        let text = "# Setup ----\nf <- function(x) {\n  print('}')\n  x\n}\n# Next ====\nx <- 1";
        assert_eq!(
            entries(text, "r")
                .iter()
                .map(|e| e.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Setup", "f", "Next"]
        );
        assert_eq!(
            fold_ranges(text, "r")
                .iter()
                .map(|r| (r.start_line, r.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 5), (1, 4), (5, 7)]
        );
    }
    #[test]
    fn python_nested_blocks() {
        let text = "def f():\n    if True:\n        print(':')\n        pass\n    return 1\n\nclass C:\n    pass\n    pass";
        assert_eq!(
            fold_ranges(text, "python")
                .iter()
                .map(|r| (r.start_line, r.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 5), (1, 4), (6, 9)]
        );
        assert_eq!(
            entries(text, "python")
                .iter()
                .map(|e| e.title.as_str())
                .collect::<Vec<_>>(),
            vec!["f", "C"]
        );
    }
    #[test]
    fn headings_fold_until_the_next_peer_and_through_eof() {
        let text = "# A\none\n## B\ntwo\n# C\nthree";
        assert_eq!(
            fold_ranges(text, "markdown")
                .iter()
                .map(|r| (r.start_line, r.end_line))
                .collect::<Vec<_>>(),
            vec![(0, 4), (2, 4), (4, 6)]
        );
    }

    #[test]
    fn python_docstrings_do_not_add_symbols_or_suites() {
        let text = "\"\"\"\ndef fake():\n    pass\n\"\"\"\nasync def real(\n    value,\n):\n    return value";
        assert_eq!(
            entries(text, "python")
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["real"]
        );
        assert_eq!(
            fold_ranges(text, "python")
                .iter()
                .map(|range| (range.start_line, range.end_line))
                .collect::<Vec<_>>(),
            vec![(4, 8)]
        );
    }
}
