use super::*;

impl Notebook {
    /// Commit current cell values into the lossless source and rebase spans.
    fn rebase_quarto(&mut self, sources: &[String]) -> Result<()> {
        let text = self.render(sources)?;
        let Original::Text(original) = &self.original else {
            anyhow::bail!("a Quarto notebook view is required");
        };
        let mut delta = 0isize;
        for (cell, source) in self.cells.iter_mut().zip(sources) {
            let start = cell
                .span
                .start
                .checked_add_signed(delta)
                .context("cell offset")?;
            let extra = cell.kind == CellKind::Code
                && cell.span.is_empty()
                && !source.is_empty()
                && !original[cell.span.end..].starts_with(['\n', '\r']);
            delta += source.len() as isize - cell.span.len() as isize + isize::from(extra);
            cell.span = start..start + source.len();
            cell.source.clone_from(source);
        }
        self.original = Original::Text(text);
        Ok(())
    }

    fn refresh_quarto_ids(&mut self) -> Result<()> {
        let Original::Text(text) = &self.original else {
            anyhow::bail!("a Quarto notebook view is required");
        };
        let chunks = qmd::parse_chunks(text);
        self.language = chunks.first().map(|chunk| chunk.language.clone());
        let mut chunks = chunks.into_iter();
        for cell in &mut self.cells {
            cell.id = if cell.kind == CellKind::Code {
                chunks
                    .next()
                    .context("the chunk could not be found")?
                    .cell_id
            } else {
                format!("text-{}", cell.span.start)
            };
        }
        anyhow::ensure!(
            chunks.next().is_none(),
            "the document's chunk fences changed"
        );
        Ok(())
    }

    pub(crate) fn remove_quarto_cell(&mut self, ix: usize, sources: &[String]) -> Result<Cell> {
        self.rebase_quarto(sources)?;
        let cell = self.cells.get(ix).context("the cell could not be found")?;
        let Original::Text(text) = &self.original else {
            unreachable!()
        };
        let mut range = if cell.kind == CellKind::Code {
            let chunk = qmd::parse_chunks(text)
                .into_iter()
                .find(|chunk| chunk.code_from == cell.span.start)
                .context("the chunk's fences could not be found")?;
            chunk.from..chunk.to
        } else {
            cell.span.clone()
        };
        // Trimmed prose still owns indentation/trailing spaces on its lines.
        let line_start = text[..range.start].rfind('\n').map_or(0, |n| n + 1);
        if text[line_start..range.start].trim().is_empty() {
            range.start = line_start;
        }
        let tail = &text[range.end..];
        if let Some(n) = tail.find('\n').filter(|n| tail[..*n].trim().is_empty()) {
            range.end += n + 1;
        } else if tail.trim().is_empty() {
            range.end = text.len();
        }
        // Include one adjacent blank-line separator, preferring the next.
        let tail = &text[range.end..];
        if let Some(n) = tail.find('\n').filter(|n| tail[..*n].trim().is_empty()) {
            range.end += n + 1;
        } else if range.start > 0 {
            let prefix = &text[..range.start];
            let before = prefix
                .strip_suffix("\r\n")
                .or_else(|| prefix.strip_suffix('\n'));
            if let Some(before) = before {
                let from = before.rfind('\n').map_or(0, |n| n + 1);
                if before[from..].trim().is_empty() {
                    range.start = from;
                }
            }
        }
        // Empty prose cells inserted between chunks still own their position.
        if let Some(previous) = ix.checked_sub(1).and_then(|ix| self.cells.get(ix)) {
            range.start = range.start.max(previous.span.end);
        }
        if let Some(next) = self.cells.get(ix + 1) {
            range.end = range.end.min(next.span.start);
        }
        let removed = RemovedText {
            text: text[range.clone()].to_string(),
            offset: text.len() - range.end,
            next_gap: self
                .cells
                .get(ix + 1)
                .map(|next| next.span.start - range.end),
        };
        let mut cell = self.cells.remove(ix);
        cell.span = cell.span.start - range.start..cell.span.end - range.start;
        cell.removed = Some(removed);
        let len = range.len();
        for cell in &mut self.cells[ix..] {
            cell.span.start -= len;
            cell.span.end -= len;
        }
        let Original::Text(text) = &mut self.original else {
            unreachable!()
        };
        text.replace_range(range, "");
        self.refresh_quarto_ids()?;
        Ok(cell)
    }

