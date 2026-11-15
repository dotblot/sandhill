use super::{
    CellKind, Format, Notebook, format_of, is_binary, looks_tab_separated, parse, parse_as,
    pick_kernel, recover_kernel, shell_shebang,
};
use sandhill_proto::KernelspecInfo;
use serde_json::Value;

#[test]
fn txt_tables_are_found_by_consistent_tabs() {
    assert!(looks_tab_separated("gene\tcount\nCD3E\t12\nMS4A1\t7\n"));
    // A preview's last line may be cut mid-row.
    assert!(looks_tab_separated("a\tb\tc\n1\t2\t3\n4\t5"));
    assert!(!looks_tab_separated("one line\twith a tab\n"));
    assert!(!looks_tab_separated("a\tb\n1\t2\t3\n"));
    assert!(!looks_tab_separated("plain\nnotes\n"));
    let nb = parse("/p/counts.txt", "gene\tcount\nCD3E\t12\n").unwrap();
    assert_eq!(
        (nb.format, nb.language.as_deref()),
        (Format::Delimited, Some("tsv"))
    );
    let nb = parse("/p/notes.txt", "a, b\nc, d\n").unwrap();
    assert_eq!(nb.format, Format::Text);
}

#[test]
fn recolouring_changes_only_text_documents() {
    let text = parse("/p/data.txt", "a,b\n1,2\n")
        .unwrap()
        .recoloured("csv");
    assert_eq!(
        (text.format, text.language.as_deref()),
        (Format::Delimited, Some("csv"))
    );
    let back = text.recoloured("json");
    assert_eq!(
        (back.format, back.language.as_deref()),
        (Format::Text, Some("json"))
    );
    let script = parse("/p/run.py", "x = 1\n").unwrap().recoloured("csv");
    assert_eq!(script.format, Format::Script);
    let unknown = parse("/p/a.txt", "x\n").unwrap().recoloured("nonsense");
    assert_eq!(unknown.language.as_deref(), Some("text"));
}

#[test]
fn mixed_quarto_languages_and_labels_use_the_chunk_parser() {
    let nb = parse(
        "mixed.qmd",
        "# Report\n\n```{r setup}\nx <- 1\n```\n\n```{python}\n#| label: model\nx = 1\n```\n",
    )
    .unwrap();
    assert_eq!(
        nb.cells
            .iter()
            .enumerate()
            .map(|(ix, _)| nb.cell_language(ix))
            .collect::<Vec<_>>(),
        vec![Some("markdown"), Some("r"), Some("python")]
    );
    assert_eq!(nb.chunk_label(1), Some("setup"));
    assert_eq!(nb.chunk_label(2), Some("model"));
}

fn spec(name: &str, language: &str) -> KernelspecInfo {
    KernelspecInfo {
        name: name.into(),
        display_name: name.into(),
        language: language.into(),
        env: None,
    }
}

fn sources(nb: &Notebook) -> Vec<String> {
    nb.cells.iter().map(|c| c.source.clone()).collect()
}

#[test]
fn quarto_delete_restore_preserves_exact_bytes() {
    for extension in ["qmd", "Rmd"] {
        for nl in ["\n", "\r\n"] {
            let header = "---\ntitle: 'QC  '\njupyter: ir\n---\n\n";
            let first = "```{r first, echo=FALSE}\n#| warning: false\nx <- 1  \n```\n\n";
            let prose = "Between **chunks**.  \n\n";
            let last = "~~~{python}\n#| echo: false\nprint(2)\n~~~\n";
            let original = format!("{header}{first}{prose}{last}").replace('\n', nl);
            let path = format!("/p/report.{extension}");
            for (ix, removed) in [(1, first), (2, prose), (3, last)] {
                let mut nb = parse(&path, &original).unwrap();
                assert!(nb.can_delete_cells());
                assert!(!nb.can_restructure());
                let cell = nb.remove_quarto_cell(ix, &sources(&nb)).unwrap();
                let expected = if ix == 3 {
                    original.replace(&format!("{nl}{}", removed.replace('\n', nl)), "")
                } else {
                    original.replacen(&removed.replace('\n', nl), "", 1)
                };
                assert_eq!(nb.render(&sources(&nb)).unwrap(), expected);
                assert_eq!(
                    nb.cells
                        .iter()
                        .filter(|cell| cell.kind == CellKind::Code)
                        .map(|cell| cell.id.clone())
                        .collect::<Vec<_>>(),
                    sandhill_daemon::qmd::parse_chunks(&expected)
                        .into_iter()
                        .map(|chunk| chunk.cell_id)
                        .collect::<Vec<_>>(),
                );
                nb.restore_quarto_cell(ix, cell, &sources(&nb)).unwrap();
                assert_eq!(nb.render(&sources(&nb)).unwrap(), original);
            }
        }
    }
}

