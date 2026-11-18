use std::collections::{HashMap, HashSet};

use sandhill_proto::{NotebookHistoryCell, notebook_readable_text};

use super::*;

impl Notebook {
    /// Current editor text in the same form the daemon stores.
    pub(crate) fn history_text(&self, sources: &[String]) -> Result<String> {
        if self.format != Format::Ipynb {
            return self.render(sources);
        }
        let cells: Vec<_> = self
            .cells
            .iter()
            .zip(sources)
            .map(|(cell, source)| NotebookHistoryCell {
                id: Some(cell.id.clone()),
                cell_type: cell_type(cell).into(),
                source: source.split_inclusive('\n').map(str::to_owned).collect(),
            })
            .collect();
        Ok(serde_json::to_string_pretty(&cells)?)
    }

    pub(crate) fn history_comparison_text(&self, sources: &[String]) -> Result<String> {
        if self.format == Format::Ipynb {
            Ok(notebook_readable_text(self.cells.iter().zip(sources).map(
                |(cell, source)| (cell_type(cell), std::iter::once(source.as_str())),
            )))
        } else {
            self.render(sources)
        }
    }

    /// Replace cell order, kinds and sources, retaining only outputs whose
    /// cell and code still match. Matching ids keep a cell's current metadata.
    pub(crate) fn history_edit(
        &self,
        sources: &[String],
        version: &str,
    ) -> Result<(Self, Vec<(usize, usize)>)> {
        if self.format == Format::Quarto {
            let mut restored = parse_quarto(&self.path, version);
            let mut by_id: HashMap<_, _> = self
                .cells
                .iter()
                .enumerate()
                .map(|(i, c)| (c.id.as_str(), i))
                .collect();
            let mut matched = Vec::new();
            for new in 0..restored.cells.len() {
                let cell = &restored.cells[new];
                if let Some(old) = by_id.remove(cell.id.as_str())
                    && self.cells[old].kind == cell.kind
                    && sources[old] == cell.source
                    && self.cell_language(old) == restored.cell_language(new)
                {
                    let old_cell = &self.cells[old];
                    restored.cells[new].outputs.clone_from(&old_cell.outputs);
                    restored.cells[new].execution_count = old_cell.execution_count;
                    matched.push((old, new));
                }
            }
            return Ok((restored, matched));
        }

        let Original::Json(original) = &self.original else {
            anyhow::bail!("File history cannot restore this notebook format");
        };
        let version: Vec<NotebookHistoryCell> = serde_json::from_str(version)?;
        let by_id: HashMap<_, _> = self
            .cells
            .iter()
            .enumerate()
            .map(|(i, cell)| (cell.id.as_str(), i))
            .collect();
        let mut seen = HashSet::with_capacity(version.len());
        let mut matched = Vec::new();
        let mut cells = Vec::with_capacity(version.len());
        for (idx, cell) in version.into_iter().enumerate() {
            let id = cell
                .id
                .filter(|id| !id.is_empty() && !seen.contains(id))
                .unwrap_or_else(|| sandhill_proto::notebook_cell_id(idx, &seen));
            seen.insert(id.clone());
            let kind = match cell.cell_type.as_str() {
                "code" => CellKind::Code,
                "markdown" | "raw" => CellKind::Text,
                _ => anyhow::bail!("Unknown notebook cell type: {}", cell.cell_type),
            };
            let source = cell.source.concat();
            let old = by_id.get(id.as_str()).copied();
            let (mut json, new_id) = old.map_or_else(
                || (serde_json::json!({"metadata": {}}), true),
                |i| (self.cells[i].json.clone(), self.cells[i].new_id),
            );
            let unchanged = old
                .filter(|&i| cell_type(&self.cells[i]) == cell.cell_type && sources[i] == source);
            let (outputs, execution_count) = if let Some(i) = unchanged {
                matched.push((i, idx));
                (self.cells[i].outputs.clone(), self.cells[i].execution_count)
            } else {
                set_cell_type(&mut json, kind);
                (Vec::new(), None)
            };
            json["cell_type"] = Value::String(cell.cell_type);
            cells.push(Cell {
                id,
                kind,
                source,
                outputs,
                execution_count,
                span: 0..0,
                new_id,
                json,
                removed: None,
            });
        }
        let metadata = original
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, _)| *key != "cells")
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok((
            Notebook {
                path: self.path.clone(),
                format: self.format,
                truncated: false,
                cells,
                language: self.language.clone(),
                kernel: self.kernel.clone(),
                original: Original::Json(Value::Object(metadata)),
            },
            matched,
        ))
    }
}

fn cell_type(cell: &Cell) -> &str {
    cell.json["cell_type"].as_str().unwrap_or(match cell.kind {
        CellKind::Code => "code",
        CellKind::Text => "markdown",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notebook_history_restores_order_and_sources_without_resurrecting_stale_outputs() {
        let nb = parse_ipynb(
            "/project/atlas.ipynb",
            r#"{"cells":[
            {"id":"a","cell_type":"code","source":"current()","outputs":[{"text":"stale"}]},
            {"id":"b","cell_type":"code","source":"keep()","outputs":[{"text":"keep"}]}
        ],"nbformat":4,"nbformat_minor":5}"#,
        )
        .unwrap();
        let version = r#"[
            {"id":"b","cell_type":"code","source":["keep()"]},
            {"id":"a","cell_type":"code","source":["earlier()"]},
            {"id":"memo","cell_type":"markdown","source":["Notes\n"]}
        ]"#;
        let (restored, matched) = nb
            .history_edit(&["current()".into(), "keep()".into()], version)
            .unwrap();
        assert_eq!(matched, [(1, 0)]);
        assert_eq!(restored.cells[0].outputs[0]["text"], "keep");
        assert!(restored.cells[1].outputs.is_empty());
        assert_eq!(restored.cells[1].source, "earlier()");
        assert_eq!(restored.cells[2].source, "Notes\n");
        assert_eq!(nb.cells[0].source, "current()");
    }
}
