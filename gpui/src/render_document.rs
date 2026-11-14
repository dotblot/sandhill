//! One-shot Quarto rendering uses the daemon's persistent Jobs machinery.
//! It never executes code in, or borrows variables from, a live kernel.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::*;
use sandhill_proto::{JobSpec, TerminalInfo};

use crate::look::{self, Palette, bar_button, icon_button};
use crate::workbench::{BottomTab, Workbench};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputAction {
    Viewer,
    Pdf,
    Files,
}

pub(crate) fn output_action(path: &str) -> OutputAction {
    if crate::viewer::is_html(path) {
        OutputAction::Viewer
    } else if path
        .rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("pdf"))
    {
        OutputAction::Pdf
    } else {
        OutputAction::Files
    }
}

fn output_label(path: &str) -> &'static str {
    match output_action(path) {
        OutputAction::Viewer => "Open in Viewer",
        OutputAction::Pdf => "Open PDF",
        OutputAction::Files => "Reveal in Files",
    }
}

pub(crate) fn render_tooltip(kernel: Option<&str>) -> &'static str {
    if kernel.is_some_and(|name| !name.trim().is_empty()) {
        "Render with Quarto as a background job in a fresh process. Current session variables are not used."
    } else {
        "Render with Quarto as a background job in a fresh process. Current session variables are not used. No kernel is declared in the document; results may differ from the current session."
    }
}

fn relative_output<'a>(project: &str, path: &'a str) -> Option<&'a str> {
    path.strip_prefix(project.trim_end_matches('/'))?
        .strip_prefix('/')
        .filter(|rel| !rel.is_empty())
}

struct RenderNotice {
    title: String,
    outputs: Vec<String>,
}

/// Kept with Jobs, so changing projects discards its notices and reveal state.
pub(crate) struct RenderState {
    notice: Option<RenderNotice>,
    notified: HashMap<String, i64>,
    files_scroll: UniformListScrollHandle,
    reveal: RefCell<Option<String>>,
}

impl Default for RenderState {
    fn default() -> Self {
        Self {
            notice: None,
            notified: HashMap::new(),
            files_scroll: UniformListScrollHandle::new(),
            reveal: RefCell::new(None),
        }
    }
}

impl RenderState {
    /// Both live finish events and restored job lists use the same deduplication.
    pub(crate) fn observe(&mut self, job: &TerminalInfo) -> Option<String> {
        let path = job.job.as_ref()?.path.as_deref()?;
        if job.running || !crate::viewer::is_previewable(path) {
            return None;
        }
        let exit = job.exit.as_ref()?;
        if self.notified.get(&job.id) == Some(&exit.ended_ms) {
            return None;
        }
        self.notified.insert(job.id.clone(), exit.ended_ms);
        let success = exit.code == Some(0) && exit.signal.is_none();
        let title = if success {
            format!("Rendered {}", crate::files::base_name(path))
        } else {
            format!(
                "Render failed: {}. See Jobs for details.",
                crate::files::base_name(path)
            )
        };
        let outputs = if success {
            job.output_paths.clone()
        } else {
            Vec::new()
        };
        let status = if outputs.is_empty() {
            title.clone()
        } else {
            format!("{title}: {}", outputs.join(", "))
        };
        self.notice = Some(RenderNotice { title, outputs });
        Some(status)
    }
}