#[test]
fn quarto_delete_restore_keeps_unsaved_edits_and_multiple_deletions() {
    let original = "---\ntitle: QC\n---\n\n```{r}\nx <- 1\n```\n\n```{r}\nx <- 2\n```\n";
    let mut nb = parse("a.Rmd", original).unwrap();
    let mut text = sources(&nb);
    text[2] = "x <- 42".into();
    let first = nb.remove_quarto_cell(1, &text).unwrap();
    assert_eq!(nb.cells[1].id, "chunk-1");
    let second = nb.remove_quarto_cell(1, &sources(&nb)).unwrap();
    nb.restore_quarto_cell(1, second, &sources(&nb)).unwrap();
    let mut text = sources(&nb);
    text[1] = "x <- 100".into();
    nb.restore_quarto_cell(1, first, &text).unwrap();
    assert_eq!(
        nb.render(&sources(&nb)).unwrap(),
        original.replace("x <- 2", "x <- 100")
    );
    assert_eq!(nb.cells[2].id, "chunk-2");
}

#[test]
fn quarto_source_notebook_round_trip_is_lossless() {
    for extension in ["qmd", "Rmd"] {
        for nl in ["\n", "\r\n"] {
            for text in [
                "",
                "  Just prose.  \n\n",
                "---\ntitle: QC\n---\n\n```{r, echo=FALSE}\n#| label: setup\nx <- 1  \n```\n\n~~~{python}\nprint('λ')\n~~~",
            ] {
                let original = text.replace('\n', nl);
                let path = format!("a.{extension}");
                let nb = parse(&path, &original).unwrap();
                let source = nb.as_quarto_source(&sources(&nb)).unwrap();
                assert!(source.single_editor());
                assert!(!source.can_run());
                assert_eq!(source.render(&sources(&source)).unwrap(), original);
                let (restored, _) = source.as_quarto_notebook(&original).unwrap();
                assert_eq!(restored, nb);
                assert_eq!(restored.render(&sources(&restored)).unwrap(), original);
                let edited = format!("{original}{nl}New prose.  {nl}");
                let (restored, _) = source.as_quarto_notebook(&edited).unwrap();
                assert_eq!(restored.render(&sources(&restored)).unwrap(), edited);
            }
        }
    }
}

#[test]
fn quarto_view_round_trip_keeps_outputs_and_edited_cell_identity() {
    let mut nb = parse("a.qmd", "```{r}\nx <- 1\n```\n\n```{r}\nx <- 2\n```\n").unwrap();
    nb.cells[1]
        .outputs
        .push(serde_json::json!({"output_type": "stream", "text": "kept"}));
    let mut values = sources(&nb);
    values[1] = "x <- 42".into();
    let source = nb.as_quarto_source(&values).unwrap();
    let original = source.render(&sources(&source)).unwrap();
    let edited = format!("```{{r}}\nx <- 0\n```\n\n{original}");
    let (restored, matched) = source.as_quarto_notebook(&edited).unwrap();
    assert!(matched.contains(&(1, 2)));
    assert_eq!(restored.cells[2].outputs, nb.cells[1].outputs);
    assert_eq!(restored.render(&sources(&restored)).unwrap(), edited);
}

