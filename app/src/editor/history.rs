//! Earlier text versions, compared with the current document.

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
    selection: Option<std::ops::RangeInclusive<usize>>,
    all: bool,
    scroll: ScrollHandle,
}

struct Comparison {
    text: String,
    diff: Vec<DiffLine>,
    notice: Option<String>,
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
        let current = self
            .open
            .iter()
            .chain(&self.others)
            .find(|doc| doc.notebook.path == path && !doc.notebook.truncated)
            .map(|doc| doc.notebook.history_text(&doc.sources(cx)));
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
                    let now =
                        match current {
                            Some(current) => current,
                            None if format_of(&path) == Some(Format::Ipynb) => daemon
                                .call::<FileText>(Call::ReadNotebook { path })
                                .await
                                .and_then(|file| {
                                    Ok(serde_json::from_str::<NotebookHistory>(&file.text)?
                                        .render()?)
                                }),
                            None => daemon
                                .call::<FileText>(Call::ReadFile { path })
                                .await
                                .map(|file| file.text),
                        };
                    Ok(cx
                        .background_spawn(async move {
                            match now {
                                Ok(now) => {
                                    let rows = diff(&text, &now);
                                    let notice = (!rows.iter().any(|l| {
                                        matches!(l, DiffLine::Added(_) | DiffLine::Removed(_))
                                    }))
                                    .then(|| {
                                        if text == now {
                                            "Same as the current file."
                                        } else {
                                            "Only line endings differ."
                                        }
                                        .into()
                                    });
                                    Comparison {
                                        text,
                                        diff: rows,
                                        notice,
                                    }
                                }
                                Err(err) => Comparison {
                                    diff: diff(&text, ""),
                                    text,
                                    notice: Some(format!(
                                        "Could not read the current file: {err:#}"
                                    )),
                                },
                            }
                        })
                        .await)
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
        let content = v_flex()
            .min_w_full()
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
                            format!("{n} unchanged lines"),
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
                    .whitespace_nowrap()
                    .when_some(tint, |d, color| d.bg(color))
                    .when(selected, |d| d.bg(p.accent.opacity(0.28)))
                    .child(number(left))
                    .child(number(right))
                    .child(mark)
                    .child(text)
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
                    .child(
                        div()
                            .id("history-diff")
                            .h_full()
                            .flex_1()
                            .min_w_0()
                            .overflow_scroll()
                            .track_scroll(&tab.scroll)
                            .restrict_scroll_to_axis()
                            .child(content),
                    ),
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
        let text = tab.selection.as_ref().map_or_else(
            || details.text.clone(),
            |range| {
                details
                    .text
                    .split_inclusive('\n')
                    .skip(*range.start() - 1)
                    .take(range.end() - range.start() + 1)
                    .collect()
            },
        );
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}