impl Workbench {
    /// Saves through the ordinary conflict-aware path before starting a job.
    pub(crate) fn render_document(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if !crate::viewer::is_previewable(&open.notebook.path) {
            return;
        }
        if self.daemon().is_none() {
            self.problem = Some("Connect to the device before rendering a document.".into());
            cx.notify();
            return;
        }
        let path = open.notebook.path.clone();
        if !open.dirty {
            self.start_document_render(path, cx);
            return;
        }
        let generation = self.generation;
        let project = self.project.clone();
        let Some(saved) = self.save_file(&path, cx) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            saved.await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project != project {
                    return;
                }
                let Some(open) = this.open_at(&path) else {
                    return;
                };
                if open.dirty {
                    // A failed/conflicting save keeps its existing error. Edits
                    // made during a successful save must not render stale text.
                    if this.problem.is_none() {
                        this.problem = Some(
                            "Render not started: the document still has unsaved changes.".into(),
                        );
                    }
                    cx.notify();
                    return;
                }
                this.start_document_render(path, cx);
            })
            .ok();
        })
        .detach();
    }

    fn start_document_render(&mut self, path: String, cx: &mut Context<Self>) {
        if self.daemon().is_none() {
            self.problem = Some("Connect to the device before rendering a document.".into());
            cx.notify();
            return;
        }
        let Some(open) = self.open_at(&path) else {
            return;
        };
        let note = render_tooltip(open.notebook.kernel.as_deref());
        self.notice = Some(note.into());
        self.start_job(
            JobSpec {
                path: Some(path),
                command: None,
            },
            cx,
        );
    }

    pub(crate) fn render_document_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let kernel = self
            .open
            .as_ref()
            .and_then(|open| open.notebook.kernel.as_deref());
        let tip = render_tooltip(kernel);
        // Icon-only: the document row is already full at the default width.
        icon_button("render-document", "share", &p, false)
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
            .on_click(cx.listener(|this, _, _, cx| this.render_document(cx)))
            .into_any_element()
    }

    pub(crate) fn open_render_output(&mut self, path: String, cx: &mut Context<Self>) {
        match output_action(&path) {
            OutputAction::Viewer => {
                let title = crate::files::base_name(&path).to_string();
                self.open_html(path, title, cx);
            }
            OutputAction::Pdf => self.open_media(path, cx),
            OutputAction::Files => {
                let Some(rel) = self
                    .project
                    .as_deref()
                    .and_then(|project| relative_output(project, &path))
                    .map(str::to_string)
                else {
                    self.problem = Some(format!(
                        "Cannot reveal {path} in Files: it is outside the current project."
                    ));
                    cx.notify();
                    return;
                };
                let parent = rel.rsplit_once('/').map_or("", |(parent, _)| parent);
                self.open_to(parent, cx);
                self.files_ui.selected = Some((rel.clone(), false));
                *self.jobs.render.reveal.borrow_mut() = Some(rel);
                self.bottom_tab = BottomTab::Files;
                self.show_files = true;
                self.refresh_files(cx);
                cx.notify();
            }
        }
    }

    /// Attach this handle to the Files uniform list. Scroll only once, after
    /// its ancestor folders have loaded, rather than fighting later user scrolls.
    pub(crate) fn render_output_file_scroll(&self) -> UniformListScrollHandle {
        let pending = self.jobs.render.reveal.borrow();
        let index = pending.as_ref().and_then(|path| {
            self.file_lines().iter().position(
                |line| matches!(line, crate::tree_actions::Line::Row(row) if &row.rel == path),
            )
        });
        drop(pending);
        if let Some(index) = index {
            self.jobs
                .render
                .files_scroll
                .scroll_to_item(index, ScrollStrategy::Center);
            self.jobs.render.reveal.borrow_mut().take();
        }
        self.jobs.render.files_scroll.clone()
    }

    /// A compact, dismissible finish notice. Output actions remain available
    /// in the job's log header even after this notice has been dismissed.
    pub(crate) fn render_document_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.jobs.render.notice.as_ref()?;
        let p = *Palette::get(cx);
        Some(
            v_flex()
                .flex_none()
                .mx(px(look::EDGE))
                .px(px(10.))
                .py(px(5.))
                .gap(px(3.))
                .text_size(px(look::SMALL))
                .text_color(p.text)
                .border_t(px(0.5))
                .border_color(p.glass_edge)
                .child(
                    h_flex()
                        .gap(px(8.))
                        .child(div().flex_1().min_w_0().child(notice.title.clone()))
                        .child(
                            icon_button("dismiss-render-notice", "close", &p, false).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.jobs.render.notice = None;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
                .child(self.render_output_actions(&notice.outputs, "render-notice", cx))
                .into_any_element(),
        )
    }

    pub(crate) fn render_output_actions(
        &self,
        paths: &[String],
        id: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        v_flex()
            .id(SharedString::from(format!("{id}-outputs")))
            .flex_none()
            .max_h(px(100.))
            .overflow_y_scroll()
            .text_size(px(look::SMALL))
            .children(paths.iter().enumerate().map(|(index, path)| {
                let path = path.clone();
                let label = format!("{}: {path}", output_label(&path));
                bar_button(
                    SharedString::from(format!("{id}-output-{index}")),
                    label,
                    Some("file"),
                    &p,
                )
                .w_full()
                .min_w_0()
                .overflow_hidden()
                .text_size(px(look::SMALL))
                .tooltip({
                    let path = path.clone();
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                    }
                })
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_render_output(path.clone(), cx)),
                )
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OutputAction, RenderState, TerminalInfo, output_action, relative_output, render_tooltip,
    };

    fn job(extra: serde_json::Value) -> TerminalInfo {
        let mut value = serde_json::json!({
            "id": "render-1", "kind": "job", "title": "report.qmd",
            "cwd": "/p", "project": "/p", "created_ms": 0,
            "running": false, "end": 0, "job": {"path": "/p/report.qmd"},
            "exit": {"code": 0, "ended_ms": 10},
            "output_paths": ["/p/report.html", "/p/report.pdf"]
        });
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn output_extensions_choose_the_existing_surface() {
        for path in ["/p/report.html", "/p/REPORT.HTML", "/p/report.HtM"] {
            assert_eq!(output_action(path), OutputAction::Viewer);
        }
        assert_eq!(output_action("/p/report.PDF"), OutputAction::Pdf);
        for path in [
            "/p/report.docx",
            "/p/report.png",
            "/p/pdf",
            "/p/report.pdf.tmp",
            "/p/dir.pdf/report",
        ] {
            assert_eq!(output_action(path), OutputAction::Files);
        }
    }

    #[test]
    fn project_reveal_respects_path_boundaries_and_root() {
        assert_eq!(
            relative_output("/p", "/p/_site/report.docx"),
            Some("_site/report.docx")
        );
        assert_eq!(
            relative_output("/p/", "/p/report.docx"),
            Some("report.docx")
        );
        assert_eq!(relative_output("/", "/report.docx"), Some("report.docx"));
        assert_eq!(relative_output("/p", "/project/report.docx"), None);
    }

    #[test]
    fn render_warning_never_claims_to_use_session_variables() {
        assert!(render_tooltip(Some("python3")).contains("fresh process"));
        assert!(!render_tooltip(Some("python3")).contains("No kernel"));
        assert!(render_tooltip(None).contains("results may differ"));
        assert!(render_tooltip(Some(" ")).contains("No kernel"));
    }

    #[test]
    fn completion_is_deduplicated_but_restart_is_reported() {
        let mut state = RenderState::default();
        let info = job(serde_json::json!({}));
        assert!(state.observe(&info).unwrap().contains("/p/report.pdf"));
        assert!(state.observe(&info).is_none());
        assert!(
            state
                .observe(&job(
                    serde_json::json!({"exit": {"code": 0, "ended_ms": 20}})
                ))
                .is_some()
        );
        assert!(
            state
                .observe(&job(serde_json::json!({"running": true})))
                .is_none()
        );
        assert!(
            state
                .observe(&job(serde_json::json!({"job": {"path": "/p/script.R"}})))
                .is_none()
        );
    }

    #[test]
    fn unsuccessful_render_does_not_offer_stale_outputs() {
        let mut state = RenderState::default();
        let info = job(serde_json::json!({"exit": {"code": 1, "ended_ms": 10}}));
        assert!(state.observe(&info).unwrap().contains("Render failed"));
        assert!(state.notice.as_ref().unwrap().outputs.is_empty());
    }
}