const IPYNB: &str = r##"{
  "metadata": {"kernelspec": {"name": "python3", "language": "python"}},
  "nbformat": 4, "nbformat_minor": 4,
  "cells": [
    {"cell_type": "markdown", "id": "a", "metadata": {}, "source": ["# Title\n", "text"]},
    {"cell_type": "code", "metadata": {}, "execution_count": 3, "source": "print(1)",
     "outputs": [{"output_type": "stream", "name": "stdout", "text": "1\n"}]},
    {"cell_type": "code", "id": "a", "metadata": {}, "source": "x = 2", "outputs": []}
  ]
}"##;

#[test]
fn reads_ipynb_cells_and_their_saved_outputs() {
    let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
    assert_eq!(nb.kernel.as_deref(), Some("python3"));
    assert_eq!(nb.language.as_deref(), Some("python"));
    let ids: Vec<_> = nb.cells.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["a", "cell-2", "cell-3"]);
    assert_eq!(nb.cells[0].source, "# Title\ntext");
    assert_eq!(nb.cells[0].kind, CellKind::Text);
    assert_eq!(nb.cells[1].kind, CellKind::Code);
    assert_eq!(nb.cells[1].outputs.len(), 1);
    assert_eq!(nb.cells[1].execution_count, Some(3));
    assert_eq!(nb.folder(), "/p");
    assert_eq!(nb.name(), "analysis.ipynb");
}

#[test]
fn writes_ipynb_in_nbformats_layout_with_new_ids() {
    let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
    let mut texts = sources(&nb);
    texts[2] = "x = 3\ny = 4".into();
    let out = nb.render(&texts).unwrap();
    assert!(out.ends_with("}\n"));
    assert!(
        out.contains("\n \"cells\": [\n"),
        "one-space indent:\n{out}"
    );
    let json: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(json["nbformat_minor"], 5);
    assert_eq!(json["cells"][1]["id"], "cell-2");
    assert_eq!(
        json["cells"][2]["source"],
        serde_json::json!(["x = 3\n", "y = 4"])
    );
    assert_eq!(json["cells"][1]["outputs"][0]["text"], "1\n");
    // Keys sorted, as nbformat writes them.
    let keys: Vec<_> = json["cells"][1]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    // Reading it back gives the same cells.
    let again = parse("/p/analysis.ipynb", &out).unwrap();
    assert_eq!(sources(&again), texts);
}

#[test]
fn does_not_save_run_counts() {
    let nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
    let mut json = nb.cells[1].json.clone();
    json["outputs"][0] = serde_json::json!({
        "output_type": "execute_result", "execution_count": 3,
        "data": {"text/plain": "2"}, "metadata": {}
    });
    let mut nb = nb;
    nb.cells[1].outputs = json["outputs"].as_array().unwrap().clone();
    let out: Value = serde_json::from_str(&nb.render(&sources(&nb)).unwrap()).unwrap();
    assert_eq!(out["cells"][1]["execution_count"], Value::Null);
    assert_eq!(
        out["cells"][1]["outputs"][0]["execution_count"],
        Value::Null
    );
    assert_eq!(out["cells"][1]["outputs"][0]["data"]["text/plain"], "2");
}

#[test]
fn adds_moves_changes_and_removes_ipynb_cells() {
    let mut nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
    assert!(nb.can_restructure());
    let added = nb.insert_cell(1, CellKind::Code);
    assert_eq!(nb.cells[1].id, added);
    assert_eq!(added.len(), 8);
    nb.move_cell(1, 3);
    assert_eq!(nb.cells[3].id, added);
    nb.set_kind(3, CellKind::Text);
    nb.set_kind(1, CellKind::Text);
    assert!(nb.cells[1].outputs.is_empty());
    nb.remove_cell(0);
    let mut texts = sources(&nb);
    texts[2] = "## Notes".into();
    let out: Value = serde_json::from_str(&nb.render(&texts).unwrap()).unwrap();
    let cells = out["cells"].as_array().unwrap();
    assert_eq!(cells.len(), 3);
    assert_eq!(cells[0]["cell_type"], "markdown");
    assert_eq!(cells[0]["id"], "cell-2");
    assert!(cells[0].get("outputs").is_none());
    assert!(cells[0].get("execution_count").is_none());
    assert_eq!(cells[1]["cell_type"], "code");
    assert_eq!(cells[2]["id"], added.as_str());
    assert_eq!(cells[2]["cell_type"], "markdown");
    assert_eq!(cells[2]["source"], serde_json::json!(["## Notes"]));
    // Read back, the cells are the same.
    let again = parse("/p/analysis.ipynb", &serde_json::to_string(&out).unwrap()).unwrap();
    assert_eq!(sources(&again), texts);
    // A Quarto document's cells stay where they are.
    assert!(!parse("/p/qc.qmd", QMD).unwrap().can_restructure());
}