    pub(crate) fn restore_quarto_cell(
        &mut self,
        at: usize,
        mut cell: Cell,
        sources: &[String],
    ) -> Result<()> {
        self.rebase_quarto(sources)?;
        let removed = cell
            .removed
            .take()
            .context("the deleted source could not be found")?;
        let at = at.min(self.cells.len());
        let Original::Text(text) = &mut self.original else {
            unreachable!()
        };
        let offset = match (self.cells.get(at), removed.next_gap) {
            (Some(next), Some(gap)) => {
                next.span.start.checked_sub(gap).context("restore offset")?
            }
            _ => text
                .len()
                .checked_sub(removed.offset)
                .context("restore offset")?,
        };
        text.insert_str(offset, &removed.text);
        for cell in &mut self.cells[at..] {
            cell.span.start += removed.text.len();
            cell.span.end += removed.text.len();
        }
        cell.span.start += offset;
        cell.span.end += offset;
        self.cells.insert(at, cell);
        self.refresh_quarto_ids()
    }

    pub(crate) fn as_quarto_source(&self, sources: &[String]) -> Result<Self> {
        let mut notebook = self.clone();
        notebook.rebase_quarto(sources)?;
        let text = notebook.render(sources)?;
        let mut source = parse_text(&self.path, &text, Format::Quarto);
        source.cells[0].kind = CellKind::Text;
        source.language.clone_from(&self.language);
        source.kernel.clone_from(&self.kernel);
        source.original = Original::Source(Box::new(notebook));
        Ok(source)
    }

    /// Return to cells, matching unchanged code/labels before positional edits.
    pub(crate) fn as_quarto_notebook(&self, text: &str) -> Result<(Self, Vec<(usize, usize)>)> {
        let Original::Source(previous) = &self.original else {
            anyhow::bail!("a Quarto source view is required");
        };
        let values: Vec<_> = previous
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect();
        if previous.render(&values)? == text {
            return Ok((
                (**previous).clone(),
                (0..previous.cells.len()).map(|ix| (ix, ix)).collect(),
            ));
        }
        let mut notebook = parse_quarto(&self.path, text);
        let mut matched = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (new, cell) in notebook.cells.iter_mut().enumerate() {
            let old = previous
                .cells
                .iter()
                .enumerate()
                .find(|(ix, old)| {
                    !used.contains(ix)
                        && old.kind == cell.kind
                        && ((old.json["label"].is_string()
                            && old.json["label"] == cell.json["label"])
                            || old.source == cell.source)
                })
                .map(|(ix, _)| ix);
            if let Some(old) = old {
                used.insert(old);
                cell.outputs.clone_from(&previous.cells[old].outputs);
                cell.execution_count = previous.cells[old].execution_count;
                matched.push((old, new));
            }
        }
        if previous.cells.len() == notebook.cells.len() {
            for ix in 0..notebook.cells.len() {
                if !used.contains(&ix)
                    && !matched.iter().any(|(_, new)| *new == ix)
                    && previous.cells[ix].kind == notebook.cells[ix].kind
                {
                    notebook.cells[ix]
                        .outputs
                        .clone_from(&previous.cells[ix].outputs);
                    notebook.cells[ix].execution_count = previous.cells[ix].execution_count;
                    matched.push((ix, ix));
                }
            }
        }
        Ok((notebook, matched))
    }

    pub(crate) fn cell_source_line(&self, ix: usize) -> usize {
        let Original::Text(text) = &self.original else {
            return 0;
        };
        self.cells
            .get(ix)
            .map_or(0, |cell| text[..cell.span.start].matches('\n').count())
    }

