//! Cell-local UTF-8 bytes, wire UTF-16 positions, and component Unicode columns.
use lsp_types::{Position, Range};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Clone, Debug)]
pub(crate) struct CellSource {
    pub cell: usize,
    pub text: String,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Document {
    pub path: String,
    pub uri: String,
    pub language: String,
    pub kind: &'static str,
    pub session: Option<String>,
    pub text: String,
    pub cells: Vec<CellSource>,
    pub alive: Arc<AtomicBool>,
    pub revision: u64,
}

impl Document {
    pub fn text_hash(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        self.text.hash(&mut hash);
        hash.finish()
    }

    pub fn new(
        path: &str,
        language: &str,
        script: bool,
        session: Option<String>,
        sources: impl IntoIterator<Item = (usize, String)>,
    ) -> Self {
        let virtual_path = if script {
            path.to_string()
        } else {
            format!(
                "{path}.sandhill.{language}.{}",
                if language == "r" { "R" } else { "py" }
            )
        };
        let mut text = String::new();
        let mut cells = Vec::new();
        let mut line = 0;
        for (cell, source) in sources {
            if !cells.is_empty() {
                text.push_str("\n\n");
                line += 2;
            }
            let start_line = line;
            line += source.bytes().filter(|b| *b == b'\n').count() as u32;
            text.push_str(&source);
            cells.push(CellSource {
                cell,
                text: source,
                line: start_line,
            });
        }
        Self {
            path: path.into(),
            uri: file_uri(&virtual_path),
            language: language.into(),
            kind: if script { "script" } else { "notebook" },
            session,
            text,
            cells,
            alive: Arc::new(AtomicBool::new(true)),
            revision: 0,
        }
    }

    pub fn position(&self, cell: usize, byte: usize) -> Option<Position> {
        let source = self.cells.iter().find(|source| source.cell == cell)?;
        let mut position = byte_position(&source.text, byte);
        position.line += source.line;
        Some(position)
    }

    pub fn local(&self, position: Position) -> Option<(usize, usize)> {
        self.cells.iter().find_map(|source| {
            let end = byte_position(&source.text, source.text.len());
            let local = Position::new(position.line.checked_sub(source.line)?, position.character);
            (local <= end).then(|| (source.cell, position_byte(&source.text, local)))
        })
    }

    /// Split server ranges at cell boundaries, never painting prose or separators.
    pub fn split_range(&self, range: Range) -> Vec<(usize, Range)> {
        self.cells
            .iter()
            .filter_map(|source| {
                let start = Position::new(source.line, 0);
                let mut end = byte_position(&source.text, source.text.len());
                end.line += source.line;
                let first = range.start.max(start);
                let last = range.end.min(end);
                if first > last || (first == last && range.start != range.end) {
                    return None;
                }
                let local = |p: Position| {
                    let p = Position::new(p.line - source.line, p.character);
                    component_position(&source.text, position_byte(&source.text, p))
                };
                Some((source.cell, Range::new(local(first), local(last))))
            })
            .collect()
    }

    pub fn component_range(&self, cell: usize, range: Range) -> Option<Range> {
        let (first, start) = self.local(range.start)?;
        let (last, end) = self.local(range.end)?;
        if first != cell || last != cell {
            return None;
        }
        let source = self.cells.iter().find(|source| source.cell == cell)?;
        Some(Range::new(
            component_position(&source.text, start),
            component_position(&source.text, end),
        ))
    }
}

pub fn byte_position(text: &str, byte: usize) -> Position {
    let byte = text.floor_char_boundary(byte.min(text.len()));
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() as u32;
    Position::new(
        line,
        prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count() as u32,
    )
}

pub fn position_byte(text: &str, position: Position) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(position.line as usize)
        .map(str::len)
        .sum::<usize>();
    let row = text[start..].split('\n').next().unwrap_or("");
    let mut units = 0;
    for (byte, ch) in row.char_indices() {
        if units + ch.len_utf16() > position.character as usize {
            return start + byte;
        }
        units += ch.len_utf16();
    }
    start + row.len()
}

pub fn component_position(text: &str, byte: usize) -> Position {
    let byte = text.floor_char_boundary(byte.min(text.len()));
    let prefix = &text[..byte];
    Position::new(
        prefix.bytes().filter(|b| *b == b'\n').count() as u32,
        prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32,
    )
}

pub fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~:".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