#[test]
fn clears_a_cells_saved_outputs() {
    let mut nb = parse("/p/analysis.ipynb", IPYNB).unwrap();
    nb.clear_outputs(1);
    let out: Value = serde_json::from_str(&nb.render(&sources(&nb)).unwrap()).unwrap();
    assert_eq!(out["cells"][1]["outputs"], serde_json::json!([]));
}

#[test]
fn quarto_insertions_preserve_source_bytes_and_yaml() {
    for extension in ["qmd", "Rmd"] {
        for nl in ["\n", "\r\n"] {
            let header = "---\ntitle: \"QC  \"\njupyter: python3\n---\n".replace('\n', nl);
            let original = format!(
                "{header}{nl}Intro  é.{nl}{nl}```{{r setup, echo=FALSE}}{nl}#| warning: false{nl}x <- 1  {nl}```{nl}{nl}Tail  {nl}"
            );
            let offsets = [
                header.len(),
                original.find("```").unwrap(),
                original.find("Tail").unwrap(),
                original.len(),
            ];
            for (requested, offset) in offsets.into_iter().enumerate() {
                for kind in [CellKind::Code, CellKind::Text] {
                    let mut nb = parse(&format!("/p/qc.{extension}"), &original).unwrap();
                    assert!(nb.can_add_cells());
                    assert!(!nb.can_restructure());
                    let before = sources(&nb);
                    let (at, split) = nb
                        .insert_quarto_cell(requested, kind, Some("python"), &before)
                        .unwrap();
                    assert_eq!(split, requested == 0);
                    let mut texts = sources(&nb);
                    texts[at] = if kind == CellKind::Code {
                        "print(2)".into()
                    } else {
                        "**New text.**".into()
                    };
                    let block = if kind == CellKind::Code {
                        format!("```{{python}}{nl}print(2){nl}```")
                    } else {
                        "**New text.**".to_string()
                    };
                    // Exactly one blank line on each side; the file still
                    // ends in one newline when the block goes last.
                    let head = original[..offset].trim_end_matches(nl);
                    let tail = original[offset..].trim_start_matches(nl);
                    let expected = if tail.is_empty() {
                        format!("{head}{nl}{nl}{block}{nl}")
                    } else {
                        format!("{head}{nl}{nl}{block}{nl}{nl}{tail}")
                    };
                    assert_eq!(nb.render(&texts).unwrap(), expected);
                    assert_eq!(
                        nb.cells
                            .iter()
                            .filter(|cell| cell.kind == CellKind::Code)
                            .map(|cell| cell.id.as_str())
                            .collect::<Vec<_>>(),
                        sandhill_daemon::qmd::parse_chunks(&expected)
                            .iter()
                            .map(|chunk| chunk.cell_id.as_str())
                            .collect::<Vec<_>>(),
                    );
                }
            }
        }
    }
}

#[test]
fn quarto_insertions_choose_language_and_rebase_unsaved_edits() {
    for extension in ["qmd", "Rmd"] {
        for (original, selected, expected) in [
            ("", None, "python"),
            ("Just prose.", None, "python"),
            ("```{r}\nx <- 1\n```\n", None, "r"),
            ("```{python}\nx = 1\n```\n", Some("R"), "r"),
            ("```{r}\nx <- 1\n```\n", Some("python3"), "python"),
        ] {
            let mut nb = parse(&format!("/p/qc.{extension}"), original).unwrap();
            let (at, _) = nb
                .insert_quarto_cell(nb.cells.len(), CellKind::Code, selected, &sources(&nb))
                .unwrap();
            assert_eq!(nb.cell_language(at), Some(expected));
            let mut texts = sources(&nb);
            texts[at] = "value = 42".into();
            let before_second = nb.render(&texts).unwrap();
            let (at, _) = nb
                .insert_quarto_cell(nb.cells.len(), CellKind::Text, None, &texts)
                .unwrap();
            let mut texts = sources(&nb);
            texts[at] = "Notes".into();
            assert_eq!(
                nb.render(&texts).unwrap(),
                if before_second.is_empty() {
                    "Notes\n".to_string()
                } else {
                    format!("{}\n\nNotes\n", before_second.trim_end_matches('\n'))
                },
            );
        }
        let original = "---\ntitle: QC\n---\n";
        let mut nb = parse(&format!("/p/qc.{extension}"), original).unwrap();
        let (at, split) = nb
            .insert_quarto_cell(0, CellKind::Code, None, &sources(&nb))
            .unwrap();
        assert_eq!((at, split), (1, false));
        assert!(nb.render(&sources(&nb)).unwrap().starts_with(original));
    }
}