    /// Inserts into Quarto's original text, keeping every surrounding byte.
    /// Existing editor values are rebased first, without replacing their editors.
    /// Adding above YAML splits that prose cell after its header; returns the
    /// actual inserted index and whether the header gained a trailing prose cell.
    pub(crate) fn insert_quarto_cell(
        &mut self,
        at: usize,
        kind: CellKind,
        language: Option<&str>,
        sources: &[String],
    ) -> Result<(usize, bool)> {
        anyhow::ensure!(
            self.format == Format::Quarto,
            "a Quarto document is required"
        );
        let text = self.render(sources)?;
        let language = sandhill_daemon::cells::normalize_language(
            language.or(self.language.as_deref()).unwrap_or("python"),
        );
        anyhow::ensure!(
            kind == CellKind::Text || matches!(language.as_str(), "r" | "python"),
            "cells can be r or python, not {language}"
        );
        let chunks = qmd::parse_chunks(&text);
        anyhow::ensure!(
            chunks.len()
                == self
                    .cells
                    .iter()
                    .filter(|cell| cell.kind == CellKind::Code)
                    .count(),
            "the document's chunk fences changed; save and reopen before adding a cell"
        );
        if chunks.last().is_some_and(|chunk| !chunk.closed) {
            let last = self
                .cells
                .iter()
                .rposition(|cell| cell.kind == CellKind::Code)
                .context("the unclosed chunk could not be found")?;
            anyhow::ensure!(
                at <= last,
                "close the last chunk's fence before adding below it"
            );
        }
        let mut delta = 0isize;
        let Original::Text(original) = &self.original else {
            unreachable!("Quarto has original text");
        };
        for (cell, source) in self.cells.iter_mut().zip(sources) {
            let start = cell
                .span
                .start
                .checked_add_signed(delta)
                .context("cell offset")?;
            let extra_newline = cell.kind == CellKind::Code
                && cell.span.is_empty()
                && !source.is_empty()
                && !original[cell.span.end..].starts_with(['\n', '\r']);
            delta += source.len() as isize - cell.span.len() as isize + isize::from(extra_newline);
            cell.span = start..start + source.len();
            cell.source.clone_from(source);
        }
        let mut at = at.min(self.cells.len());
        let mut split = false;
        let mut after_header = None;
        if at == 0
            && let Some(end) = front_matter_end(&text)
            && let Some(first) = self.cells.first_mut()
            && first.kind == CellKind::Text
            && first.span.start == 0
            && first.span.end >= text[..end].trim_end_matches(['\n', '\r']).len()
        {
            let tail_end = first.span.end;
            let header_end = text[..end].trim_end_matches(['\n', '\r']).len();
            first.span.end = header_end;
            first.source = text[..header_end].to_string();
            let tail = &text[end.min(tail_end)..tail_end];
            if !tail.trim().is_empty() {
                let start = end + tail.len() - tail.trim_start().len();
                let mut cell = first.clone();
                cell.id = self.fresh_id();
                cell.span = start..tail_end;
                cell.source = text[start..tail_end].to_string();
                self.cells.insert(1, cell);
                split = true;
            }
            at = 1;
            after_header = Some(end);
        }
        let offset = if let Some(end) = after_header {
            end
        } else {
            match self.cells.get(at) {
                Some(cell) if cell.kind == CellKind::Code => chunks
                    .iter()
                    .find(|chunk| chunk.code_from == cell.span.start)
                    .map(|chunk| chunk.from)
                    .context("the next chunk could not be found")?,
                Some(cell) => cell.span.start,
                None => text.len(),
            }
        };
        let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let (body, source_offset) = if kind == CellKind::Code {
            // Use the daemon's fence, language and newline rules on an empty
            // document; placement here also supports prose and "above".
            let (chunk, _) = sandhill_daemon::cells::add_cell_to_text(
                std::path::Path::new(&self.path),
                "",
                "",
                Some(&language),
                None,
                &self.path,
            )?;
            let chunk = chunk.replace('\n', nl);
            let source_offset = chunk.find(nl).context("chunk opening fence")? + nl.len();
            (
                chunk.trim_end_matches(['\n', '\r']).to_string(),
                source_offset,
            )
        } else {
            (String::new(), 0)
        };
        // One blank line before the block, none doubled; a block added at the
        // end leaves the file ending in a single newline.
        let mut trailing = 0;
        let mut head = &text[..offset];
        while trailing < 2
            && let Some(rest) = head.strip_suffix(nl)
        {
            trailing += 1;
            head = rest;
        }
        let prefix = if offset == 0 {
            String::new()
        } else {
            nl.repeat(2 - trailing)
        };
        let suffix = if offset == text.len() {
            nl.to_string()
        } else {
            let mut leading = 0;
            let mut tail = &text[offset..];
            while leading < 2
                && let Some(rest) = tail.strip_prefix(nl)
            {
                leading += 1;
                tail = rest;
            }
            nl.repeat(2 - leading)
        };
        let insert = format!("{prefix}{body}{suffix}");
        let start = offset + prefix.len() + source_offset;
        for cell in &mut self.cells[at..] {
            cell.span.start += insert.len();
            cell.span.end += insert.len();
        }
        let mut text = text;
        text.insert_str(offset, &insert);
        let id = self.fresh_id();
        self.cells.insert(
            at,
            Cell {
                id,
                kind,
                source: String::new(),
                outputs: Vec::new(),
                execution_count: None,
                span: start..start,
                new_id: false,
                removed: None,
                json: if kind == CellKind::Code {
                    serde_json::json!({"language": language, "label": null})
                } else {
                    Value::Null
                },
            },
        );
        // Positional ids must agree with the daemon after an earlier insertion.
        let chunks = qmd::parse_chunks(&text);
        self.language = chunks.first().map(|chunk| chunk.language.clone());
        let mut chunks = chunks.into_iter();
        for cell in &mut self.cells {
            if cell.kind == CellKind::Code {
                cell.id = chunks
                    .next()
                    .context("the inserted chunk could not be found")?
                    .cell_id;
            }
        }
        self.original = Original::Text(text);
        Ok((at, split))
    }
}
