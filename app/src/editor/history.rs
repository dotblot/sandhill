//! Earlier text versions, compared with the current document.

use std::ops::RangeInclusive;
use std::rc::Rc;

use gpui_kit::component::{Disableable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, FileText, FileVersionInfo, FileVersionReason, NotebookHistory};

use crate::editor::notebook::{Format, format_of};
use crate::editor::text_diff::{DiffLine, diff};
use crate::look::{self, Palette};
use crate::panes::files::base_name;
use crate::workbench::Workbench;

mod restore;
pub(crate) use restore::NotebookHistoryEdits;

const FIRST_ROWS: usize = 4000;

pub(crate) struct HistoryTab {
    pub path: String,
    token: Rc<()>,
    versions: Option<Vec<FileVersionInfo>>,
    selected: Option<i64>,
    details: Option<Comparison>,
    error: Option<String>,
    anchor: Option<usize>,
    selection: Option<RangeInclusive<usize>>,
    all: bool,
    scroll: ScrollHandle,
}

struct Comparison {
    text: String,
    /// Jupyter restore needs cell IDs, kinds and sources from the JSON.
    snapshot: Option<String>,
    diff: Vec<DiffLine>,
    notice: Option<String>,
}

impl Comparison {
    fn new(version: String, now: anyhow::Result<String>, notebook: bool) -> anyhow::Result<Self> {
        let (text, snapshot) = if notebook {
            (
                NotebookHistory::from_version(&version)?.readable_text(),
                Some(version),
            )
        } else {
            (version, None)
        };
        let (now, mut notice) = match now {
            Ok(now) => (now, None),
            Err(err) => (
                String::new(),
                Some(format!("Could not read the current file: {err:#}")),
            ),
        };
        let rows = diff(&text, &now);
        if notice.is_none()
            && !rows
                .iter()
                .any(|line| matches!(line, DiffLine::Added(_) | DiffLine::Removed(_)))
        {
            notice = Some(
                if text == now {
                    "Same as the current file."
                } else {
                    "Only line endings differ."
                }
                .into(),
            );
        }
        Ok(Self {
            text,
            snapshot,
            diff: rows,
            notice,
        })
    }

    fn copy_text(&self, selection: Option<&RangeInclusive<usize>>) -> String {
        selection.map_or_else(
            || self.text.clone(),
            |range| {
                self.text
                    .split_inclusive('\n')
                    .skip(*range.start() - 1)
                    .take(range.end() - range.start() + 1)
                    .collect()
            },
        )
    }
}

