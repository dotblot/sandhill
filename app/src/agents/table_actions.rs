//! Under each table in an AI reply (Huan, 2026-11-16): Copy (tab-separated,
//! so it pastes into a spreadsheet as cells), Save as CSV… (on this Mac) and
//! Open in Data Explorer, which puts it into the Console's session as a
//! data frame, `ai_table_1`, and opens that.

use std::rc::Rc;

use gpui_kit::base::text::TableData;
use gpui_kit::component::h_flex;
use gpui_kit::*;
use sandhill_proto::Call;

use crate::agents::cell_ai::pill;
use crate::look::Palette;
use crate::workbench::Workbench;

/// The table's buttons; `workbench` is the window's.
pub(crate) fn table_buttons(
    table: &TableData,
    workbench: WeakEntity<Workbench>,
    p: &Palette,
) -> Div {
    let table = Rc::new((table.headers.clone(), table.rows.clone()));
    let copied = table.clone();
    let saved = table.clone();
    let saving = workbench.clone();
    h_flex()
        .gap(px(6.))
        .mt(px(4.))
        .child(
            pill("copy", "edit.copy", "Copy", p).on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(delimited(
                    &copied.0, &copied.1, '\t',
                )))
            }),
        )
        .child(
            pill("save", "doc.download", "Save as CSV…", p).on_click(move |_, _, cx| {
                save_csv(delimited(&saved.0, &saved.1, ','), saving.clone(), cx)
            }),
        )
        .child(
            pill("explore", "table", "Open in Data Explorer", p).on_click(move |_, _, cx| {
                let (headers, rows) = (table.0.clone(), table.1.clone());
                workbench
                    .update(cx, |w, cx| w.explore_table(headers, rows, cx))
                    .ok();
            }),
        )
}

/// The table as text: a header line, then a line per row; CSV quoting
/// when `separator` is a comma.
fn delimited(headers: &[String], rows: &[Vec<String>], separator: char) -> String {
    let field = |text: &str| {
        if separator == ',' && text.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", text.replace('"', "\"\""))
        } else if separator == '\t' {
            text.replace(['\t', '\n'], " ")
        } else {
            text.to_string()
        }
    };
    let mut out = String::new();
    for line in std::iter::once(headers).chain(rows.iter().map(Vec::as_slice)) {
        let cells: Vec<String> = (0..headers.len().max(line.len()))
            .map(|i| field(line.get(i).map_or("", String::as_str)))
            .collect();
        out.push_str(&cells.join(&separator.to_string()));
        out.push('\n');
    }
    out
}

/// Asks where on this Mac, then writes `csv` there.
fn save_csv(csv: String, workbench: WeakEntity<Workbench>, cx: &mut App) {
    let folder = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("Downloads"))
        .unwrap_or_else(std::env::temp_dir);
    let picked = cx.prompt_for_new_path(&folder, Some("ai_table.csv"));
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(path))) = picked.await
            && let Err(err) = std::fs::write(&path, csv)
        {
            workbench
                .update(cx, |w, cx| {
                    w.problem = Some(format!("Could not save {}: {err}", path.display()));
                    cx.notify();
                })
                .ok();
        }
    })
    .detach();
}

/// The R or Python that makes `name` from `csv`, the numbers as numbers,
/// leaving no other names behind.
fn table_code(name: &str, csv: &str, python: bool) -> String {
    // A JSON string is also a valid R and Python string.
    let text = serde_json::to_string(csv).unwrap_or_default();
    if python {
        format!("{name} = __import__(\"pandas\").read_csv(__import__(\"io\").StringIO({text}))")
    } else {
        format!(
            "{name} <- utils::read.csv(text = {text}, check.names = FALSE, stringsAsFactors = FALSE)"
        )
    }
}

impl Workbench {
    /// Puts a reply's table into the Variables pane's session as the first
    /// free `ai_table_N` and opens it in the Data Explorer.
    pub(crate) fn explore_table(
        &mut self,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            self.problem = Some("Start a kernel in the Console first: the Data Explorer shows tables from a session.".into());
            cx.notify();
            return;
        };
        let python = self
            .session_language(&session)
            .is_some_and(|l| l.eq_ignore_ascii_case("python"));
        // Not one being made now, nor one the Variables pane has.
        let pending = self.explore_after.as_ref().map(|(_, name)| name.as_str());
        let taken: std::collections::HashSet<&str> = self
            .vars
            .list
            .iter()
            .map(|v| v.name.as_str())
            .chain(pending)
            .collect();
        let name = (1..)
            .map(|n| format!("ai_table_{n}"))
            .find(|n| !taken.contains(n.as_str()))
            .unwrap_or_default();
        let code = table_code(&name, &delimited(&headers, &rows, ','), python);
        // A visible run, so the Console shows how the table was made and the
        // Variables pane lists it.
        let started = daemon.call_in_order::<sandhill_proto::RunStarted>(Call::Execute {
            session,
            code,
            cell: None,
            quiet: false,
            by: Some("Sandhill".into()),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                match started {
                    Ok(started) => {
                        this.explore_after = Some((started.run.clone(), name));
                        this.explore_when_made(&started.run, cx);
                    }
                    Err(err) => this.problem = Some(format!("Could not make the table: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the table [`Self::explore_table`] made, once run `run` made it.
    pub(crate) fn explore_when_made(&mut self, run: &str, cx: &mut Context<Self>) {
        let Some(finished) = self.runs.get(run).filter(|r| r.finished) else {
            return;
        };
        if self.explore_after.as_ref().is_none_or(|(r, _)| r != run) {
            return;
        }
        let ok = finished.status.as_deref() == Some("ok");
        let Some((_, name)) = self.explore_after.take() else {
            return;
        };
        if !ok {
            self.problem = Some(format!("Could not make {name}: the Console shows why."));
            return;
        }
        let handle = self.window_handle;
        let this = cx.entity().downgrade();
        cx.defer(move |cx| {
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |w, cx| w.open_explorer(vec![name], window, cx))
                    .ok();
            })
            .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{delimited, table_code};

    #[test]
    fn tables_leave_the_reply_as_cells() {
        let headers = vec!["gene".to_string(), "note".to_string()];
        let rows = vec![
            vec!["TP53".to_string(), "up, \"strongly\"".to_string()],
            // Still streaming: a short row.
            vec!["MYC".to_string()],
        ];
        assert_eq!(
            delimited(&headers, &rows, ','),
            "gene,note\nTP53,\"up, \"\"strongly\"\"\"\nMYC,\n"
        );
        assert_eq!(
            delimited(&headers, &rows, '\t'),
            "gene\tnote\nTP53\tup, \"strongly\"\nMYC\t\n"
        );
        let csv = delimited(&headers, &rows, ',');
        let r = table_code("ai_table_1", &csv, false);
        assert!(r.starts_with("ai_table_1 <- utils::read.csv(text = \"gene,note\\nTP53,"));
        // One assignment: no other names are left in the user's session.
        let python = table_code("ai_table_1", &csv, true);
        assert!(python.starts_with("ai_table_1 = __import__(\"pandas\").read_csv("));
        assert!(!python.contains('\n') && !python.contains(" import "));
    }
}
