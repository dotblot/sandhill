//! File-scoped AI shortcuts. Each starts a new conversation, not a new turn.
use gpui_kit::base::ElementExt as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::AgentOptions;
use serde_json::json;

use crate::look::{self, Palette, icon_button};
use crate::workbench::Workbench;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotebookAction {
    Explain,
    Improve,
}

impl NotebookAction {
    pub(crate) fn prompt(self, path: &str) -> String {
        match self {
            Self::Explain => format!(
                "Explain this notebook: {path}. Describe its purpose, the analysis in cell order, inputs, outputs, assumptions, and any limitations. Use the attached file and its existing outputs. This is read-only: do not edit files or run code."
            ),
            Self::Improve => format!(
                "Improve this notebook: {path}. First inspect the attached notebook and explain the changes you propose. Make targeted, per-cell changes using edit_cell so Sandhill shows each diff with Keep/Undo for the user to review. Do not rewrite the whole notebook or replace the file with a new version. Preserve the analysis intent and existing results unless a change is explained. For single-cell analysis, consult Sandhill's built-in bioinformatics guides before proposing changes. Do not run the analysis unless the user asks."
            ),
        }
    }

    pub(crate) fn options(self, kind: &str, mut options: AgentOptions, path: &str) -> AgentOptions {
        options.documents = vec![path.to_string()];
        options.refs = vec![json!({"kind": "file", "name": path, "path": path})];
        options.pin = Some(path.to_string());
        options.code = None;
        // Explain only looks, in the agent's mode closest to read-only;
        // Improve must be able to submit reviewable edit_cell changes, so
        // it starts in the agent's own default.
        options.permission = match self {
            Self::Explain => crate::agents::composer::read_only_mode(kind),
            Self::Improve => crate::agents::composer::mode(kind, None).map(|m| m.permission),
        }
        .map(str::to_string);
        options
    }
}

impl Workbench {
    pub(crate) fn notebook_ai_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let button = icon_button(
            "notebook-ai-trigger",
            "sparkle.ai",
            &p,
            self.notebook_ai_menu,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new("Ask AI about this notebook")
                .build(window, cx)
        })
        .on_prepaint(self.popup_triggers.track("notebook-ai"))
        .on_click(cx.listener(|this, _, _, cx| {
            this.notebook_ai_menu = !this.notebook_ai_menu;
            cx.notify();
        }));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when(self.notebook_ai_menu, |d| {
                let mut popup = crate::agents::composer_view::popup(&p)
                    .id("notebook-ai-menu")
                    .occlude()
                    .w(px(232.))
                    .text_size(px(look::small()))
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.notebook_ai_menu = false;
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }))
                    .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if !this.popup_triggers.contains("notebook-ai", event.position) {
                            this.notebook_ai_menu = false;
                            cx.notify();
                        }
                    }));
                for (id, label, action) in [
                    (
                        "notebook-ai-explain",
                        "Explain this notebook",
                        NotebookAction::Explain,
                    ),
                    (
                        "notebook-ai-improve",
                        "Improve this notebook",
                        NotebookAction::Improve,
                    ),
                ] {
                    popup = popup.child(
                        div()
                            .id(id)
                            .px(px(10.))
                            .py(px(6.))
                            .cursor_pointer()
                            .hover(|d| d.bg(p.hover))
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.ask_agent_about_notebook(action, window, cx)
                            })),
                    );
                }
                d.child(
                    div().absolute().top(px(25.)).left_0().child(
                        deferred(anchored().snap_to_window_with_margin(px(8.)).child(popup))
                            .with_priority(3),
                    ),
                )
            })
            .into_any_element()
    }

    fn ask_agent_about_notebook(
        &mut self,
        action: NotebookAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notebook_ai_menu = false;
        let Some(open) = self
            .open
            .as_ref()
            .filter(|o| !o.is_script() && !o.notebook.is_text_document())
        else {
            return;
        };
        let path = open.notebook.path.clone();
        let relative = self
            .project
            .as_deref()
            .and_then(|p| {
                path.strip_prefix(p.trim_end_matches('/'))
                    .and_then(|s| s.strip_prefix('/'))
            })
            .unwrap_or(&path)
            .to_string();
        self.show_agents = true;
        if let Some(agents) = self.agents.clone() {
            window.defer(cx, move |window, cx| {
                agents.update(cx, |agents, cx| {
                    agents.start_notebook_conversation(action, relative, window, cx)
                });
            });
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentOptions, NotebookAction};

    #[test]
    fn explain_is_read_only_and_attaches_only_this_notebook() {
        let options = NotebookAction::Explain.options(
            "codex",
            AgentOptions {
                model: Some("selected-model".into()),
                code: Some("console".into()),
                ..Default::default()
            },
            "analysis.qmd",
        );
        assert_eq!(options.permission.as_deref(), Some("readonly"));
        let claude = NotebookAction::Explain.options("claude", Default::default(), "a.qmd");
        assert_eq!(claude.permission.as_deref(), Some("plan"));
        assert_eq!(options.model.as_deref(), Some("selected-model"));
        assert_eq!(options.documents, vec!["analysis.qmd"]);
        assert_eq!(options.refs[0]["path"], "analysis.qmd");
        assert_eq!(options.code, None);
        assert!(
            NotebookAction::Explain
                .prompt("analysis.qmd")
                .contains("do not edit files or run code")
        );
    }

    #[test]
    fn improve_requires_cell_review_and_bio_guides() {
        let prompt = NotebookAction::Improve.prompt("cells.ipynb");
        for text in [
            "cells.ipynb",
            "edit_cell",
            "Keep/Undo",
            "Do not rewrite the whole notebook",
            "single-cell",
            "built-in bioinformatics guides",
        ] {
            assert!(prompt.contains(text));
        }
        assert_eq!(
            NotebookAction::Improve
                .options("codex", Default::default(), "cells.ipynb")
                .permission
                .as_deref(),
            Some("supervised")
        );
    }
}
