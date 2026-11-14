//! Window-local language features. All analysis and kernel access stay daemon-side.
mod providers;
mod signature;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
pub(crate) mod transport;
mod virtual_doc;

use crate::workbench::Workbench;
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::*;
use lsp_types::Diagnostic;
use sandhill_proto::LspMessage;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use virtual_doc::Document;

#[derive(Default)]
pub(crate) struct Language {
    documents: HashMap<(String, String), Document>,
    changes: HashMap<String, Task<()>>,
    diagnostics: HashMap<String, Task<()>>,
    revisions: HashMap<String, u64>,
    last_edit: HashMap<String, Instant>,
    status: HashMap<(String, String), String>,
    subscriptions: HashMap<EntityId, (String, Subscription)>,
    console_subscription: Option<Subscription>,
    pending_definition: Option<(String, lsp_types::Range)>,
    leases: HashMap<String, Arc<AtomicBool>>,
    popover_editor: Option<Entity<EditorState>>,
    signature: signature::State,
}

impl Workbench {
    pub(crate) fn install_language(
        &mut self,
        editor: &Entity<EditorState>,
        path: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let subscription = providers::install(editor, cx.entity().downgrade(), path.is_none(), cx);
        if let Some(path) = path {
            self.language
                .subscriptions
                .insert(editor.entity_id(), (path.into(), subscription));
        } else {
            self.language.console_subscription = Some(subscription);
        }
    }
    /// Each notebook editor owns a Kit overlay host. Leaving an editor does
    /// not clear its diagnostic bubble, so keep one notebook-wide owner.
    pub(crate) fn clear_language_popovers(
        &mut self,
        except: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let current = except.and_then(|ix| self.open.as_ref()?.editors.get(ix).cloned());
        if let Some(previous) = self.language.popover_editor.take()
            && current.as_ref() != Some(&previous)
        {
            previous.update(cx, |state, cx| {
                state.clear_diagnostic_popover(cx);
                state.clear_hover_state(cx);
            });
        }
        if let Some(editor) = &current {
            let state = editor.read(cx);
            if state.diagnostic_popover().is_some() && state.hover_popover().is_some() {
                editor.update(cx, |state, cx| state.clear_hover_state(cx));
            }
        }
        self.language.popover_editor = current;
    }
    /// Build on the Mac only. A provider passes its rope because its editor is
    /// currently borrowed by the component; never reborrow that editor here.
    fn language_document(
        &mut self,
        path: &str,
        language: &str,
        replacement: Option<(&Entity<EditorState>, &str)>,
        cx: &App,
    ) -> Option<Document> {
        let alive = self
            .language
            .leases
            .entry(path.into())
            .or_insert_with(|| Arc::new(AtomicBool::new(true)))
            .clone();
        let open = self
            .open
            .iter()
            .chain(&self.others)
            .find(|open| open.notebook.path == path)?;
        let sources = open
            .editors
            .iter()
            .enumerate()
            .filter_map(|(ix, editor)| {
                if open.notebook.cell_language(ix) != Some(language) {
                    return None;
                }
                let text = match replacement {
                    Some((target, text)) if target == editor => text.to_string(),
                    _ => editor.read(cx).text().to_string(),
                };
                Some((ix, text))
            })
            .collect::<Vec<_>>();
        if sources.is_empty() {
            return None;
        }
        // A mixed-language document must never send Python requests to its R
        // session, or attach R analysis to its Python session.
        let session = open
            .session
            .id()
            .filter(|_| {
                open.kernel
                    .as_ref()
                    .and_then(|name| self.kernels.iter().find(|kernel| &kernel.name == name))
                    .is_some_and(|kernel| kernel.language.eq_ignore_ascii_case(language))
            })
            .map(str::to_string);
        let mut document = Document::new(path, language, open.is_script(), session, sources);
        document.alive = alive;
        document.revision = self.language.revisions.get(path).copied().unwrap_or(0);
        Some(document)
    }