const QMD: &str = "---\ntitle: QC\n---\n\nSome text.\n\n```{r}\nx <- 1\n```\n\n```{r qc-plot}\nplot(x)\n```\n\n```{r}\n```\nEnd.\n";

#[test]
fn reads_quarto_chunks_with_the_daemons_ids() {
    let nb = parse("/p/qc.qmd", QMD).unwrap();
    let shown: Vec<_> = nb
        .cells
        .iter()
        .map(|c| (c.kind, c.id.as_str(), c.source.as_str()))
        .collect();
    assert_eq!(shown[1], (CellKind::Code, "chunk-1", "x <- 1"));
    assert_eq!(shown[2], (CellKind::Code, "qc-plot", "plot(x)"));
    assert_eq!(shown[3], (CellKind::Code, "chunk-3", ""));
    assert_eq!(shown[4].2, "End.");
    assert!(shown[0].2.contains("Some text."));
    assert_eq!(nb.language.as_deref(), Some("r"));
}

#[test]
fn writes_quarto_with_only_the_cells_changed() {
    let nb = parse("/p/qc.qmd", QMD).unwrap();
    assert_eq!(nb.render(&sources(&nb)).unwrap(), QMD);
    let mut texts = sources(&nb);
    texts[1] = "x <- 2\ny <- 3".into();
    texts[3] = "summary(x)".into();
    texts[4] = "The end.".into();
    let out = nb.render(&texts).unwrap();
    assert_eq!(
        out,
        QMD.replace("x <- 1", "x <- 2\ny <- 3")
            .replace("```{r}\n```", "```{r}\nsummary(x)\n```")
            .replace("End.", "The end.")
    );
}

#[test]
fn picks_the_named_kernel_then_the_language() {
    let specs = [spec("python3", "python"), spec("ark", "R")];
    let mut nb = parse("/p/a.qmd", "```{r}\n1\n```\n").unwrap();
    assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("ark"));
    nb.kernel = Some("python3".into());
    assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("python3"));
    nb.kernel = Some("missing".into());
    nb.language = None;
    assert_eq!(pick_kernel(&nb, &specs).as_deref(), Some("python3"));
}

#[test]
fn recovery_prefers_live_then_document_history_then_project() {
    let specs = [spec("python3", "python"), spec("ark", "R")];
    let nb = parse("/p/qc.py", "").unwrap();
    assert_eq!(
        recover_kernel(
            &nb,
            &specs,
            Some("live"),
            Some("remembered"),
            Some("project")
        )
        .as_deref(),
        Some("live")
    );
    assert_eq!(
        recover_kernel(
            &nb,
            &specs,
            None,
            Some("missing-remembered"),
            Some("python3")
        )
        .as_deref(),
        Some("missing-remembered")
    );
    assert_eq!(
        recover_kernel(&nb, &specs, None, None, Some("missing-project")).as_deref(),
        Some("missing-project")
    );
    assert_eq!(
        recover_kernel(&nb, &specs, None, None, None).as_deref(),
        Some("python3")
    );
    let markdown = parse("/p/readme.md", "").unwrap();
    assert_eq!(
        recover_kernel(
            &markdown,
            &specs,
            Some("live"),
            Some("remembered"),
            Some("project")
        ),
        None
    );
    let mut script = parse("/p/qc.R", "").unwrap();
    script.kernel = Some("python3".into());
    assert_eq!(pick_kernel(&script, &specs).as_deref(), Some("ark"));
}