pub fn uri_path(uri: &str) -> Option<String> {
    let raw = uri.strip_prefix("file://")?;
    if !raw.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw.as_bytes()[i] == b'%' {
            bytes.push(u8::from_str_radix(raw.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw.as_bytes()[i]);
            i += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::{Document, byte_position, component_position, position_byte, uri_path};
    use lsp_types::{Position, Range};
    #[test]
    fn mixed_cells_and_stable_uri() {
        let doc = Document::new(
            "/project/a b.qmd",
            "python",
            false,
            None,
            [(1, "x = 1\ny = 2".into()), (4, "print(x)".into())],
        );
        assert_eq!(doc.text, "x = 1\ny = 2\n\nprint(x)");
        assert_eq!(doc.position(4, 3), Some(Position::new(3, 3)));
        assert_eq!(doc.local(Position::new(2, 0)), None);
        assert_eq!(doc.local(Position::new(3, 2)), Some((4, 2)));
        assert_eq!(doc.uri, "file:///project/a%20b.qmd.sandhill.python.py");
        assert_eq!(
            uri_path(&doc.uri).unwrap(),
            "/project/a b.qmd.sandhill.python.py"
        );
    }
    #[test]
    fn parsed_mixed_quarto_excludes_other_languages_and_prose() {
        let notebook = crate::editor::notebook::parse("/p/mixed.qmd", "# Report\n\n```{r}\nx <- 1\n```\n\n```{python}\nx = '名😀'\n```\n\n```{r}\ny <- x\n```\n").unwrap();
        let build = |language: &str| {
            Document::new(
                &notebook.path,
                language,
                false,
                None,
                notebook
                    .cells
                    .iter()
                    .enumerate()
                    .filter(|(ix, _)| notebook.cell_language(*ix) == Some(language))
                    .map(|(ix, cell)| (ix, cell.source.clone())),
            )
        };
        let r = build("r");
        let python = build("python");
        assert_eq!(r.cells.len(), 2);
        assert_eq!(python.cells.len(), 1);
        assert!(!r.text.contains("Report"));
        assert!(!r.text.contains("名"));
        assert!(python.text.contains("名😀"));
        let second = &r.cells[1];
        assert_eq!(
            r.local(Position::new(second.line, 0)),
            Some((second.cell, 0))
        );
    }
    fn quarto_document(text: &str, language: &str) -> Document {
        let notebook = crate::editor::notebook::parse("/p/report.qmd", text).unwrap();
        Document::new(
            &notebook.path,
            language,
            false,
            None,
            notebook
                .cells
                .iter()
                .enumerate()
                .filter(|(ix, _)| notebook.cell_language(*ix) == Some(language))
                .map(|(ix, cell)| (ix, cell.source.clone())),
        )
    }
    fn new_quarto_python_document(language: Option<&str>) -> Document {
        use crate::editor::notebook::{CellKind, parse};

        let mut notebook = parse(
            "/p/report.qmd",
            &crate::files::template("report.qmd", language),
        )
        .unwrap();
        let mut sources: Vec<_> = notebook
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        let first = notebook
            .cells
            .iter()
            .position(|cell| cell.kind == CellKind::Code)
            .unwrap_or_else(|| {
                notebook
                    .insert_quarto_cell(
                        notebook.cells.len(),
                        CellKind::Code,
                        Some("python"),
                        &sources,
                    )
                    .unwrap()
                    .0
            });
        assert_eq!(notebook.cell_language(first), Some("python"));
        sources = notebook
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        sources[first] = "import pandas as pd\n\ndf = pd.DataFrame({'value': [1]})".into();
        let (second, _) = notebook
            .insert_quarto_cell(
                notebook.cells.len(),
                CellKind::Code,
                Some("python"),
                &sources,
            )
            .unwrap();
        sources = notebook
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        sources[second] = "print(df.shape)\nprint(sandhill_missing)".into();
        Document::new(
            &notebook.path,
            "python",
            false,
            None,
            sources
                .into_iter()
                .enumerate()
                .filter(|(ix, _)| notebook.cell_language(*ix) == Some("python")),
        )
    }

    #[test]
    fn quarto_new_document_and_late_kernel_include_all_python_cells() {
        for language in [Some("python"), None] {
            let document = new_quarto_python_document(language);
            assert_eq!(document.cells.len(), 2);
            assert_eq!(
                document.text,
                "import pandas as pd\n\ndf = pd.DataFrame({'value': [1]})\n\nprint(df.shape)\nprint(sandhill_missing)",
            );
        }
        for header in [
            "{python}",
            "{PYTHON}",
            "{python data, echo=FALSE}",
            "{python label=\"data\"}",
        ] {
            let document = quarto_document(
                &format!(
                    "```{header}\nimport pandas as pd\ndf = pd.DataFrame()\n```\n\n```{{python}}\nprint(df.shape)\n```\n"
                ),
                "python",
            );
            assert_eq!(document.cells.len(), 2, "{header}");
            assert!(document.text.starts_with("import pandas as pd"), "{header}");
        }
    }

    const QUARTO: &str = "---\ntitle: Report\njupyter: python3\n---\n\n# Analysis\n\n```{r setup}\n#| label: setup\nx <- 1\n```\n\nProse, not Python.\n\n```{python data}\n#| echo: false\nimport pandas as pd\ndf = pd.DataFrame({\n    'name': ['名😀'],\n    'value': [1],\n})\n```\n\n```{r}\ny <- x + 1\n```\n\n```{python shape}\nprint(df.shape)\nprint(sandhill_missing)\n```\n";

    #[test]
    fn quarto_yaml_options_mixed_chunks_and_crlf_map_exactly() {
        for text in [QUARTO.to_string(), QUARTO.replace('\n', "\r\n")] {
            let python = quarto_document(&text, "python");
            let r = quarto_document(&text, "r");
            assert_eq!(
                python
                    .cells
                    .iter()
                    .map(|cell| cell.cell)
                    .collect::<Vec<_>>(),
                vec![3, 5]
            );
            assert_eq!(
                r.cells.iter().map(|cell| cell.cell).collect::<Vec<_>>(),
                vec![1, 4]
            );
            assert_eq!(python.cells[0].line, 0);
            assert_eq!(python.cells[1].line, 7);
            assert_eq!(r.cells[1].line, 3);
            assert!(
                python.text.starts_with("#| echo: false\nimport")
                    || python.text.starts_with("#| echo: false\r\nimport")
            );
            assert!(!python.text.contains("Report"));
            assert!(!python.text.contains("Prose"));
            assert!(!python.text.contains("```"));
            assert!(!python.text.contains("x <-"));
            for cell in python.cells.iter().chain(&r.cells) {
                let doc = if python.cells.iter().any(|source| source.cell == cell.cell) {
                    &python
                } else {
                    &r
                };
                for (byte, ch) in cell.text.char_indices() {
                    if ch == '\r' || ch == '\n' {
                        continue;
                    }
                    let position = doc.position(cell.cell, byte).unwrap();
                    assert_eq!(doc.local(position), Some((cell.cell, byte)));
                }
            }
            assert_eq!(python.local(Position::new(6, 0)), None);
            let missing = Range::new(Position::new(8, 6), Position::new(8, 22));
            assert_eq!(
                python.split_range(missing),
                vec![(5, Range::new(Position::new(1, 6), Position::new(1, 22)))],
            );
            assert_eq!(
                python.component_range(5, missing),
                Some(Range::new(Position::new(1, 6), Position::new(1, 22))),
            );
            let ipynb = Document::new(
                "/p/report.ipynb",
                "python",
                false,
                None,
                python
                    .cells
                    .iter()
                    .map(|cell| (cell.cell, cell.text.clone())),
            );
            assert_eq!(python.text, ipynb.text);
            assert_eq!(python.split_range(missing), ipynb.split_range(missing));
        }
    }

    #[tokio::test]
    async fn quarto_python_real_servers_share_definitions_and_map_diagnostics() {
        assert_python_definitions(quarto_document(&QUARTO.replace('\n', "\r\n"), "python")).await;
    }

    #[tokio::test]
    async fn quarto_new_document_python_real_servers_share_definitions() {
        for language in [Some("python"), None] {
            assert_python_definitions(new_quarto_python_document(language)).await;
        }
    }

    async fn assert_python_definitions(mut doc: Document) {
        use sandhill_daemon::{Daemon, StateDir};
        use sandhill_proto::client::Client;
        use serde_json::json;
        use std::time::Duration;

        let project = tempfile::tempdir().unwrap();
        for name in ["ty", "ruff"] {
            if sandhill_daemon::lsp::find_binary(project.path(), None, name).is_none() {
                eprintln!("skipping Quarto Python language features: {name} is not installed");
                return;
            }
        }
        let state = tempfile::tempdir().unwrap();
        let daemon = Daemon::open(StateDir::new(state.path())).await.unwrap();
        let (socket, peer) = tokio::io::duplex(1024 * 1024);
        let (reader, writer) = tokio::io::split(peer);
        let server = tokio::spawn(daemon.clone().serve_connection(reader, writer));
        let (reader, writer) = tokio::io::split(socket);
        let mut client = Client::new(reader, writer);
        let mut messages = client.take_lsp_messages().unwrap();
        let host = client
            .lsp_start(
                project.path().to_str().unwrap(),
                "python",
                Some("notebook".into()),
                None,
            )
            .await
            .unwrap();
        let second = doc.cells.last().unwrap().cell;
        doc.uri = super::file_uri(
            project
                .path()
                .join("report.qmd.sandhill.python.py")
                .to_str()
                .unwrap(),
        );
        client.lsp_send(&host.lsp, json!({
            "jsonrpc":"2.0", "method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":doc.uri,"languageId":"python","version":1,"text":doc.text}}
        })).await.unwrap();
        let diagnostics = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let message = messages.recv().await.unwrap().message;
                assert_ne!(message["method"], "sandhill/lspExited", "{message}");
                if message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == doc.uri
                    && message["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                {
                    break message;
                }
            }
        })
        .await
        .expect("Ruff did not publish diagnostics");
        let marks: Vec<lsp_types::Diagnostic> =
            serde_json::from_value(diagnostics["params"]["diagnostics"].clone()).unwrap();
        assert_eq!(marks.len(), 1, "{diagnostics}");
        assert!(
            marks[0].message.contains("sandhill_missing"),
            "{diagnostics}"
        );
        assert_eq!(marks[0].source.as_deref(), Some("Ruff"));
        assert_eq!(
            doc.split_range(marks[0].range),
            vec![(
                second,
                Range::new(Position::new(1, 6), Position::new(1, 22))
            )],
        );
        // ty's hover on the later reference must resolve the earlier cell's df.
        client
            .lsp_send(
                &host.lsp,
                json!({
                    "jsonrpc":"2.0", "id":1, "method":"textDocument/hover",
                    "params":{"textDocument":{"uri":doc.uri},"position":doc.position(second, 6).unwrap()}
                }),
            )
            .await
            .unwrap();
        let hover = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let message = messages.recv().await.unwrap().message;
                if message["id"] == 1 {
                    break message;
                }
            }
        })
        .await
        .expect("ty did not answer hover");
        assert!(hover["error"].is_null(), "{hover}");
        assert!(!hover["result"].is_null(), "{hover}");
        if let Ok(range) = serde_json::from_value::<Range>(hover["result"]["range"].clone()) {
            assert_eq!(
                doc.component_range(second, range).unwrap().start,
                Position::new(0, 6)
            );
        }
        client.lsp_stop(&host.lsp).await.unwrap();
        daemon.detach_all();
        server.abort();
    }

    #[tokio::test]
    async fn quarto_r_real_ark_maps_diagnostics_and_completes_across_chunks() {
        use sandhill_daemon::{Daemon, StateDir, kernel};
        use sandhill_proto::{Call, SessionInfo, client::Client};
        use serde_json::json;
        use std::time::Duration;

        let mut ark = None;
        for info in kernel::list_kernelspecs(None).await {
            if let Ok(spec) = kernel::find_kernelspec(&info.name, None).await
                && spec.kernelspec.language.eq_ignore_ascii_case("r")
                && spec.kernelspec.argv.first().is_some_and(|program| {
                    std::path::Path::new(program)
                        .file_name()
                        .is_some_and(|name| name == "ark")
                })
            {
                ark = Some(info.name);
                break;
            }
        }
        let Some(ark) = ark else {
            eprintln!("skipping Quarto R language features: Ark is not installed");
            return;
        };
        let project = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let daemon = Daemon::open(StateDir::new(state.path())).await.unwrap();
        let (socket, peer) = tokio::io::duplex(1024 * 1024);
        let (reader, writer) = tokio::io::split(peer);
        let server = tokio::spawn(daemon.clone().serve_connection(reader, writer));
        let (reader, writer) = tokio::io::split(socket);
        let mut client = Client::new(reader, writer);
        let mut messages = client.take_lsp_messages().unwrap();
        let root = project.path().to_str().unwrap();
        let session: SessionInfo = client
            .call(Call::StartSession {
                kernel: ark,
                cwd: Some(root.into()),
                project: Some(root.into()),
            })
            .await
            .unwrap();
        client.subscribe(&session.id, None).await.unwrap();
        let run = client.execute(&session.id, "1 + 1", None).await.unwrap();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let event = client.next_event().await.unwrap();
                if event.run.as_deref() == Some(&run.run)
                    && event.msg_type == sandhill_proto::EVENT_RUN_FINISHED
                {
                    assert_eq!(event.content["status"], "ok");
                    break;
                }
            }
        })
        .await
        .expect("Ark startup did not become idle");
        let host = client
            .lsp_start(root, "r", Some("notebook".into()), Some(session.id.clone()))
            .await
            .unwrap();
        let text = format!(
            "{}\n```{{r}}\n#| label: invalid\nbroken <- )\n```\n",
            QUARTO
                .replace("x <- 1", "sandhill_qmd_value <- 1")
                .replace("y <- x + 1", "sandhill_qmd_va")
        );
        let mut doc = quarto_document(&text.replace('\n', "\r\n"), "r");
        doc.uri = super::file_uri(
            project
                .path()
                .join("report.qmd.sandhill.r.R")
                .to_str()
                .unwrap(),
        );
        client.lsp_send(&host.lsp, json!({
            "jsonrpc":"2.0", "method":"textDocument/didOpen",
            "params":{"textDocument":{"uri":doc.uri,"languageId":"r","version":1,"text":doc.text}}
        })).await.unwrap();
        client.lsp_send(&host.lsp, json!({
            "jsonrpc":"2.0", "id":1, "method":"textDocument/completion",
            "params":{"textDocument":{"uri":doc.uri},"position":doc.position(4, "sandhill_qmd_va".len()).unwrap()}
        })).await.unwrap();
        let (completion, diagnostics) = tokio::time::timeout(Duration::from_secs(30), async {
            let mut completion = None;
            let mut diagnostics = None;
            loop {
                let message = messages.recv().await.unwrap().message;
                assert_ne!(message["method"], "sandhill/lspExited", "{message}");
                if message["id"] == 1 {
                    completion = Some(message.clone());
                }
                if message["method"] == "textDocument/publishDiagnostics"
                    && message["params"]["uri"] == doc.uri
                    && message["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                {
                    diagnostics = Some(message);
                }
                if let (Some(completion), Some(diagnostics)) = (&completion, &diagnostics) {
                    break (completion.clone(), diagnostics.clone());
                }
            }
        })
        .await
        .expect("Ark did not answer completion and diagnostics");
        client.lsp_stop(&host.lsp).await.unwrap();
        client.shutdown(&session.id).await.unwrap();
        daemon.detach_all();
        server.abort();
        let items = completion["result"]
            .as_array()
            .or_else(|| completion["result"]["items"].as_array())
            .unwrap();
        assert!(
            items.iter().any(|item| item["label"]
                .as_str()
                .is_some_and(|label| label.starts_with("sandhill_qmd_value"))),
            "{completion}"
        );
        let marks: Vec<lsp_types::Diagnostic> =
            serde_json::from_value(diagnostics["params"]["diagnostics"].clone()).unwrap();
        let errors: Vec<_> = marks
            .iter()
            .filter(|mark| {
                mark.severity
                    .is_none_or(|severity| severity == lsp_types::DiagnosticSeverity::ERROR)
            })
            .flat_map(|mark| doc.split_range(mark.range))
            .collect();
        assert!(!errors.is_empty(), "{diagnostics}");
        assert!(
            errors
                .iter()
                .all(|(cell, range)| *cell == 6 && range.start.line == 1),
            "{diagnostics}: {errors:?}"
        );
    }
    #[test]
    fn utf16_cjk_and_emoji() {
        let text = "名😀x\n列";
        assert_eq!(byte_position(text, "名😀".len()), Position::new(0, 3));
        assert_eq!(position_byte(text, Position::new(0, 3)), "名😀".len());
        assert_eq!(position_byte(text, Position::new(0, 2)), "名".len());
        assert_eq!(component_position(text, "名😀".len()), Position::new(0, 2));
        assert_eq!(position_byte(text, Position::new(20, 9)), text.len());
    }
    #[test]
    fn diagnostics_split_across_cells() {
        let doc = Document::new(
            "/a.ipynb",
            "r",
            false,
            None,
            [(0, "名😀x".into()), (2, "bad()".into())],
        );
        assert_eq!(
            doc.split_range(Range::new(Position::new(0, 1), Position::new(2, 3))),
            vec![
                (0, Range::new(Position::new(0, 1), Position::new(0, 3))),
                (2, Range::new(Position::new(0, 0), Position::new(0, 3)))
            ]
        );
        assert_eq!(
            Document::new("/a.R", "r", true, None, [(0, "x".into())]).uri,
            "file:///a.R"
        );
    }
}