    pub(crate) fn language_event(
        &mut self,
        path: &str,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Focus | InputEvent::Change) {
            return;
        }
        let edited = matches!(event, InputEvent::Change);
        if edited {
            *self.language.revisions.entry(path.into()).or_default() += 1;
            self.language.last_edit.insert(path.into(), Instant::now());
        }
        let path = path.to_string();
        let key = path.clone();
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            if edited {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.sync_language(&path, false, cx);
                }
            });
        });
        self.language.changes.insert(key, task);
    }

    pub(crate) fn sync_language(&mut self, path: &str, saved: bool, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        if self.restricted() {
            return;
        }
        let documents: Vec<_> = ["r", "python"]
            .into_iter()
            .filter_map(|language| self.language_document(path, language, None, cx))
            .collect();
        for language in ["r", "python"] {
            let key = (path.to_string(), language.to_string());
            if !documents
                .iter()
                .any(|document| document.language == language)
                && self.language.documents.remove(&key).is_some()
            {
                self.language.status.remove(&key);
                let transport = daemon.language.clone();
                let project = project.clone();
                let path = path.to_string();
                let language = language.to_string();
                let work = daemon.language_task(async move {
                    transport.close(project, Some(path), Some(language)).await
                });
                cx.spawn(async move |_, _| {
                    if let Err(error) = work.await {
                        tracing::warn!("Closing language document: {error:#}");
                    }
                })
                .detach();
            }
        }
        let generation = self.generation;
        for document in documents {
            let key = (path.to_string(), document.language.clone());
            self.language
                .documents
                .insert(key.clone(), document.clone());
            let transport = daemon.language.clone();
            let project = project.clone();
            let work =
                daemon.language_task(async move { transport.sync(project, document, saved).await });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    if this.generation != generation || !this.language.documents.contains_key(&key)
                    {
                        return;
                    }
                    this.language_result(key, result.as_ref().err().map(ToString::to_string), cx);
                });
            })
            .detach();
        }
    }

    fn language_result(
        &mut self,
        key: (String, String),
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(error) = error {
            self.language.status.insert(key, status_reason(&error));
        } else {
            self.language.status.remove(&key);
        }
        cx.notify();
    }

    pub(crate) fn language_status(&self, path: &str) -> Option<String> {
        let mut errors: Vec<_> = self
            .language
            .status
            .iter()
            .filter(|((file, _), _)| file == path)
            .map(|((_, language), reason)| {
                format!("{}: {reason}", if language == "r" { "R" } else { "Python" })
            })
            .collect();
        errors.sort();
        (!errors.is_empty()).then(|| errors.join("; "))
    }

    pub(crate) fn close_language(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        self.clear_language_popovers(None, cx);
        if let Some(path) = path {
            if let Some(alive) = self.language.leases.remove(path) {
                alive.store(false, Ordering::Release);
            }
            self.language.changes.remove(path);
            self.language
                .subscriptions
                .retain(|_, (file, _)| file != path);
            self.language.documents.retain(|(file, _), _| file != path);
            self.language.status.retain(|(file, _), _| file != path);
            self.language.revisions.remove(path);
            self.language.last_edit.remove(path);
            // Editors still exist while close is dispatched, so clear marks now.
            if let Some(open) = self.open_at(path) {
                for editor in &open.editors {
                    editor.update(cx, |editor, cx| {
                        if let Some(set) = editor.diagnostics_mut() {
                            set.clear();
                        }
                        cx.notify();
                    });
                }
            }
        } else {
            for alive in self.language.leases.values() {
                alive.store(false, Ordering::Release);
            }
            let console_subscription = self.language.console_subscription.take();
            self.language = Language {
                console_subscription,
                ..Language::default()
            };
        }
        if let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) {
            let transport = daemon.language.clone();
            let path = path.map(str::to_string);
            let work =
                daemon.language_task(async move { transport.close(project, path, None).await });
            cx.spawn(async move |_, _| {
                if let Err(error) = work.await {
                    tracing::warn!("Closing language document: {error:#}");
                }
            })
            .detach();
        }
    }

    pub(crate) fn on_lsp_message(&mut self, event: LspMessage, cx: &mut Context<Self>) {
        let method = event.message["method"].as_str().unwrap_or("");
        if method == "sandhill/lspExited" {
            let language = event.message["_sandhill"]["language"].as_str();
            let session = event.message["_sandhill"]["session"].as_str();
            for (key, document) in &self.language.documents {
                if language == Some(document.language.as_str())
                    && (document.language != "r" || session == document.session.as_deref())
                {
                    self.language.status.insert(
                        key.clone(),
                        "Language server exited; edit to restart".into(),
                    );
                }
            }
            cx.notify();
            return;
        }
        if method != "textDocument/publishDiagnostics" {
            return;
        }
        let params = &event.message["params"];
        let Some(uri) = params["uri"].as_str().map(str::to_string) else {
            return;
        };
        let Some(document) = self
            .language
            .documents
            .values()
            .find(|doc| doc.uri == uri)
            .cloned()
        else {
            return;
        };
        if event.message["_sandhill"]["textHash"].as_u64() != Some(document.text_hash()) {
            return;
        }
        if document.language == "r"
            && event.message["_sandhill"]["session"].as_str() != document.session.as_deref()
        {
            return;
        }
        let Ok(diagnostics) = serde_json::from_value::<Vec<Diagnostic>>(
            params.get("diagnostics").cloned().unwrap_or(Value::Null),
        ) else {
            return;
        };
        let revision = self
            .language
            .revisions
            .get(&document.path)
            .copied()
            .unwrap_or(0);
        let wait = self
            .language
            .last_edit
            .get(&document.path)
            .map_or(Duration::ZERO, |edited| {
                Duration::from_millis(500).saturating_sub(edited.elapsed())
            });
        let generation = self.generation;
        let key = uri.clone();
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation
                    || this
                        .language
                        .revisions
                        .get(&document.path)
                        .copied()
                        .unwrap_or(0)
                        != revision
                {
                    return;
                }
                if !this
                    .language
                    .documents
                    .values()
                    .any(|doc| doc.uri == uri && doc.text == document.text)
                {
                    return;
                }
                let Some(open) = this.open_at(&document.path) else {
                    return;
                };
                let mut mapped: HashMap<usize, Vec<Diagnostic>> = HashMap::new();
                for diagnostic in diagnostics {
                    for (cell, range) in document.split_range(diagnostic.range) {
                        let mut local = diagnostic.clone();
                        local.range = range;
                        mapped.entry(cell).or_default().push(local);
                    }
                }
                for source in &document.cells {
                    let Some(editor) = open.editors.get(source.cell) else {
                        continue;
                    };
                    let marks = mapped.remove(&source.cell).unwrap_or_default();
                    editor.update(cx, |editor, cx| {
                        if let Some(set) = editor.diagnostics_mut() {
                            set.clear();
                            for mark in marks {
                                set.push(mark);
                            }
                        }
                        cx.notify();
                    });
                }
            });
        });
        self.language.diagnostics.insert(key, task);
    }
}