#[test]
fn knows_which_files_are_notebooks() {
    assert_eq!(format_of("a/b.ipynb"), Some(Format::Ipynb));
    assert_eq!(format_of("b.Rmd"), Some(Format::Quarto));
    assert_eq!(format_of("b.R"), Some(Format::Script));
    assert_eq!(format_of("Makefile"), None);
}

#[test]
fn reads_and_writes_scripts_whole() {
    let nb = parse("/p/qc.py", "import os\nprint(1)\n").unwrap();
    assert_eq!(nb.format, Format::Script);
    assert_eq!(nb.language.as_deref(), Some("python"));
    assert_eq!(sources(&nb), ["import os\nprint(1)\n"]);
    assert_eq!(nb.render(&["x <- 1".into()]).unwrap(), "x <- 1");
    let r = parse("/p/qc.R", "").unwrap();
    assert_eq!(r.language.as_deref(), Some("r"));
    let python = KernelspecInfo {
        name: "python3".into(),
        display_name: "Python 3".into(),
        language: "python".into(),
        env: None,
    };
    assert_eq!(
        pick_kernel(&nb, std::slice::from_ref(&python)).as_deref(),
        Some("python3")
    );
    assert_eq!(pick_kernel(&r, &[python]), None);
}

#[test]
fn standalone_markdown_keeps_raw_source_and_has_no_kernel() {
    let specs = [spec("python3", "python"), spec("ark", "R")];
    for ext in ["md", "MARKDOWN", "Mdown", "mkd"] {
        let nb = parse(
            &format!("/p/readme.{ext}"),
            "# Hi\n<script>alert(1)</script>\n",
        )
        .unwrap();
        assert_eq!(nb.format, Format::Markdown);
        assert!(nb.single_editor());
        assert!(!nb.can_run());
        assert_eq!(nb.render(&["# Unsaved\n".into()]).unwrap(), "# Unsaved\n");
        assert_eq!(pick_kernel(&nb, &specs), None);
    }
    assert_eq!(parse("/p/README", "# Hi").unwrap().format, Format::Markdown);
    assert_eq!(format_of("/p/doc.mdx"), None);
    assert_eq!(format_of("/p/doc.qmd"), Some(Format::Quarto));
    assert_eq!(format_of("/p/doc.Rmd"), Some(Format::Quarto));
}

#[test]
fn shell_recognition_is_specific_and_never_picks_a_kernel() {
    let specs = [spec("python3", "python"), spec("ark", "R")];
    for first in [
        "#!/bin/sh",
        "#! /bin/bash -e",
        "#!/usr/bin/env zsh",
        "#!/usr/bin/env -S bash -eu",
        "#!/bin/dash",
        "#!/bin/ksh",
    ] {
        let text = format!("{first}\necho safe\n");
        let nb = parse("/p/launch", &text).unwrap();
        assert_eq!(nb.format, Format::Text);
        assert_eq!(nb.language.as_deref(), Some("bash"));
        assert!(!nb.can_run());
        assert_eq!(pick_kernel(&nb, &specs), None);
        assert_eq!(nb.render(std::slice::from_ref(&text)).unwrap(), text);
        let saved = parse_as("/p/launch", "echo edited", nb.format).unwrap();
        assert_eq!(
            saved.render(&["echo edited".into()]).unwrap(),
            "echo edited"
        );
    }
    for text in [
        "echo bash",
        "#!/usr/bin/env python",
        "#!/bin/bashful",
        "#!/bin/echo bash",
    ] {
        assert!(!shell_shebang(text));
        assert_eq!(
            parse("/p/launch", text).unwrap().language.as_deref(),
            Some("text")
        );
    }
    for ext in ["SH", "bash", "zsh"] {
        assert_eq!(
            parse(&format!("/p/run.{ext}"), "").unwrap().format,
            Format::Text
        );
    }
}

