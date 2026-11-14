//! Lossless, bounded highlighting for bioinformatics and workflow text.
//! Decorations never replace editor text. Limit work and range count so long
//! sequences and giant records cannot stall editing or allocate millions of marks.
use std::ops::Range;

use gpui_kit::component::input::{EditorState, InputEvent, TextDecoration};
use gpui_kit::*;

use crate::look::Palette;
use crate::workbench::Workbench;

const HIGHLIGHT_BYTES: usize = 262_144;
const MAX_MARKS: usize = 50_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ink {
    Muted,
    Header,
    Base(u8),
    Column(usize),
    Keyword,
    String,
}

#[derive(Debug, PartialEq, Eq)]
struct Mark {
    range: Range<usize>,
    ink: Ink,
}

fn push(out: &mut Vec<Mark>, range: Range<usize>, ink: Ink) {
    if !range.is_empty() && out.len() < MAX_MARKS {
        out.push(Mark { range, ink });
    }
}

fn bases(text: &str, offset: usize, out: &mut Vec<Mark>) {
    let mut start = 0;
    let mut previous = None;
    for (i, byte) in text.bytes().enumerate() {
        let ink = match byte.to_ascii_uppercase() {
            b'A' => Some(Ink::Base(0)),
            b'C' => Some(Ink::Base(1)),
            b'G' => Some(Ink::Base(2)),
            b'T' | b'U' => Some(Ink::Base(3)),
            b'N' => Some(Ink::Base(4)),
            _ => None,
        };
        if ink != previous {
            if let Some(ink) = previous {
                push(out, offset + start..offset + i, ink);
            }
            previous = ink;
            start = i;
        }
        if out.len() == MAX_MARKS {
            return;
        }
    }
    if let Some(ink) = previous {
        push(out, offset + start..offset + text.len(), ink);
    }
}

fn sequence_marks(text: &str, fastq: bool) -> Vec<Mark> {
    let mut out = Vec::new();
    let mut offset = 0;
    // FASTQ sequence/quality may span lines. Quality is counted, not guessed
    // from '@' or '+', both of which are valid quality characters.
    let mut state = 0;
    let mut sequence_len = 0;
    let mut quality_len = 0;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        if !fastq {
            if line.starts_with('>') {
                push(&mut out, offset..offset + line.len(), Ink::Header);
            } else if line.starts_with(';') {
                push(&mut out, offset..offset + line.len(), Ink::Muted);
            } else {
                bases(line, offset, &mut out);
            }
        } else {
            match state {
                0 if line.starts_with('@') => {
                    push(&mut out, offset..offset + line.len(), Ink::Header);
                    sequence_len = 0;
                    quality_len = 0;
                    state = 1;
                }
                1 if line.starts_with('+') => {
                    push(&mut out, offset..offset + line.len(), Ink::Muted);
                    state = 2;
                }
                1 => {
                    sequence_len += line.len();
                    bases(line, offset, &mut out);
                }
                2 => {
                    push(&mut out, offset..offset + line.len(), Ink::Muted);
                    quality_len += line.len();
                    if quality_len >= sequence_len {
                        state = 0;
                    }
                }
                _ => {}
            }
        }
        offset += raw.len();
        if out.len() == MAX_MARKS {
            break;
        }
    }
    out
}

fn table_marks(text: &str) -> Vec<Mark> {
    let mut out = Vec::new();
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\r', '\n']);
        if line.trim_start().starts_with(['#', '@', '%']) {
            push(&mut out, offset..offset + line.len(), Ink::Muted);
        } else {
            let mut start = 0;
            let mut column = 0;
            let tabs = line.contains('\t');
            for (i, byte) in line.bytes().enumerate() {
                if byte == b'\t' || (!tabs && byte.is_ascii_whitespace()) {
                    push(&mut out, offset + start..offset + i, Ink::Column(column));
                    if tabs || start < i {
                        column += 1;
                    }
                    start = i + 1;
                }
                if out.len() == MAX_MARKS {
                    break;
                }
            }
            push(
                &mut out,
                offset + start..offset + line.len(),
                Ink::Column(column),
            );
        }
        offset += raw.len();
        if out.len() == MAX_MARKS {
            break;
        }
    }
    out
}

