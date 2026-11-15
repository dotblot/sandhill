use super::{
    CompletionItem, CompletionItemKind, CompletionReply, CompletionTextEdit, Document, HELP_TOPIC,
    InsertTextFormat, Position, Range, TextEdit, Value, coalesce_edits, finish, kernel_items,
    map_completion, match_tier, pauses, rank, snippet_text, trigger,
};
fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|item| item.label.as_str()).collect()
}
fn item(label: &str) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        ..Default::default()
    }
}
#[test]
fn ranking_filters_and_orders_by_match() {
    let items = [
        "palette",
        "calibrate",
        "lib_rename",
        "LIBRARY_X",
        "libraryX_long",
        "library",
    ]
    .map(item)
    .to_vec();
    assert_eq!(
        labels(&rank(items, "x <- libr", 9, 5, false, false)),
        vec![
            "library",
            "libraryX_long",
            "LIBRARY_X",
            "lib_rename",
            "calibrate"
        ]
    );
    assert_eq!(match_tier("rN", "rowNames"), Some(2));
    assert_eq!(match_tier("rn", "rowNames"), Some(2));
    assert_eq!(match_tier("rwn", "rowNames"), Some(3));
    assert_eq!(match_tier("x", "rowNames"), None);
    // Ties keep the server's order, then shorter names.
    let mut late = item("mean");
    late.sort_text = Some("1".into());
    let mut early = item("median");
    early.sort_text = Some("0".into());
    assert_eq!(
        labels(&rank(
            vec![late, early, item("me")],
            "me",
            2,
            0,
            false,
            false
        )),
        vec!["median", "mean", "me"]
    );
    // An empty word keeps everything.
    assert_eq!(
        labels(&rank(vec![item("b"), item("a")], "x$", 2, 2, false, false)).len(),
        2
    );
    // An item inserted at the cursor must still match the typed name:
    // Ark's `...` in `seq(le|` is not offered.
    let dots = CompletionItem {
        label: "...".into(),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, 6), Position::new(0, 6)),
            new_text: "...".into(),
        })),
        ..Default::default()
    };
    assert_eq!(
        labels(&rank(
            vec![dots, item("length")],
            "seq(le",
            6,
            4,
            true,
            false
        )),
        vec!["length"]
    );
}
#[test]
fn arguments_and_packages_come_first() {
    let items = ["org.Hs.eg.db", "orgDb", "organism = ", "color = ", "order"]
        .map(item)
        .to_vec();
    // Inside `getKEGG(or|`: arguments lead each match group.
    assert_eq!(
        labels(&rank(items.clone(), "getKEGG(or", 10, 8, true, false)),
        vec!["organism = ", "order", "org.Hs.eg.db", "orgDb", "color = "]
    );
    // Just after `(`: every argument before every other name.
    assert_eq!(
        labels(&rank(items, "getKEGG(", 8, 8, true, false))[..2],
        ["color = ", "organism = "]
    );
    let mut package = item("ggplot2");
    package.kind = Some(CompletionItemKind::MODULE);
    assert_eq!(
        labels(&rank(
            vec![item("gg"), item("ggg"), package],
            "library(gg",
            10,
            8,
            true,
            true
        )),
        vec!["ggplot2", "gg", "ggg"]
    );
}
#[test]
fn duplicate_results_keep_the_server_item() {
    let edit = |label: &str, start: u32, detail: &str| CompletionItem {
        label: label.into(),
        detail: Some(detail.into()),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(Position::new(0, start), Position::new(0, 5)),
            new_text: label.into(),
        })),
        ..Default::default()
    };
    let items = rank(
        vec![
            edit("mean", 3, "server"),
            edit("np.mean", 0, "kernel"),
            edit("np.median", 0, "kernel"),
        ],
        "np.me",
        5,
        3,
        false,
        false,
    );
    assert_eq!(labels(&items), vec!["mean", "np.median"]);
    assert_eq!(items[0].detail.as_deref(), Some("server"));
}
#[test]
fn cell_mapping_keeps_items_spanning_the_cursor() {
    let document = Document::new(
        "/p/a.ipynb",
        "r",
        false,
        None,
        [(0, "x <- 1".to_string()), (1, "getKEGG(org".to_string())],
    );
    // Cell 1 starts on the virtual document's third line.
    let cursor = document.position(1, 11).unwrap();
    let token = Range::new(Position::new(0, 8), Position::new(0, 11));
    let item = |start: Position, end: Position| CompletionItem {
        label: "organism = ".into(),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(start, end),
            new_text: "organism = ".into(),
        })),
        ..Default::default()
    };
    let mut across = item(Position::new(0, 0), Position::new(2, 11));
    assert!(map_completion(&mut across, &document, 1, cursor, token));
    let Some(CompletionTextEdit::Edit(edit)) = &across.text_edit else {
        panic!("expected edit");
    };
    assert_eq!(edit.range, token);
    let mut past = item(Position::new(2, 8), Position::new(2, 40));
    assert!(map_completion(&mut past, &document, 1, cursor, token));
    let mut inside = item(Position::new(2, 8), Position::new(2, 11));
    assert!(map_completion(&mut inside, &document, 1, cursor, token));
    let Some(CompletionTextEdit::Edit(edit)) = &inside.text_edit else {
        panic!("expected edit");
    };
    assert_eq!(edit.range, token);
    let mut elsewhere = item(Position::new(0, 0), Position::new(0, 1));
    assert!(!map_completion(&mut elsewhere, &document, 1, cursor, token));
}
#[test]
fn automatic_trigger_rules() {
    for special in ["$", "@", ".", "[", "(", ","] {
        assert!(trigger("x", special));
    }
    assert!(trigger("pkg:", ":"));
    assert!(trigger("pkg", "::"));
    assert!(!trigger("pkg", ":"));
    // Plain words from their first letter.
    assert!(trigger("", "l"));
    assert!(trigger("x <- ", "l"));
    assert!(trigger("np.", "a"));
    assert!(trigger("a", "b"));
    assert!(trigger("名列", "名"));
    assert!(trigger("getKEGG(", "o"));
    assert!(!trigger("abc", " "));
    // Not in numbers or comments; a `#` in a string is no comment.
    assert!(!trigger("x <- 1", "2"));
    assert!(!trigger("x <- 1", "."));
    assert!(!trigger("1.", "5"));
    assert!(trigger("x1", "."));
    assert!(!trigger("# see ", "x"));
    assert!(!trigger("f(x) # ", "("));
    assert!(trigger("f(\"#\", ", "y"));
}
#[test]
fn first_letters_of_a_plain_word_wait_for_a_pause() {
    assert!(pauses("", "l"));
    assert!(pauses("l", "i"));
    assert!(pauses("x <- ", "g"));
    assert!(!pauses("li", "b"));
    assert!(!pauses("x <- libr", "a"));
    // After a trigger character every letter refines at once.
    assert!(!pauses("np.", "l"));
    assert!(!pauses("np.l", "i"));
    assert!(!pauses("df$", "a"));
    assert!(!pauses("obj@", "s"));
    assert!(!pauses("stats::", "l"));
    assert!(!pauses("f(", "x"));
    assert!(!pauses("f(a, ", "b"));
    assert!(!pauses("x[", "i"));
    // Trigger characters themselves never wait.
    assert!(!pauses("np", "."));
    assert!(!pauses("f", "("));
}
#[test]
fn snippets_become_text_with_the_cursor_at_their_stop() {
    assert_eq!(snippet_text("library($0)"), ("library()".into(), Some(8)));
    assert_eq!(
        snippet_text("ggplot2::ggplot($0)"),
        ("ggplot2::ggplot()".into(), Some(16))
    );
    assert_eq!(snippet_text("f($2, $1)"), ("f(, )".into(), Some(4)));
    assert_eq!(snippet_text("f(${1:x}, $0)"), ("f(x, )".into(), Some(5)));
    assert_eq!(
        snippet_text("f(${1:g(${2:y})})"),
        ("f(g(y))".into(), Some(2))
    );
    assert_eq!(snippet_text("f(${1|a,b|})"), ("f(a)".into(), Some(2)));
    assert_eq!(snippet_text("f(${1})"), ("f()".into(), Some(2)));
    assert_eq!(snippet_text("\\$x \\} \\\\"), ("$x } \\".into(), None));
    assert_eq!(snippet_text("a$ ${TM_FILENAME:n}$x"), ("a$ n".into(), None));
    assert_eq!(snippet_text("plain"), ("plain".into(), None));
    assert_eq!(snippet_text("名($0)"), ("名()".into(), Some(4)));
}
#[test]
fn calls_get_parentheses_with_the_cursor_inside() {
    let edit = |new_text: &str, kind, snippet: bool, line: &str| {
        let at = line.find('|').unwrap();
        let text = line.replace('|', "");
        let start = super::word_start(&text, at, false);
        let mut item = CompletionItem {
            label: new_text.into(),
            kind: Some(kind),
            insert_text_format: snippet.then_some(InsertTextFormat::SNIPPET),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(
                    Position::new(0, text[..start].chars().count() as u32),
                    Position::new(0, text[..at].chars().count() as u32),
                ),
                new_text: new_text.into(),
            })),
            ..Default::default()
        };
        let accept = finish(&mut item, &text, true);
        (
            super::inserted_text(&item).to_string(),
            accept.map(|accept| accept.cursor),
        )
    };
    let function = CompletionItemKind::FUNCTION;
    assert_eq!(
        edit("print", function, false, "pr|"),
        ("print()".into(), Some(6))
    );
    assert_eq!(
        edit("mean", CompletionItemKind::METHOD, false, "x = np.me|"),
        ("mean()".into(), Some(5))
    );
    // Not before an existing `(`, nor for other kinds.
    assert_eq!(
        edit("print", function, false, "pr|(x)"),
        ("print".into(), None)
    );
    assert_eq!(
        edit("DataFrame", CompletionItemKind::CLASS, false, "pd.Da|"),
        ("DataFrame".into(), None)
    );
    // Imports, definitions and decorators name functions without calling.
    assert_eq!(
        edit("path", function, false, "from os import pa|"),
        ("path".into(), None)
    );
    assert_eq!(
        edit("wraps", function, false, "@wr|"),
        ("wraps".into(), None)
    );
    // A snippet's own parentheses, dropped before an existing `(`.
    assert_eq!(
        edit("library($0)", function, true, "lib|"),
        ("library()".into(), Some(8))
    );
    assert_eq!(
        edit("library($0)", function, true, "lib|(x)"),
        ("library".into(), None)
    );
}
#[test]
fn kernel_unicode_offsets_are_scalar_not_wire_units() {
    let items = kernel_items(
        "名😀abc",
        CompletionReply {
            matches: vec!["abcdef".into()],
            cursor_start: 2,
            cursor_end: 5,
            metadata: Value::Null,
        },
        false,
    );
    let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
        panic!("expected edit");
    };
    assert_eq!(
        edit.range,
        Range::new(
            lsp_types::Position::new(0, 2),
            lsp_types::Position::new(0, 5)
        )
    );
}
#[test]
fn python_kernel_items_name_the_dotted_topic_they_complete() {
    let topic = |text: &str, start: usize, label: &str| {
        let items = kernel_items(
            text,
            CompletionReply {
                matches: vec![label.into()],
                cursor_start: start,
                cursor_end: text.chars().count(),
                metadata: Value::Null,
            },
            true,
        );
        items[0].data.as_ref().map(|data| data[HELP_TOPIC].clone())
    };
    assert_eq!(topic("x = sc.Ann", 7, "AnnData"), Some("sc.AnnData".into()));
    assert_eq!(
        topic("print(sc.pp.ne", 12, "neighbors"),
        Some("sc.pp.neighbors".into())
    );
    assert_eq!(topic("sc.Ann", 0, "sc.AnnData"), Some("sc.AnnData".into()));
    // After an index or call, there is no name to look up.
    assert_eq!(topic("x[0].sh", 5, "shape"), None);
}
#[test]
fn accepting_import_completion_preserves_all_edits() {
    let mut item = CompletionItem {
        label: "os".into(),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range: Range::new(
                lsp_types::Position::new(1, 0),
                lsp_types::Position::new(1, 2),
            ),
            new_text: "os".into(),
        })),
        additional_text_edits: Some(vec![TextEdit {
            range: Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            new_text: "import os\n".into(),
        }]),
        ..Default::default()
    };
    assert!(coalesce_edits(&mut item, "# comment\nos"));
    let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
        panic!("expected edit");
    };
    assert_eq!(edit.new_text, "import os\n# comment\nos");
    assert!(item.additional_text_edits.is_none());
}