#[test]
fn text_files_open_with_their_highlighting_and_never_run() {
    let specs = [spec("python3", "python"), spec("ark", "R")];
    for (path, language) in [
        ("/p/data.json", "json"),
        ("/p/env.YML", "yaml"),
        ("/p/config.yaml", "yaml"),
        ("/p/pyproject.toml", "toml"),
        ("/p/.gitignore", "bash"),
        // A shebang does not make a .txt file a script.
        ("/p/notebook.txt", "text"),
    ] {
        let nb = parse(path, "#!/bin/bash\n").unwrap();
        assert_eq!(nb.format, Format::Text, "{path}");
        assert_eq!(nb.language.as_deref(), Some(language), "{path}");
        assert!(!nb.can_run());
        assert_eq!(pick_kernel(&nb, &specs), None);
    }
    assert_eq!(format_of("/p/.gitignore"), Some(Format::Text));
    assert_eq!(format_of("/p/Makefile"), None);
}

#[test]
fn single_editor_empty_files_save_without_adding_a_newline() {
    for path in ["/p/a.md", "/p/a.sh", "/p/a.csv", "/p/a.tsv", "/p/a.py"] {
        let nb = parse(path, "").unwrap();
        assert!(nb.single_editor());
        assert_eq!(
            nb.render(&["é,\"x\"\r\nnext".into()]).unwrap(),
            "é,\"x\"\r\nnext"
        );
        assert_eq!(nb.render(&["".into()]).unwrap(), "");
    }
}

#[test]
fn icon_text_types_have_lossless_non_runnable_groups() {
    for (language, paths) in [
        ("python", vec!["Snakefile", "pipeline.SMK"]),
        ("yaml", vec!["workflow.cwl"]),
        (
            "toml",
            vec![
                "Cargo.lock",
                "UV.LOCK",
                "a.ini",
                "a.cfg",
                "a.config",
                "nextflow.config",
            ],
        ),
        ("json", vec!["renv.lock"]),
        ("bash", vec![".gitattributes"]),
        ("fasta", vec!["a.fa", "a.fasta", "a.fna", "a.faa"]),
        ("fastq", vec!["a.fq", "a.fastq"]),
        (
            "bio-table",
            vec![
                "a.vcf",
                "a.bed",
                "a.bedgraph",
                "a.narrowpeak",
                "a.gff",
                "a.gff3",
                "a.gtf",
                "a.sam",
                "a.mtx",
                "a.paf",
                "a.bedpe",
                "a.interval_list",
            ],
        ),
        ("tree", vec!["a.nwk", "a.treefile", "a.nexus"]),
        ("nextflow", vec!["a.nf"]),
        ("wdl", vec!["a.wdl"]),
        ("bibtex", vec!["a.bib"]),
        (
            "text",
            vec!["LICENSE", "COPYING", "other.lock", "unknown.ext", "unknown"],
        ),
    ] {
        for path in paths {
            let nb = parse(path, "A\tα\r\n").unwrap();
            assert_eq!(nb.format, Format::Text, "{path}");
            assert_eq!(nb.language.as_deref(), Some(language), "{path}");
            assert!(!nb.can_run(), "{path}");
            assert_eq!(nb.render(&["new\té\r\n".into()]).unwrap(), "new\té\r\n");
        }
    }
    for path in ["AGENTS.MD", "CLAUDE.MD", "Readme", "README.notes"] {
        assert_eq!(format_of(path), Some(Format::Markdown), "{path}");
    }
    for ext in [
        "bam", "cram", "bcf", "h5", "h5ad", "h5seurat", "hdf5", "loom", "parquet", "rds", "rda",
        "rdata", "zip", "tar", "tgz", "gz", "bgz", "bz2", "xz", "zst",
    ] {
        let path = format!("reads.FASTQ.{}", ext.to_uppercase());
        assert!(is_binary(&path), "{path}");
        assert_eq!(format_of(&path), None, "{path}");
        assert!(parse(&path, "not really text").is_err(), "{path}");
    }
}

#[test]
fn truncated_preview_can_never_render_for_saving() {
    let mut nb = parse("large.vcf", "#CHROM\nchr1\t2\n").unwrap();
    nb.truncated = true;
    assert!(nb.render(&["replacement".into()]).is_err());
}