fn keyword_marks(text: &str, language: &str) -> Vec<Mark> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && out.len() < MAX_MARKS {
        let start = i;
        let byte = bytes[i];
        if byte == b'#' || bytes.get(i..i + 2) == Some(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            push(&mut out, start..i, Ink::Muted);
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            i += 2;
            while i < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            push(&mut out, start..i, Ink::Muted);
        } else if byte == b'\'' || byte == b'"' {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == byte {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            push(&mut out, start..i, Ink::String);
        } else if language == "bibtex" && byte == b'%' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            push(&mut out, start..i, Ink::Muted);
        } else if byte.is_ascii_alphabetic()
            || byte == b'_'
            || (language == "bibtex" && byte == b'@')
        {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let keyword = match language {
                "nextflow" => matches!(
                    word,
                    "process"
                        | "workflow"
                        | "input"
                        | "output"
                        | "script"
                        | "shell"
                        | "exec"
                        | "when"
                        | "include"
                        | "from"
                        | "params"
                        | "Channel"
                        | "emit"
                        | "take"
                        | "main"
                        | "publishDir"
                        | "container"
                        | "conda"
                ),
                "wdl" => matches!(
                    word,
                    "version"
                        | "task"
                        | "workflow"
                        | "call"
                        | "input"
                        | "output"
                        | "command"
                        | "runtime"
                        | "meta"
                        | "parameter_meta"
                        | "scatter"
                        | "in"
                        | "if"
                        | "as"
                        | "import"
                        | "String"
                        | "Int"
                        | "Float"
                        | "Boolean"
                        | "File"
                        | "Array"
                        | "Map"
                        | "Object"
                ),
                "bibtex" => word.starts_with('@') || text[i..].trim_start().starts_with('='),
                _ => false,
            };
            if keyword {
                push(&mut out, start..i, Ink::Keyword);
            }
        } else {
            // Advance a whole codepoint, so all decoration endpoints are UTF-8 boundaries.
            i += text[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out
}

fn marks(text: &str, language: &str) -> Vec<Mark> {
    match language {
        "fasta" => sequence_marks(text, false),
        "fastq" => sequence_marks(text, true),
        "bio-table" => table_marks(text),
        _ => keyword_marks(text, language),
    }
}

fn decorate(editor: &Entity<EditorState>, language: &str, cx: &App) -> Vec<TextDecoration> {
    let rope = editor.read(cx).text();
    let end = rope.char_to_byte_idx(rope.byte_to_char_idx(rope.len().min(HIGHLIGHT_BYTES)));
    let text = rope.slice(..end).to_string();
    let p = Palette::get(cx);
    marks(&text, language)
        .into_iter()
        .map(|mark| {
            let color = match mark.ink {
                Ink::Muted => p.muted,
                Ink::Header => p.ansi[4],
                Ink::Base(index) => p.ansi[[2, 4, 3, 1, 8][index as usize]],
                Ink::Column(column) => p.ansi[[4, 2, 5, 6, 3, 1][column % 6]],
                Ink::Keyword => p.ansi[5],
                Ink::String => p.ansi[2],
            };
            TextDecoration::new(
                mark.range,
                HighlightStyle {
                    color: Some(color),
                    ..Default::default()
                },
            )
        })
        .collect()
}

pub(crate) fn decorate_editor(
    editor: &Entity<EditorState>,
    language: Option<&str>,
    cx: &mut Context<Workbench>,
) -> Vec<Subscription> {
    let Some(language @ ("fasta" | "fastq" | "bio-table" | "nextflow" | "wdl" | "bibtex")) =
        language
    else {
        return Vec::new();
    };
    let language = language.to_owned();
    let collection = editor.update(cx, |state, cx| {
        state.create_decorations_collection(Vec::new(), cx)
    });
    collection.set(decorate(editor, &language, cx), cx);
    let body = editor.clone();
    let theme_collection = collection.clone();
    let theme_language = language.clone();
    let theme = cx.observe_global::<Palette>(move |_, cx| {
        theme_collection.set(decorate(&body, &theme_language, cx), cx);
    });
    let changes = cx.subscribe(editor, move |_, editor, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            collection.set(decorate(&editor, &language, cx), cx);
        }
    });
    vec![theme, changes]
}