fn status_reason(error: &str) -> String {
    if error.contains("ark_session_required") {
        "Start an Ark session for code intelligence".into()
    } else if error.contains("ark_lsp_unsupported") {
        "IRkernel has kernel completion only; use Ark for analysis".into()
    } else if error.contains("lsp_exited")
        || error.contains("exited")
        || error.contains("disconnected")
    {
        "Language server exited; edit to restart".into()
    } else if error.contains("not found") || error.contains("missing") || error.contains("install")
    {
        "Language server unavailable".into()
    } else {
        error
            .lines()
            .next()
            .unwrap_or("Language service unavailable")
            .chars()
            .take(150)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Workbench;
    use gpui_kit::component::highlighter::{Diagnostic, DiagnosticEntry};
    use gpui_kit::{AppContext, TestAppContext, WeakEntity, WindowOptions};
    use lsp_types::{Hover, HoverContents, MarkedString};
    use std::sync::Arc;

    #[gpui_kit::test]
    fn quarto_editors_share_one_bubble_and_restore_python_grammar(cx: &mut TestAppContext) {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            crate::register_r();
            crate::resource_manager::block_host(sandhill_link::LOCAL, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        rt,
                        sandhill_link::Target::Local,
                        None,
                        WeakEntity::new_invalid(),
                        window,
                        cx,
                    )
                })
            })
            .unwrap()
        });
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| {
                let notebook = crate::notebook::parse("/p/mixed.qmd",
                    "---\r\ntitle: Test\r\n---\r\n\r\n```{r}\r\nx <- 1\r\n```\r\n\r\n```{python}\r\nimport pandas as pd\r\ndf = pd.DataFrame({'x': [1]})\r\n```\r\n\r\n```{python}\r\nprint(df.shape)\r\n```\r\n").unwrap();
                this.show_notebook(notebook, 0, window, cx);
                let editors = this.open.as_ref().unwrap().editors.clone();
                assert_eq!(editors[1].read(cx).language_name(), "r");
                assert_eq!(editors[2].read(cx).language_name(), "python");
                assert_eq!(editors[3].read(cx).language_name(), "python");
                let document = this.language_document("/p/mixed.qmd", "python", None, cx).unwrap();
                assert_eq!(document.text, "import pandas as pd\r\ndf = pd.DataFrame({'x': [1]})\n\nprint(df.shape)");
                this.clear_language_popovers(Some(2), cx);
                editors[2].update(cx, |state, cx| {
                    state.present_diagnostic(
                        DiagnosticEntry {
                            range: 0..6, diagnostic: Diagnostic::default(),
                        }, cx,
                    );
                    state.present_hover(0..6, Hover {
                        contents: HoverContents::Scalar(MarkedString::String("symbol".into())),
                        range: None,
                    }, cx);
                });
                this.clear_language_popovers(Some(2), cx);
                assert!(editors[2].read(cx).diagnostic_popover().is_some());
                assert!(editors[2].read(cx).hover_popover().is_none());
                this.clear_language_popovers(Some(3), cx);
                assert!(editors[2].read(cx).diagnostic_popover().is_none());
                editors[3].update(cx, |state, cx| {
                    state.present_hover(0..5, Hover {
                        contents: HoverContents::Scalar(MarkedString::String("print".into())),
                        range: None,
                    }, cx);
                });
                this.clear_language_popovers(None, cx);
                assert!(editors[3].read(cx).hover_popover().is_none());
                // Restoring a Python chunk in an R-first document must not
                // reinitialize it with the document's first chunk's R grammar.
                this.delete_cell(2, cx);
                this.undo_delete_cell(window, cx);
                assert_eq!(this.open.as_ref().unwrap().editors[2].read(cx).language_name(), "python");
            });
        }).unwrap();
    }
}