impl Workbench {
    fn show_file_history(&mut self) {
        self.reveal_middle();
        if self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor) {
            self.middle_vertical = crate::shell::pane_sizes::Vertical::Split;
        }
        self.hide_object_explorer();
        self.diff_shown = false;
        self.explorer_shown = false;
        self.history_shown = true;
    }

    pub(crate) fn open_file_history(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        self.show_file_history();
        let token = Rc::new(());
        self.history = Some(HistoryTab {
            path: path.clone(),
            token: token.clone(),
            versions: None,
            selected: None,
            details: None,
            error: None,
            anchor: None,
            selection: None,
            all: false,
            scroll: ScrollHandle::new(),
        });
        let request = daemon.call_in_project::<Vec<FileVersionInfo>>(
            self.project.clone(),
            Call::FileVersions { path },
        );
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                let Some(tab) = this
                    .history
                    .as_mut()
                    .filter(|tab| Rc::ptr_eq(&token, &tab.token))
                else {
                    return;
                };
                match result {
                    Ok(versions) => {
                        let newest = versions.first().map(|v| v.id);
                        tab.versions = Some(versions);
                        if let Some(id) = newest {
                            this.select_file_version(id, cx);
                        }
                    }
                    Err(err) if crate::daemon::unknown_call(&err) => {
                        tab.error =
                            Some("File history needs a newer Sandhill on this server.".into());
                    }
                    Err(err) => tab.error = Some(format!("Could not load file history: {err:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_file_version(&mut self, id: i64, cx: &mut Context<Self>) {
        let (Some(daemon), Some(tab)) = (self.daemon(), self.history.as_mut()) else {
            return;
        };
        tab.selected = Some(id);
        tab.details = None;
        tab.error = None;
        tab.anchor = None;
        tab.selection = None;
        tab.all = false;
        tab.scroll.set_offset(Point::default());
        let token = Rc::new(());
        tab.token = token.clone();
        let path = tab.path.clone();
        let notebook = format_of(&path) == Some(Format::Ipynb);
        let current = self
            .open
            .iter()
            .chain(&self.others)
            .find(|doc| doc.notebook.path == path && !doc.notebook.truncated)
            .map(|doc| doc.notebook.history_comparison_text(&doc.sources(cx)));
        let question = daemon.call_in_project::<String>(
            self.project.clone(),
            Call::FileVersion {
                path: path.clone(),
                id,
            },
        );
        cx.spawn(async move |this, cx| {
            let result = match question.await {
                Err(err) => Err(err),
                Ok(text) => {
                    let now = match current {
                        Some(current) => current,
                        None if notebook => daemon
                            .call::<FileText>(Call::ReadNotebook { path })
                            .await
                            .and_then(|file| {
                                Ok(serde_json::from_str::<NotebookHistory>(&file.text)?
                                    .readable_text())
                            }),
                        None => daemon
                            .call::<FileText>(Call::ReadFile { path })
                            .await
                            .map(|file| file.text),
                    };
                    cx.background_spawn(async move { Comparison::new(text, now, notebook) })
                        .await
                }
            };
            let _ = this.update(cx, |this, cx| {
                let Some(tab) = this
                    .history
                    .as_mut()
                    .filter(|tab| Rc::ptr_eq(&token, &tab.token))
                else {
                    return;
                };
                match result {
                    Ok(details) => tab.details = Some(details),
                    Err(err) => tab.error = Some(format!("Could not read this version: {err:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn close_file_history(&mut self, cx: &mut Context<Self>) {
        self.history = None;
        self.history_shown = false;
        cx.notify();
    }

    pub(crate) fn history_tab_element(
        &self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let tab = self.history.as_ref()?;
        let p = *Palette::get(cx);
        let picked = expanded && self.history_shown;
        let lead = look::icon("history")
            .size(px(look::ICON))
            .into_any_element();
        let (lead, trailing) = if expanded {
            look::place_close(
                lead,
                look::tab_close("history-close", picked, &p).on_click(cx.listener(
                    |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_file_history(cx);
                    },
                )),
                picked,
            )
        } else {
            (lead, None)
        };
        let path = tab.path.clone();
        Some(
            Self::tab(
                "history-tab",
                Some(lead),
                if expanded {
                    format!("History: {}", base_name(&path))
                } else {
                    String::new()
                },
                picked,
                &p,
            )
            .flex_none()
            .when_some(trailing, |d, close| d.child(close))
            .tooltip(|w, cx| {
                gpui_kit::component::tooltip::Tooltip::new("File history").build(w, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.open_file_history(path.clone(), cx)))
            .into_any_element(),
        )
    }

    pub(crate) fn render_file_history(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.history.as_ref().expect("visible history tab");
        let p = *Palette::get(cx);
        let file = base_name(&tab.path).to_string();
        let details = tab.details.as_ref();
        let controls = h_flex()
            .flex_none()
            .px(px(look::PANE_PAD_X))
            .gap(px(look::GAP_8))
            .h(px(look::control_row()))
            .child(div().min_w_0().flex_1().truncate().child(file))
            .child(
                look::button("history-copy", "Copy", None, look::ButtonSize::Row)
                    .disabled(details.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.copy_file_history(cx))),
            )
            .child(
                look::button(
                    "history-restore",
                    "Restore This Version",
                    None,
                    look::ButtonSize::Row,
                )
                .disabled(details.is_none())
                .on_click(cx.listener(|this, _, window, cx| this.restore_file_history(window, cx))),
            );
        let list = v_flex()
            .id("history-versions")
            .w(relative(0.34))
            .flex_none()
            .min_h_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(p.line)
            .children(tab.versions.iter().flatten().map(|version| {
                let id = version.id;
                let time = chrono::DateTime::from_timestamp_millis(version.time_ms)
                    .map(|t| {
                        t.with_timezone(&chrono::Local)
                            .format("%b %-d, %Y %H:%M")
                            .to_string()
                    })
                    .unwrap_or_default();
                let reason = match version.reason {
                    FileVersionReason::Saved => "Saved",
                    FileVersionReason::Opened => "Opened",
                };
                v_flex()
                    .id(("history-version", id as usize))
                    .flex_none()
                    .min_w_0()
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::GAP_4))
                    .cursor_pointer()
                    .text_size(px(look::small()))
                    .hover(|d| d.bg(p.hover))
                    .when(tab.selected == Some(id), |d| d.bg(p.accent.opacity(0.16)))
                    .child(div().truncate().child(time))
                    .child(
                        h_flex()
                            .gap(px(look::GAP_6))
                            .text_color(p.muted)
                            .child(reason)
                            .child(div().text_color(p.ok).child(format!("+{}", version.added)))
                            .child(
                                div()
                                    .text_color(p.err)
                                    .child(format!("-{}", version.removed)),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.select_file_version(id, cx)))
            }));
        let notice = tab.error.as_ref().map(|s| (s.as_str(), p.err)).or_else(|| {
            if let Some(details) = details {
                details.notice.as_deref().map(|s| (s, p.muted))
            } else if tab.versions.as_ref().is_some_and(Vec::is_empty) {
                Some(("No versions kept for this file.", p.muted))
            } else if tab.versions.is_none() {
                Some(("Loading file history…", p.muted))
            } else {
                Some(("Loading version…", p.muted))
            }
        });
        let mut old = 0;
        let mut new = 0;
        let rows = details.map_or(&[][..], |details| details.diff.as_slice());
        let shown = if tab.all {
            rows.len()
        } else {
            rows.len().min(FIRST_ROWS)
        };
        // Scroll extents come from direct children. Put the unwrapped rows
        // in the scroll container itself so wide code adds to its extent.
        let content = v_flex()
            .id("history-diff")
            .items_start()
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_scroll()
            .track_scroll(&tab.scroll)
            .restrict_scroll_to_axis()
            .when_some(notice, |d, (label, color)| {
                d.child(
                    div()
                        .p(px(look::PANE_PAD_X))
                        .text_size(px(look::small()))
                        .text_color(color)
                        .child(label.to_owned()),
                )
            })
            .when(
                rows.iter().any(|row| matches!(row, DiffLine::Gap(_))),
                |d| {
                    d.child(
                        look::button(
                            "history-context",
                            "Show unchanged lines",
                            None,
                            look::ButtonSize::Row,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.expand_history_lines(cx))),
                    )
                },
            )
            .children(rows[..shown].iter().enumerate().map(|(ix, line)| {
                let number = |value: Option<usize>| {
                    div()
                        .w(px(look::row() * 2.))
                        .flex_none()
                        .text_right()
                        .text_color(p.muted)
                        .child(value.map(|n| n.to_string()).unwrap_or_default())
                };
                let (left, right, mark, text, tint) = match line {
                    DiffLine::Same(text) => {
                        old += 1;
                        new += 1;
                        (Some(old), Some(new), " ", text.clone(), None)
                    }
                    DiffLine::Removed(text) => {
                        old += 1;
                        (
                            Some(old),
                            None,
                            "-",
                            text.clone(),
                            Some(p.err.opacity(0.12)),
                        )
                    }
                    DiffLine::Added(text) => {
                        new += 1;
                        (None, Some(new), "+", text.clone(), Some(p.ok.opacity(0.12)))
                    }
                    DiffLine::Gap(n) => {
                        old += n;
                        new += n;
                        (
                            None,
                            None,
                            " ",
                            format!("{n} unchanged {}", if *n == 1 { "line" } else { "lines" }),
                            Some(p.chunk),
                        )
                    }
                };
                let selected = left.is_some_and(|line| {
                    tab.selection
                        .as_ref()
                        .is_some_and(|range| range.contains(&line))
                });
                h_flex()
                    .id(("history-line", ix))
                    .min_w_full()
                    .flex_none()
                    .h(px(look::row()))
                    .gap(px(look::GAP_6))
                    .px(px(look::GAP_4))
                    .font_family(look::mono())
                    .text_size(px(look::code()))
                    .when_some(tint, |d, color| d.bg(color))
                    .when(selected, |d| d.bg(p.accent.opacity(0.28)))
                    .child(number(left))
                    .child(number(right))
                    .child(mark)
                    .child(div().flex_none().whitespace_nowrap().child(text))
                    .when_some(left, |d, line| {
                        d.cursor_pointer().on_click(cx.listener(
                            move |this, click: &ClickEvent, _, cx| {
                                if let Some(tab) = this.history.as_mut() {
                                    let anchor = if click.modifiers().shift {
                                        tab.anchor.unwrap_or(line)
                                    } else {
                                        line
                                    };
                                    tab.anchor = Some(anchor);
                                    tab.selection = Some(anchor.min(line)..=anchor.max(line));
                                }
                                cx.notify();
                            },
                        ))
                    })
            }))
            .when(shown < rows.len(), |d| {
                d.child(
                    look::button("history-more", "Show all", None, look::ButtonSize::Row).on_click(
                        cx.listener(|this, _, _, cx| {
                            if let Some(tab) = this.history.as_mut() {
                                tab.all = true;
                            }
                            cx.notify();
                        }),
                    ),
                )
            });
        look::pane(&p)
            .flex_1()
            .h_full()
            .child(self.render_tabs(cx))
            .child(controls)
            .child(
                h_flex()
                    .items_start()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(list.h_full())
                    .child(content),
            )
            .into_any_element()
    }

    fn expand_history_lines(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.history.as_mut().and_then(|tab| tab.details.as_mut()) else {
            return;
        };
        let mut source = details.text.lines();
        details.diff = std::mem::take(&mut details.diff)
            .into_iter()
            .flat_map(|row| match row {
                DiffLine::Gap(count) => source
                    .by_ref()
                    .take(count)
                    .map(|line| DiffLine::Same(line.to_owned()))
                    .collect(),
                row => {
                    if matches!(row, DiffLine::Same(_) | DiffLine::Removed(_)) {
                        source.next();
                    }
                    vec![row]
                }
            })
            .collect();
        cx.notify();
    }

    fn copy_file_history(&self, cx: &mut App) {
        let Some(tab) = &self.history else { return };
        let Some(details) = &tab.details else { return };
        let text = details.copy_text(tab.selection.as_ref());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui_kit::{AppContext, ScrollHandle, TestAppContext, WeakEntity, WindowOptions, px};

    use super::{Comparison, DiffLine, HistoryTab, Workbench};

    #[test]
    fn notebook_history_compares_and_copies_code_not_snapshot_json() {
        let version = r##"[
            {"id":"a","cell_type":"code","source":["  plt.title('old')\n"]},
            {"id":"b","cell_type":"markdown","source":["# Notes"]}
        ]"##;
        let current = crate::editor::notebook::parse(
            "/p/test.ipynb",
            r##"{"cells":[
            {"id":"a","cell_type":"code","source":"  plt.title('new')\n"},
            {"id":"b","cell_type":"markdown","source":"# Notes"}
        ]}"##,
        )
        .unwrap();

        let sources = current
            .cells
            .iter()
            .map(|cell| cell.source.clone())
            .collect::<Vec<_>>();
        let comparison = Comparison::new(
            version.into(),
            current.history_comparison_text(&sources),
            true,
        )
        .unwrap();
        assert_eq!(
            comparison.copy_text(None),
            "Cell 1 · code\n  plt.title('old')\n\nCell 2 · markdown\n# Notes\n\n"
        );
        assert_eq!(comparison.copy_text(Some(&(2..=2))), "  plt.title('old')\n");
        assert_eq!(comparison.snapshot.as_deref(), Some(version));
        assert_eq!(
            comparison
                .diff
                .iter()
                .filter(|line| matches!(line, DiffLine::Added(_) | DiffLine::Removed(_)))
                .collect::<Vec<_>>(),
            [
                &DiffLine::Removed("  plt.title('old')".into()),
                &DiffLine::Added("  plt.title('new')".into())
            ]
        );
    }

    #[gpui_kit::test]
    fn long_history_lines_extend_the_horizontal_scroll_range(cx: &mut TestAppContext) {
        let scroll = ScrollHandle::new();
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (_, _workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            crate::welcome::resource_manager::block_host(sandhill_link::LOCAL, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    let mut workbench = Workbench::new(
                        rt,
                        sandhill_link::Target::Local,
                        None,
                        WeakEntity::new_invalid(),
                        window,
                        cx,
                    );
                    workbench.history = Some(HistoryTab {
                        path: "/p/long.py".into(),
                        token: Rc::new(()),
                        versions: None,
                        selected: None,
                        details: Some(
                            Comparison::new("x".repeat(1024), Ok(String::new()), false).unwrap(),
                        ),
                        error: None,
                        anchor: None,
                        selection: None,
                        all: false,
                        scroll: scroll.clone(),
                    });
                    workbench.show_file_history();
                    workbench
                })
            })
            .unwrap()
        });
        cx.run_until_parked();
        assert!(
            scroll.max_offset().x > px(0.),
            "long code must increase the real scroll extent"
        );
    }
}