#[cfg(test)]
mod tests {
    use super::{Ink, keyword_marks, sequence_marks, table_marks};

    #[test]
    fn fasta_headers_and_base_ranges_preserve_original_offsets() {
        let text = ">α\r\nAACtN\r\n";
        let marks = sequence_marks(text, false);
        assert_eq!(marks[0].range, 0..3);
        assert_eq!(marks[0].ink, Ink::Header);
        assert_eq!(&text[marks[1].range.clone()], "AA");
        assert_eq!(marks[1].ink, Ink::Base(0));
        assert_eq!(&text[marks[4].range.clone()], "N");
        assert!(marks.iter().all(|m| text.get(m.range.clone()).is_some()));
    }

    #[test]
    fn fastq_multiline_quality_is_not_mistaken_for_headers() {
        let text = "@one\nAC\nGT\n+\n@@\n++\n@two\nN\n+\n!\n";
        let marks = sequence_marks(text, true);
        let headers: Vec<_> = marks
            .iter()
            .filter(|m| m.ink == Ink::Header)
            .map(|m| &text[m.range.clone()])
            .collect();
        assert_eq!(headers, ["@one", "@two"]);
        for quality in ["@@", "++", "!"] {
            assert!(
                marks
                    .iter()
                    .any(|m| m.ink == Ink::Muted && &text[m.range.clone()] == quality)
            );
        }
    }

    #[test]
    fn vcf_and_bed_headers_columns_and_empty_fields() {
        for text in [
            "##vcf\r\n#CHROM\tPOS\r\nchrα\t\t42\r\n",
            "chr1\t10\t20\n",
            "@HD\tVN:1\n%matrix\n",
        ] {
            let marks = table_marks(text);
            assert!(marks.iter().all(|m| text.get(m.range.clone()).is_some()));
            for mark in &marks {
                if let Ink::Column(_) = mark.ink {
                    assert!(!text[mark.range.clone()].contains(['\t', '\r', '\n']));
                }
            }
            if text.starts_with("##") {
                assert!(
                    marks
                        .iter()
                        .any(|m| m.ink == Ink::Column(2) && &text[m.range.clone()] == "42")
                );
                assert_eq!(marks[0].ink, Ink::Muted);
            }
        }
    }

    #[test]
    fn keyword_strings_comments_and_bib_fields() {
        let text = "workflow { process X // task\n script: \"input α\" }";
        let marks = keyword_marks(text, "nextflow");
        assert!(
            marks
                .iter()
                .any(|m| m.ink == Ink::Keyword && &text[m.range.clone()] == "workflow")
        );
        assert!(
            marks
                .iter()
                .any(|m| m.ink == Ink::Muted && &text[m.range.clone()] == "// task")
        );
        assert!(
            marks
                .iter()
                .any(|m| m.ink == Ink::String && &text[m.range.clone()] == "\"input α\"")
        );
        let bib = "@article{key, title = \"Hi\", year={2026}}";
        let fields: Vec<_> = keyword_marks(bib, "bibtex")
            .into_iter()
            .filter(|m| m.ink == Ink::Keyword)
            .map(|m| bib[m.range].to_string())
            .collect();
        assert_eq!(fields, ["@article", "title", "year"]);
        let wdl = "task hello { input { File x } command { echo hi } }";
        assert!(
            keyword_marks(wdl, "wdl")
                .iter()
                .any(|m| &wdl[m.range.clone()] == "File")
        );
    }
}
