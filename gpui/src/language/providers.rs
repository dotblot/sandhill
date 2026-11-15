use super::signature::{self, CallSite, Hint};
use super::virtual_doc::{Document, byte_position, component_position, position_byte, uri_path};
use crate::daemon::Daemon;
use crate::workbench::Workbench;
use anyhow::Result;
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, EditorState, HoverProvider, InputEvent, Rope,
    highlight_characters, set_completion_docs,
};
use gpui_kit::*;
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, Hover, InsertTextFormat, Location, LocationLink, MarkupContent, MarkupKind,
    Position, Range, TextEdit,
};
use sandhill_proto::{Call, CompletionReply, HelpPage, InspectReply, Packages};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

/// After `ty` answers, how long the kernel's answer may still join it.
const KERNEL_GRACE: Duration = Duration::from_millis(300);
/// Calls whose first argument is an installed package's name.
const LIBRARY: &[&str] = &["library", "require", "requireNamespace"];
/// How long the first two letters of a plain word wait before the menu.
const TYPING_PAUSE: Duration = Duration::from_secs(3);
/// Accepts kept across answers (the menu may show an earlier one).
const MAX_ACCEPTS: usize = 512;
/// Documentation kept per editor before the store starts over.
const MAX_DOCS: usize = 512;
/// The key in a kernel item's `data` naming its help topic (`sc.AnnData`).
const HELP_TOPIC: &str = "sandhill_help";

#[derive(Clone)]
struct Provider {
    workbench: WeakEntity<Workbench>,
    editor: WeakEntity<EditorState>,
    text: Rc<RefCell<Rope>>,
    console: bool,
    /// Where the last answer's items put the cursor once accepted.
    accepts: Rc<RefCell<Vec<Accept>>>,
    /// Whether the editor's completion menu was open at its last notify.
    menu_open: Rc<Cell<bool>>,
    /// Bumped by every keystroke and request; a waiting show that sees
    /// another value was overtaken.
    pause: Rc<Cell<u64>>,
    /// Who answered the last completion request, asked again for the
    /// documentation of items that came without it.
    asked: Rc<RefCell<Option<Asked>>>,
    /// Documentation found for items (by `docs_key`); `None` when there
    /// is none, so it isn't asked again.
    docs: Rc<RefCell<HashMap<String, Option<String>>>>,
}

#[derive(Clone)]
struct Asked {
    daemon: Rc<Daemon>,
    project: String,
    document: Option<Document>,
    session: Option<String>,
    language: String,
}

/// An item whose text Kit inserts with the cursor at its end although the
/// cursor belongs inside (`library(|)`): its replacement of `start..end` in
/// the requested text, and the cursor's byte in `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Accept {
    start: usize,
    end: usize,
    text: String,
    cursor: usize,
}

struct Context {
    daemon: Rc<Daemon>,
    project: String,
    document: Option<Document>,
    cell: usize,
    session: Option<String>,
    generation: u64,
    /// `r` or `python` (the Console's may be another kernel language).
    language: String,
}

pub(super) fn install(
    editor: &Entity<EditorState>,
    workbench: WeakEntity<Workbench>,
    console: bool,
    cx: &mut gpui_kit::Context<Workbench>,
) -> Subscription {
    let text = Rc::new(RefCell::new(editor.read(cx).text().clone()));
    let cache = text.clone();
    let accepts = Rc::new(RefCell::new(Vec::new()));
    let pending = accepts.clone();
    let subscription = cx.subscribe(
        editor,
        move |this, editor, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                let state = editor.read(cx);
                let text = state.text().clone();
                let cursor = accepted(
                    &mut pending.borrow_mut(),
                    &cache.borrow(),
                    &text,
                    state.cursor(),
                );
                *cache.borrow_mut() = text;
                // Kit has just inserted an accepted item with the cursor at
                // its end; Kit has no hook to place it, so place it now.
                if let Some(cursor) = cursor {
                    editor.update(cx, |editor, cx| {
                        editor.set_selected_range(cursor..cursor, cx)
                    });
                }
            }
            InputEvent::Blur => this.blur_signature(editor.entity_id(), cx),
            _ => {}
        },
    );
    let menu_open = Rc::new(Cell::new(false));
    let menu = menu_open.clone();
    // Kit notifies on every edit and cursor move; the hint follows both.
    let follow = cx.observe(editor, move |this, editor, cx| {
        menu.set(editor.read(cx).completion_menu_state().open);
        this.follow_signature(&editor, console, cx)
    });
    let provider = Rc::new(Provider {
        workbench: workbench.clone(),
        editor: editor.downgrade(),
        text,
        console,
        accepts,
        menu_open,
        pause: Rc::new(Cell::new(0)),
        asked: Rc::new(RefCell::new(None)),
        docs: Rc::new(RefCell::new(HashMap::new())),
    });
    // The completion menu asks for the documentation items came without.
    let id = editor.entity_id();
    let documents = provider.clone();
    set_completion_docs(
        id,
        Some(Rc::new(move |item: &CompletionItem, cx: &mut App| {
            documents.docs(item, cx)
        })),
    );
    cx.observe_release(editor, move |_, _, _| set_completion_docs(id, None))
        .detach();
    editor.update(cx, |editor, _| {
        let lsp = editor.lsp_mut();
        lsp.completion_provider = Some(provider.clone());
        lsp.set_completion_highlighter(Rc::new(highlight));
        if !console {
            lsp.hover_provider = Some(provider.clone());
            lsp.definition_provider = Some(provider);
            lsp.show_document = Some(Rc::new(move |params, window, cx| {
                if uri_path(params.uri.as_str()).is_none() {
                    return false;
                }
                let workbench = workbench.clone();
                let uri = params.uri.to_string();
                let selection = params.selection.unwrap_or_default();
                // Component calls this while its editor is mutably borrowed.
                window.defer(cx, move |window, cx| {
                    let _ = workbench.update(cx, |this, cx| {
                        let target = this
                            .language
                            .documents
                            .values()
                            .find(|doc| doc.uri == uri)
                            .cloned();
                        if let Some(document) = target {
                            if let (Some((cell, start)), Some((end_cell, end))) = (
                                document.local(selection.start),
                                document.local(selection.end),
                            ) {
                                this.show_file_tab(&document.path, cx);
                                let line = document
                                    .cells
                                    .iter()
                                    .find(|source| source.cell == cell)
                                    .map_or(0, |source| {
                                        byte_position(&source.text, start).line as usize
                                    });
                                this.jump_to_source(
                                    cell,
                                    line,
                                    Some(start..if cell == end_cell { end } else { start }),
                                    window,
                                    cx,
                                );
                            }
                        } else if let Some(path) = uri_path(&uri) {
                            // open_file_location takes Unicode columns rather than
                            // UTF-16. Convert once text is available after opening.
                            this.open_file_location(
                                path.clone(),
                                Some(selection.start.line + 1),
                                None,
                                window,
                                cx,
                            );
                            this.language.pending_definition = Some((path, selection));
                            this.take_language_definition(window, cx);
                        }
                    });
                });
                true
            }));
        }
    });
    Subscription::join(subscription, follow)
}

/// What a request needs, if `text` is still the editor's: its document for
/// the language server and the session whose kernel answers too.
fn context(
    workbench: &WeakEntity<Workbench>,
    editor: &WeakEntity<EditorState>,
    console: bool,
    text: &str,
    cx: &mut App,
) -> Option<Context> {
    let editor = editor.upgrade()?;
    workbench
        .update(cx, |this, cx| {
            let daemon = this.daemon()?;
            let project = this.project.clone()?;
            if editor.read(cx).text() != text {
                return None;
            }
            if console {
                let ark = this
                    .loose_kernel
                    .as_deref()
                    .is_some_and(|kernel| kernel.to_ascii_lowercase().contains("ark"));
                let language = this.console_language().to_string();
                let target = this.console_target().map(str::to_string);
                // Ark answers `complete_request` with nothing; its language
                // server sees the live session, so the input goes there as a
                // one-cell document of its own.
                let document = target
                    .clone()
                    .filter(|_| ark && language == "r")
                    .map(|session| {
                        Document::new(
                            &format!("{project}/.sandhill/console"),
                            "r",
                            false,
                            Some(session),
                            [(0, text.to_string())],
                        )
                    });
                return Some(Context {
                    daemon,
                    project,
                    document,
                    cell: 0,
                    session: target.filter(|_| !ark),
                    generation: this.generation,
                    language,
                });
            }
            let open = this
                .open
                .iter()
                .chain(&this.others)
                .find(|open| open.editors.contains(&editor))?;
            let cell = open
                .editors
                .iter()
                .position(|candidate| candidate == &editor)?;
            let language = open.notebook.cell_language(cell)?.to_string();
            if !matches!(language.as_str(), "r" | "python") {
                return None;
            }
            let path = open.notebook.path.clone();
            let ark = open
                .kernel
                .as_ref()
                .is_some_and(|kernel| kernel.to_ascii_lowercase().contains("ark"));
            let document = this.language_document(&path, &language, Some((&editor, text)), cx)?;
            let session = (!ark).then(|| document.session.clone()).flatten();
            this.language
                .documents
                .insert((path, language.clone()), document.clone());
            Some(Context {
                daemon,
                project,
                document: Some(document),
                cell,
                session,
                generation: this.generation,
                language,
            })
        })
        .ok()
        .flatten()
}

impl Provider {
    /// The documentation of `item` (which came without it), as markdown: a
    /// kernel item's from its help page, a language server item's from the
    /// server (`completionItem/resolve`; Ark sends R's help only so).
    /// `None` when the item can't be asked about.
    fn docs(&self, item: &CompletionItem, cx: &mut App) -> Option<Task<Option<String>>> {
        let key = docs_key(item)?;
        if let Some(known) = self.docs.borrow().get(&key) {
            return Some(Task::ready(known.clone()));
        }
        let asked = self.asked.borrow().clone()?;
        let topic = item
            .data
            .as_ref()
            .and_then(|data| data.get(HELP_TOPIC))
            .and_then(Value::as_str)
            .map(str::to_string);
        let item = item.clone();
        let store = self.docs.clone();
        Some(cx.spawn(async move |_| {
            let found = match topic {
                Some(topic) => {
                    let page = asked
                        .daemon
                        .call_within::<HelpPage>(
                            Call::HelpPage {
                                session: asked.session?,
                                topic,
                                package: None,
                            },
                            Duration::from_secs(3),
                        )
                        .await;
                    page.map(|page| help_markdown(&page))
                }
                None => {
                    let document = asked.document?;
                    let transport = asked.daemon.language.clone();
                    let params = serde_json::to_value(&item).ok()?;
                    let python = asked.language == "python";
                    asked
                        .daemon
                        .language_task(async move {
                            transport
                                .request(asked.project, document, "completionItem/resolve", params)
                                .await
                        })
                        .await
                        .map(|value| {
                            serde_json::from_value::<CompletionItem>(value)
                                .ok()
                                .and_then(|mut resolved| {
                                    if python {
                                        plain_docs_as_markdown(&mut resolved);
                                    }
                                    docs_markdown(&resolved)
                                })
                        })
                }
            };
            // A busy kernel or a slow server may answer next time.
            let found = found.ok()?;
            let mut store = store.borrow_mut();
            if store.len() >= MAX_DOCS {
                store.clear();
            }
            store.insert(key, found.clone());
            found
        }))
    }

    /// Gives items without documentation what was found for them before,
    /// so the menu shows it at once.
    fn known_docs(&self, items: &mut [CompletionItem]) {
        let store = self.docs.borrow();
        for item in items.iter_mut().filter(|item| item.documentation.is_none()) {
            if let Some(known) = docs_key(item).and_then(|key| store.get(&key)) {
                item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: known.clone().unwrap_or_default(),
                }));
            }
        }
    }

    fn context(&self, text: &str, cx: &mut App) -> Option<Context> {
        context(&self.workbench, &self.editor, self.console, text, cx)
    }

    fn request(
        &self,
        text: &Rope,
        offset: usize,
        method: &'static str,
        cx: &mut App,
    ) -> Task<Result<(Value, Option<Document>, usize)>> {
        let text = text.to_string();
        let provider = self.clone();
        cx.spawn(async move |cx| {
            // Run after the input action returns. Programmatic insertions may
            // originate inside a Workbench update as well as an editor update.
            // Kit asks both the symbol-hover and diagnostic presenters for the
            // same pointer location. Let its diagnostic bubble own a squiggle.
            if method == "textDocument/hover"
                && cx.update(|cx| {
                    provider.editor.upgrade().is_some_and(|editor| {
                        editor
                            .read(cx)
                            .diagnostics()
                            .is_some_and(|set| set.for_offset(offset).is_some())
                    })
                })
            {
                return Ok((Value::Null, None, 0));
            }
            let Some(context) = cx.update(|cx| provider.context(&text, cx)) else {
                return Ok((Value::Null, None, 0));
            };
            let Some(document) = context.document else {
                return Ok((Value::Null, None, 0));
            };
            let Some(position) = document.position(context.cell, offset) else {
                return Ok((Value::Null, None, 0));
            };
            let transport = context.daemon.language.clone();
            let sent = document.clone();
            let key = (document.path.clone(), document.language.clone());
            let work = context.daemon.language_task(async move {
                transport
                    .request(context.project, sent, method, json!({"position":position}))
                    .await
            });
            let workbench = provider.workbench.clone();
            let result = work.await;
            let current = workbench
                .update(cx, |this, cx| {
                    let current = this.generation == context.generation
                        && this
                            .language
                            .revisions
                            .get(&document.path)
                            .copied()
                            .unwrap_or(0)
                            == document.revision
                        && this
                            .language
                            .documents
                            .get(&key)
                            .is_some_and(|current| current.text == document.text);
                    if current {
                        this.language_result(
                            key,
                            result.as_ref().err().map(ToString::to_string),
                            cx,
                        );
                    }
                    current
                })
                .unwrap_or(false);
            Ok((
                if current {
                    result.unwrap_or(Value::Null)
                } else {
                    Value::Null
                },
                Some(document),
                context.cell,
            ))
        })
    }
}

impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        // A request (typed, Tab, or a pause's end) overtakes a waiting show.
        self.pause.set(self.pause.get() + 1);
        let text = text.to_string();
        let provider = self.clone();
        cx.spawn(async move |cx| {
            let Some(context) = cx.update(|cx| provider.context(&text, cx)) else {
                return Ok(CompletionResponse::Array(vec![]));
            };
            *provider.asked.borrow_mut() = Some(Asked {
                daemon: context.daemon.clone(),
                project: context.project.clone(),
                document: context.document.clone(),
                session: context.session.clone(),
                language: context.language.clone(),
            });
            let offset = text.floor_char_boundary(offset.min(text.len()));
            let r = context.language == "r";
            let start = word_start(&text, offset, r);
            let call = signature::call_site(&text[..offset], r);
            let library = r
                && call.as_ref().is_some_and(|call| {
                    call.index == 0 && call.named.is_none() && LIBRARY.contains(&call.name.as_str())
                });
            let kernel = context.session.clone().map(|session| {
                context.daemon.call::<CompletionReply>(Call::Complete {
                    session,
                    code: text.clone(),
                    cursor_pos: text[..offset].chars().count(),
                })
            });
            let key = context
                .document
                .as_ref()
                .map(|doc| (doc.path.clone(), doc.language.clone()));
            let position = context
                .document
                .as_ref()
                .and_then(|document| document.position(context.cell, offset));
            let lsp = context
                .document
                .clone()
                .zip(position)
                .map(|(document, position)| {
                    let transport = context.daemon.language.clone();
                    let project = context.project.clone();
                    context.daemon.language_task(async move {
                        transport
                            .request(
                                project,
                                document,
                                "textDocument/completion",
                                json!({"position":position,"context":{"triggerKind":1}}),
                            )
                            .await
                    })
                });
            // Both are already asked. Show the server's answer once it comes,
            // with the kernel's if it follows within the grace; without a
            // server answer, wait for the kernel's own bound instead.
            let (result, reply) = context
                .daemon
                .language_task(async move {
                    let result = match lsp {
                        Some(lsp) => Some(lsp.await),
                        None => None,
                    };
                    let reply = match kernel {
                        Some(kernel) if result.as_ref().is_some_and(Result::is_ok) => {
                            tokio::time::timeout(KERNEL_GRACE, kernel)
                                .await
                                .ok()
                                .and_then(Result::ok)
                        }
                        Some(kernel) => kernel.await.ok(),
                        None => None,
                    };
                    Ok((result, reply))
                })
                .await?;
            let token = Range::new(
                component_position(&text, start),
                component_position(&text, offset),
            );
            let mut items = vec![];
            if let Some(result) = result {
                if let Some(key) = key {
                    let _ = provider.workbench.update(cx, |this, cx| {
                        if this.generation == context.generation
                            && this.language.documents.contains_key(&key)
                        {
                            this.language_result(
                                key,
                                result.as_ref().err().map(ToString::to_string),
                                cx,
                            );
                        }
                    });
                }
                if let Ok(value) = result
                    && let Ok(response) = serde_json::from_value::<CompletionResponse>(value)
                {
                    items = match response {
                        CompletionResponse::Array(items) => items,
                        CompletionResponse::List(list) => list.items,
                    };
                    if let (Some(document), Some(position)) = (&context.document, position) {
                        items.retain_mut(|item| {
                            map_completion(item, document, context.cell, position, token)
                        });
                    }
                    for item in &mut items {
                        if item.text_edit.is_none() {
                            item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                                range: token,
                                new_text: item
                                    .insert_text
                                    .clone()
                                    .unwrap_or_else(|| item.label.clone()),
                            }));
                        }
                        plain_docs_as_markdown(item);
                    }
                    items.retain_mut(|item| coalesce_edits(item, &text));
                }
            }
            if let Some(reply) = reply {
                items.extend(kernel_items(&text, reply, context.language == "python"));
            }
            // Ark offers installed packages in `library(`; other R kernels
            // don't, so ask the session for its list.
            if library
                && !items
                    .iter()
                    .any(|item| item.kind == Some(CompletionItemKind::MODULE))
                && let Some(session) = context.session.clone()
                && let Ok(listed) = context
                    .daemon
                    .call_within::<Packages>(
                        Call::ListPackages { session },
                        Duration::from_millis(1500),
                    )
                    .await
            {
                items.extend(listed.packages.into_iter().map(|package| CompletionItem {
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: token,
                        new_text: package.name.clone(),
                    })),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(package.title).filter(|title| !title.is_empty()),
                    label: package.name,
                    ..Default::default()
                }));
            }
            let mut items = rank(items, &text, offset, start, call.is_some(), library);
            let python = context.language == "python";
            let found: Vec<Accept> = items
                .iter_mut()
                .filter_map(|item| finish(item, &text, python))
                .collect();
            if !found.is_empty() {
                // The menu may still show an earlier answer's items.
                let mut accepts = provider.accepts.borrow_mut();
                accepts.extend(found);
                let excess = accepts.len().saturating_sub(MAX_ACCEPTS);
                accepts.drain(..excess);
            }
            provider.known_docs(&mut items);
            Ok(CompletionResponse::Array(items))
        })
    }

    fn is_completion_trigger(&self, offset: usize, new_text: &str, cx: &mut App) -> bool {
        // Kit asks before this edit's `Change` reaches the cache, so the
        // cache still holds everything before the insertion point.
        let text = self.text.borrow();
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let mut line: Vec<char> = text
            .chars_at(offset)
            .reversed()
            .take_while(|ch| *ch != '\n')
            .collect();
        line.reverse();
        let line: String = line.into_iter().collect();
        let length = text.len() + new_text.len();
        drop(text);
        self.pause.set(self.pause.get() + 1);
        if !trigger(&line, new_text) {
            return false;
        }
        // An open menu refines with every letter.
        if self.menu_open.get() {
            return true;
        }
        let continues = line
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_');
        let wait = if pauses(&line, new_text) {
            TYPING_PAUSE
        } else if !continues {
            return true;
        } else {
            // Kit would start this menu's query at this letter; Tab's entry
            // point starts it at the word's beginning.
            Duration::ZERO
        };
        let generation = self.pause.get();
        let pause = self.pause.clone();
        let editor = self.editor.clone();
        let typed = new_text.to_string();
        cx.spawn(async move |cx| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            cx.update(|cx| {
                if pause.get() == generation {
                    show_completions(&editor, offset, &typed, length, cx);
                }
            });
        })
        .detach();
        false
    }
}

/// Opens the menu as Tab does, if `editor` still reads as just after
/// `typed` went in at `offset` (a `length`-byte text) and has focus.
fn show_completions(
    editor: &WeakEntity<EditorState>,
    offset: usize,
    typed: &str,
    length: usize,
    cx: &mut App,
) {
    let Some(editor) = editor.upgrade() else {
        return;
    };
    let state = editor.read(cx);
    let cursor = offset + typed.len();
    let text = state.text();
    if text.len() != length
        || state.cursor() != cursor
        || !state.selected_range().is_empty()
        || !text.is_char_boundary(offset)
        || !text.is_char_boundary(cursor)
        || text.slice(offset..cursor) != typed
    {
        return;
    }
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, cx| {
            if editor.read(cx).focus_handle(cx).is_focused(window) {
                editor.update(cx, |editor, cx| editor.show_completions(window, cx));
            }
        });
    }
}

impl HoverProvider for Provider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let work = self.request(text, offset, "textDocument/hover", cx);
        cx.spawn(async move |_| {
            let (value, document, cell) = work.await?;
            let mut hover = serde_json::from_value::<Option<Hover>>(value).unwrap_or(None);
            if let (Some(hover), Some(document)) = (&mut hover, document) {
                hover.range = hover
                    .range
                    .and_then(|range| document.component_range(cell, range));
            }
            Ok(hover)
        })
    }
}

impl DefinitionProvider for Provider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let work = self.request(text, offset, "textDocument/definition", cx);
        cx.spawn(async move |_| {
            let (value, document, cell) = work.await?;
            let values = match value {
                Value::Array(values) => values,
                Value::Null => vec![],
                value => vec![value],
            };
            let links = values
                .into_iter()
                .filter_map(|value| {
                    let mut link = serde_json::from_value::<LocationLink>(value.clone())
                        .ok()
                        .or_else(|| {
                            serde_json::from_value::<Location>(value)
                                .ok()
                                .map(|location| LocationLink {
                                    origin_selection_range: None,
                                    target_uri: location.uri,
                                    target_range: location.range,
                                    target_selection_range: location.range,
                                })
                        })?;
                    if let Some(document) = &document {
                        link.origin_selection_range = link
                            .origin_selection_range
                            .and_then(|range| document.component_range(cell, range));
                    }
                    Some(link)
                })
                .collect();
            Ok(links)
        })
    }
}

/// IPython's experimental completion types (`function`, `class`…), by text.
fn kernel_kinds(metadata: &Value) -> HashMap<String, CompletionItemKind> {
    let Some(types) = metadata
        .get("_jupyter_types_experimental")
        .and_then(Value::as_array)
    else {
        return HashMap::new();
    };
    types
        .iter()
        .filter_map(|entry| {
            let kind = match entry.get("type")?.as_str()? {
                "function" => CompletionItemKind::FUNCTION,
                "class" => CompletionItemKind::CLASS,
                "module" => CompletionItemKind::MODULE,
                "keyword" => CompletionItemKind::KEYWORD,
                "instance" | "statement" | "param" => CompletionItemKind::VARIABLE,
                "property" => CompletionItemKind::PROPERTY,
                _ => return None,
            };
            Some((entry.get("text")?.as_str()?.to_string(), kind))
        })
        .collect()
}

/// Plain-text documentation (ty sends docstrings so) as markdown, so its
/// code shows as code and its prose keeps its sections and entries.
fn plain_docs_as_markdown(item: &mut CompletionItem) {
    let plain = match &item.documentation {
        Some(Documentation::String(text)) => text.clone(),
        Some(Documentation::MarkupContent(content)) if content.kind == MarkupKind::PlainText => {
            content.value.clone()
        }
        _ => return,
    };
    item.documentation = Some(Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: crate::doc_text::docstring_markdown(&plain),
    }));
}

/// The kernel's matches as items. A Python item names its help topic: the
/// dotted name it completes (`sc.` + `AnnData`), asked once it is selected.
fn kernel_items(text: &str, reply: CompletionReply, python: bool) -> Vec<CompletionItem> {
    let kinds = kernel_kinds(&reply.metadata);
    let scalar_byte = |scalar| {
        text.char_indices()
            .nth(scalar)
            .map_or(text.len(), |(byte, _)| byte)
    };
    let start = scalar_byte(reply.cursor_start);
    let range = Range::new(
        component_position(text, start),
        component_position(text, scalar_byte(reply.cursor_end)),
    );
    let owner_start = text[..start]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_' || *ch == '.')
        .last()
        .map_or(start, |(at, _)| at);
    let owner = &text[owner_start..start];
    reply
        .matches
        .into_iter()
        .map(|label| CompletionItem {
            kind: kinds.get(&label).copied(),
            data: (python && !owner.starts_with(|ch: char| ch == '.' || ch.is_ascii_digit()))
                .then(|| json!({ HELP_TOPIC: format!("{owner}{label}") })),
            label: label.clone(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: label,
            })),
            ..Default::default()
        })
        .collect()
}

/// What an item's documentation is stored under: its label and the data
/// its source needs to find it again. Items without data can't be asked.
fn docs_key(item: &CompletionItem) -> Option<String> {
    Some(format!("{}\u{0}{}", item.label, item.data.as_ref()?))
}

/// An item's documentation as markdown, if it has any.
fn docs_markdown(item: &CompletionItem) -> Option<String> {
    let text = match item.documentation.as_ref()? {
        Documentation::String(text) => text.clone(),
        Documentation::MarkupContent(content) => content.value.clone(),
    };
    (!text.trim().is_empty()).then_some(text)
}

/// A Python help page as markdown: how it is called, then its docstring.
fn help_markdown(page: &HelpPage) -> Option<String> {
    let mut parts = vec![];
    if let Some(signature) = page.signature.as_deref().filter(|sig| !sig.is_empty()) {
        parts.push(crate::doc_text::python_code_markdown(
            &crate::help::signature_lines(&page.topic, signature, 60),
        ));
    }
    if let Some(text) = page.text.as_deref().filter(|text| !text.trim().is_empty()) {
        parts.push(crate::doc_text::docstring_markdown(text));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Moves a server item into cell coordinates. A primary edit that leaves the
/// cell but spans the cursor (Ark's argument items can) replaces the word
/// being typed instead; one that misses the cursor drops the item.
fn map_completion(
    item: &mut CompletionItem,
    document: &Document,
    cell: usize,
    cursor: Position,
    token: Range,
) -> bool {
    let map = |range| document.component_range(cell, range);
    let spans = |range: Range| range.start <= cursor && cursor <= range.end;
    if let Some(edit) = &mut item.text_edit {
        match edit {
            CompletionTextEdit::Edit(edit) => {
                edit.range = match map(edit.range) {
                    Some(range) => range,
                    None if spans(edit.range) => token,
                    None => return false,
                };
            }
            CompletionTextEdit::InsertAndReplace(edit) => {
                match (map(edit.insert), map(edit.replace)) {
                    (Some(insert), Some(replace)) => {
                        edit.insert = insert;
                        edit.replace = replace;
                    }
                    _ if spans(edit.insert) || spans(edit.replace) => {
                        edit.insert = token;
                        edit.replace = token;
                    }
                    _ => return false,
                }
            }
        }
    }
    if let Some(edits) = &mut item.additional_text_edits {
        for edit in edits {
            let Some(range) = map(edit.range) else {
                return false;
            };
            edit.range = range;
        }
    }
    true
}

/// Kit inserts the primary edit only. Fold same-cell import edits into that
/// replacement so accepting a suggestion never silently loses its imports.
/// A snippet primary keeps its syntax; the plain text around it is escaped.
fn coalesce_edits(item: &mut CompletionItem, text: &str) -> bool {
    let Some(edits) = item.additional_text_edits.take() else {
        return true;
    };
    if edits.is_empty() {
        return true;
    }
    let primary = match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => edit.clone(),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => TextEdit {
            range: edit.replace,
            new_text: edit.new_text.clone(),
        },
        None => return false,
    };
    let snippet = item.insert_text_format == Some(InsertTextFormat::SNIPPET);
    let plain = |text: &str| {
        if snippet {
            text.replace('\\', "\\\\")
                .replace('$', "\\$")
                .replace('}', "\\}")
        } else {
            text.to_string()
        }
    };
    let mut edits: Vec<(TextEdit, bool)> = edits.into_iter().map(|edit| (edit, false)).collect();
    edits.push((primary, true));
    edits.sort_by_key(|(edit, _)| edit.range.start);
    if edits
        .windows(2)
        .any(|pair| pair[0].0.range.end > pair[1].0.range.start)
    {
        return false;
    }
    let offset = |position| component_byte(text, position);
    let range = Range::new(
        edits.first().unwrap().0.range.start,
        edits.last().unwrap().0.range.end,
    );
    let mut cursor = offset(range.start);
    let mut replacement = String::new();
    for (edit, primary) in edits {
        let start = offset(edit.range.start);
        let end = offset(edit.range.end);
        if start < cursor || end < start {
            return false;
        }
        replacement.push_str(&plain(&text[cursor..start]));
        if primary {
            replacement.push_str(&edit.new_text);
        } else {
            replacement.push_str(&plain(&edit.new_text));
        }
        cursor = end;
    }
    item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
        range,
        new_text: replacement,
    }));
    true
}

/// The byte at a component position (line, Unicode column) of `text`.
fn component_byte(text: &str, position: Position) -> usize {
    let start = text
        .split_inclusive('\n')
        .take(position.line as usize)
        .map(str::len)
        .sum::<usize>();
    let row = text[start..].split('\n').next().unwrap_or("");
    start
        + row
            .char_indices()
            .nth(position.character as usize)
            .map_or(row.len(), |(byte, _)| byte)
}

/// Where the name being typed at `offset` starts; R names include dots.
fn word_start(text: &str, offset: usize, r: bool) -> usize {
    text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_' || (r && *ch == '.'))
        .last()
        .map_or(offset, |(byte, _)| byte)
}

fn inserted_text(item: &CompletionItem) -> &str {
    match &item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => &edit.new_text,
        Some(CompletionTextEdit::InsertAndReplace(edit)) => &edit.new_text,
        None => item.insert_text.as_deref().unwrap_or(&item.label),
    }
}

/// Where the item's replacement starts, when that is on the cursor's line.
fn edit_start(item: &CompletionItem, text: &str, offset: usize) -> Option<usize> {
    let range = match item.text_edit.as_ref()? {
        CompletionTextEdit::Edit(edit) => edit.range,
        CompletionTextEdit::InsertAndReplace(edit) => edit.insert,
    };
    Some(component_byte(text, range.start))
        .filter(|start| *start <= offset && !text[*start..offset].contains('\n'))
}

/// `organism = ` (Ark) or `axis=` (`ty`, the kernel): a call's argument.
fn is_argument(text: &str) -> bool {
    text.trim_end()
        .strip_suffix('=')
        .map(str::trim_end)
        .is_some_and(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '.'))
        })
}

/// How `query` matches `name`, best first: exact-case prefix, any-case
/// prefix, word starts (`rN` → `rowNames`), then any subsequence.
fn match_tier(query: &str, name: &str) -> Option<u8> {
    if name.starts_with(query) {
        return Some(0);
    }
    let lower = |text: &str| {
        text.chars()
            .flat_map(char::to_lowercase)
            .collect::<Vec<_>>()
    };
    let (typed, name_lower) = (lower(query), lower(name));
    if name_lower.starts_with(&typed) {
        return Some(1);
    }
    let typed: Vec<char> = typed
        .into_iter()
        .filter(|ch| ch.is_alphanumeric())
        .collect();
    if typed.is_empty() {
        return None;
    }
    if abbreviates(&typed, &words(name)) {
        return Some(2);
    }
    let mut rest = name_lower.iter();
    typed
        .iter()
        .all(|ch| rest.any(|candidate| candidate == ch))
        .then_some(3)
}

/// A name's lowercase words: split at punctuation and lower-to-upper case.
fn words(name: &str) -> Vec<Vec<char>> {
    let mut words: Vec<Vec<char>> = Vec::new();
    let mut previous: Option<char> = None;
    for ch in name.chars() {
        if !ch.is_alphanumeric() {
            previous = None;
            continue;
        }
        if previous.is_none_or(|previous| ch.is_uppercase() && !previous.is_uppercase()) {
            words.push(Vec::new());
        }
        if let Some(word) = words.last_mut() {
            word.extend(ch.to_lowercase());
        }
        previous = Some(ch);
    }
    words
}

/// Whether `typed` reads as prefixes of successive words, in order.
fn abbreviates(typed: &[char], words: &[Vec<char>]) -> bool {
    if typed.is_empty() {
        return true;
    }
    words.iter().enumerate().any(|(at, word)| {
        let common = word.iter().zip(typed).take_while(|(a, b)| a == b).count();
        (1..=common)
            .rev()
            .any(|take| abbreviates(&typed[take..], &words[at + 1..]))
    })
}

/// Keeps the items that match what is typed and orders them by match
/// quality; a call's arguments (in `library(`, packages) come first among
/// prefix matches and among looser ones; then the server's `sortText`, then
/// shorter names. One item per resulting text, the language server's first.
fn rank(
    items: Vec<CompletionItem>,
    text: &str,
    offset: usize,
    start: usize,
    call: bool,
    library: bool,
) -> Vec<CompletionItem> {
    let mut seen = HashSet::new();
    let mut ranked: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let from = edit_start(&item, text, offset).unwrap_or(start);
            let line = text[..from].rfind('\n').map_or(0, |at| at + 1);
            let inserted = inserted_text(&item);
            // Ark's `library($0)` and the kernel's `library` are one item.
            let plain = match item.insert_text_format {
                Some(InsertTextFormat::SNIPPET) => snippet_text(inserted).0,
                _ => inserted.to_string(),
            };
            let plain = plain.strip_suffix("()").unwrap_or(&plain);
            if !seen.insert(format!("{}{plain}", &text[line..from])) {
                return None;
            }
            // An item that only inserts at the cursor (Ark's `...`) still has
            // to match the name typed before it.
            let query = &text[from.min(start)..offset];
            let name = item.filter_text.as_deref().unwrap_or(&item.label);
            let tier = match_tier(query, name).or_else(|| {
                let word = query.trim_start_matches(|ch: char| !ch.is_alphanumeric() && ch != '_');
                (word.len() < query.len())
                    .then(|| match_tier(word, name))
                    .flatten()
            })?;
            let first = if library {
                item.kind == Some(CompletionItemKind::MODULE)
            } else {
                call && (is_argument(&item.label) || is_argument(inserted))
            };
            Some(((tier >= 2, !first, tier), item))
        })
        .collect();
    ranked.sort_by(|(left, a), (right, b)| {
        left.cmp(right)
            // LSP: an item without `sortText` sorts by its label.
            .then_with(|| {
                let a_order = a.sort_text.as_deref().unwrap_or(&a.label);
                a_order.cmp(b.sort_text.as_deref().unwrap_or(&b.label))
            })
            .then_with(|| a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.cmp(&b.label))
    });
    ranked.into_iter().map(|(_, item)| item).collect()
}

/// Whether typing `inserted` after `line` (the current line up to it)
/// should ask for completions: `$ @ . [ :: ( ,` and every letter of a name,
/// from the first (`pauses` may hold the first two back); not in numbers
/// or comments.
fn trigger(line: &str, inserted: &str) -> bool {
    let Some(last) = inserted.chars().last() else {
        return false;
    };
    let typed = format!("{line}{inserted}");
    if in_comment(&typed) {
        return false;
    }
    let numeric = |text: &str| {
        text.chars()
            .rev()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .last()
            .is_some_and(|first| first.is_ascii_digit())
    };
    match last {
        '$' | '@' | '[' | '(' | ',' => true,
        ':' => typed.ends_with("::"),
        '.' => !numeric(&typed[..typed.len() - 1]),
        ch if ch.is_alphanumeric() || ch == '_' => !numeric(&typed),
        _ => false,
    }
}

fn in_comment(line: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in line.chars() {
        match quote {
            Some(_) if escaped => escaped = false,
            Some(_) if ch == '\\' => escaped = true,
            Some(close) if ch == close => quote = None,
            Some(_) => {}
            None if ch == '#' => return true,
            None if matches!(ch, '"' | '\'' | '`') => quote = Some(ch),
            None => {}
        }
    }
    false
}

/// Whether a letter typed after `line` waits for a pause before the menu:
/// the first two letters of a word, unless the word follows `. $ @ :: ( , [`
/// (`np.l` refines at once). Tab skips the wait.
fn pauses(line: &str, inserted: &str) -> bool {
    if !inserted
        .chars()
        .last()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
    {
        return false;
    }
    let typed = format!("{line}{inserted}");
    let word: Vec<char> = typed
        .chars()
        .rev()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    if word.len() >= 3 {
        return false;
    }
    let bytes: usize = word.iter().map(|ch| ch.len_utf8()).sum();
    let before = typed[..typed.len() - bytes].trim_end_matches([' ', '\t']);
    !(before.ends_with(['.', '$', '@', '(', ',', '[']) || before.ends_with("::"))
}

/// A snippet's text without its syntax, and the cursor's byte in it: at
/// `$0`, else at the lowest-numbered tab stop. Placeholders keep their
/// default, choices their first option, variables their default (if any).
fn snippet_text(snippet: &str) -> (String, Option<usize>) {
    let chars: Vec<char> = snippet.chars().collect();
    let mut text = String::new();
    let mut stops = Vec::new();
    let mut at = 0;
    snippet_part(&chars, &mut at, &mut text, &mut stops, false);
    let cursor = stops
        .iter()
        .find(|(stop, _)| *stop == 0)
        .or_else(|| stops.iter().filter(|(stop, _)| *stop > 0).min())
        .map(|(_, byte)| *byte);
    (text, cursor)
}

/// Text up to the end of `chars` (or a placeholder's `}` when `nested`).
fn snippet_part(
    chars: &[char],
    at: &mut usize,
    text: &mut String,
    stops: &mut Vec<(u32, usize)>,
    nested: bool,
) {
    while let Some(&ch) = chars.get(*at) {
        match ch {
            '\\' if matches!(chars.get(*at + 1), Some('$' | '}' | '\\')) => {
                text.push(chars[*at + 1]);
                *at += 2;
            }
            '}' if nested => {
                *at += 1;
                return;
            }
            '$' => {
                *at += 1;
                if !snippet_element(chars, at, text, stops) {
                    text.push('$');
                }
            }
            _ => {
                text.push(ch);
                *at += 1;
            }
        }
    }
}

/// `$1`, `${1}`, `${1:default}`, `${1|a,b|}`, `$name`, `${name:default}`
/// after a `$`; false (nothing consumed) when it is a literal `$`.
fn snippet_element(
    chars: &[char],
    at: &mut usize,
    text: &mut String,
    stops: &mut Vec<(u32, usize)>,
) -> bool {
    let start = *at;
    let scan = |at: &mut usize, keep: fn(&char) -> bool| {
        let from = *at;
        while chars.get(*at).is_some_and(keep) {
            *at += 1;
        }
        chars[from..*at].iter().collect::<String>()
    };
    let name_start = |ch: &char| ch.is_ascii_alphabetic() || *ch == '_';
    let name_char = |ch: &char| ch.is_ascii_alphanumeric() || *ch == '_';
    match chars.get(*at) {
        Some(ch) if ch.is_ascii_digit() => {
            let stop = scan(at, char::is_ascii_digit).parse().unwrap_or(u32::MAX);
            stops.push((stop, text.len()));
            true
        }
        Some(ch) if name_start(ch) => {
            scan(at, name_char);
            true
        }
        Some('{') => {
            *at += 1;
            let stop = match chars.get(*at) {
                Some(ch) if ch.is_ascii_digit() => {
                    Some(scan(at, char::is_ascii_digit).parse().unwrap_or(u32::MAX))
                }
                Some(ch) if name_start(ch) => {
                    scan(at, name_char);
                    None
                }
                _ => {
                    *at = start;
                    return false;
                }
            };
            match chars.get(*at) {
                Some('}' | ':') => {}
                Some('|') if stop.is_some() => {}
                _ => {
                    *at = start;
                    return false;
                }
            }
            if let Some(stop) = stop {
                stops.push((stop, text.len()));
            }
            match chars[*at] {
                '}' => *at += 1,
                ':' => {
                    *at += 1;
                    snippet_part(chars, at, text, stops, true);
                }
                _ => {
                    *at += 1;
                    let mut first = true;
                    while let Some(&ch) = chars.get(*at) {
                        match ch {
                            '\\' if matches!(
                                chars.get(*at + 1),
                                Some(',' | '|' | '\\' | '$' | '}')
                            ) =>
                            {
                                if first {
                                    text.push(chars[*at + 1]);
                                }
                                *at += 2;
                                continue;
                            }
                            ',' => first = false,
                            '|' if chars.get(*at + 1) == Some(&'}') => {
                                *at += 2;
                                break;
                            }
                            _ if first => text.push(ch),
                            _ => {}
                        }
                        *at += 1;
                    }
                }
            }
            true
        }
        _ => false,
    }
}

/// Makes an item insert plain text and says where the cursor belongs: a
/// snippet loses its syntax; a Python function, method or constructor gets
/// `()` (not in `import`, `def`, `class` or decorator lines); neither adds
/// `()` before an existing `(`.
fn finish(item: &mut CompletionItem, text: &str, python: bool) -> Option<Accept> {
    let range = match item.text_edit.as_ref()? {
        CompletionTextEdit::Edit(edit) => edit.range,
        CompletionTextEdit::InsertAndReplace(edit) => edit.replace,
    };
    let start = component_byte(text, range.start);
    let end = component_byte(text, range.end).max(start);
    let (mut plain, mut cursor) = if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
        item.insert_text_format = Some(InsertTextFormat::PLAIN_TEXT);
        snippet_text(inserted_text(item))
    } else {
        (inserted_text(item).to_string(), None)
    };
    let next = text[end..].chars().next();
    let line = &text[text[..start].rfind('\n').map_or(0, |at| at + 1)..start];
    if python
        && cursor.is_none()
        && matches!(
            item.kind,
            Some(
                CompletionItemKind::FUNCTION
                    | CompletionItemKind::METHOD
                    | CompletionItemKind::CONSTRUCTOR
            )
        )
        && !plain.contains('(')
        && !next.is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        && !["import ", "from ", "def ", "class ", "@"]
            .iter()
            .any(|prefix| line.trim_start().starts_with(prefix))
    {
        plain.push_str("()");
        cursor = Some(plain.len() - 1);
    }
    if next == Some('(') && plain.ends_with("()") && cursor == Some(plain.len() - 1) {
        plain.truncate(plain.len() - 2);
        cursor = None;
    }
    match item.text_edit.as_mut()? {
        CompletionTextEdit::Edit(edit) => edit.new_text.clone_from(&plain),
        CompletionTextEdit::InsertAndReplace(edit) => edit.new_text.clone_from(&plain),
    }
    let cursor = cursor.filter(|cursor| *cursor < plain.len())?;
    Some(Accept {
        start,
        end,
        text: plain,
        cursor,
    })
}

/// Where the cursor goes when the edit from `old` to `new` (cursor at
/// `cursor`) is Kit inserting one of `accepts`; clears them when it is.
fn accepted(accepts: &mut Vec<Accept>, old: &Rope, new: &Rope, cursor: usize) -> Option<usize> {
    let found = accepts.iter().find(|accept| {
        let end = accept.start + accept.text.len();
        accept.end <= old.len()
            && new.len() + (accept.end - accept.start) == old.len() + accept.text.len()
            && cursor == end
            && end <= new.len()
            && new.is_char_boundary(accept.start)
            && new.is_char_boundary(end)
            && new.slice(accept.start..end) == accept.text
    })?;
    let at = found.start + found.cursor;
    accepts.clear();
    Some(at)
}

/// Kit's query runs from where the menu opened (`.`, `(`, `$`…); paint the
/// typed name's characters where they matched.
fn highlight(query: &str, item: &CompletionItem) -> Vec<std::ops::Range<usize>> {
    let start = query
        .char_indices()
        .rev()
        .take_while(|(_, ch)| ch.is_alphanumeric() || matches!(ch, '_' | '.'))
        .last()
        .map_or(query.len(), |(at, _)| at);
    highlight_characters(query[start..].trim_start_matches('.'), item)
}

/// The signature of the call at `offset`: from the language server, else
/// from the kernel's `inspect` (the Python Console, or no server answer).
pub(super) fn signature_help(
    workbench: WeakEntity<Workbench>,
    editor: WeakEntity<EditorState>,
    console: bool,
    text: String,
    offset: usize,
    call: CallSite,
    cx: &mut App,
) -> Task<Option<Hint>> {
    cx.spawn(async move |cx| {
        let context = cx.update(|cx| context(&workbench, &editor, console, &text, cx))?;
        if let Some(document) = context.document.clone()
            && let Some(position) = document.position(context.cell, offset)
        {
            let transport = context.daemon.language.clone();
            let project = context.project.clone();
            let answer = context
                .daemon
                .language_task(async move {
                    transport
                        .request(
                            project,
                            document,
                            "textDocument/signatureHelp",
                            json!({"position":position}),
                        )
                        .await
                })
                .await;
            if let Ok(value) = answer
                && let Some(hint) = signature::from_lsp(value, &call)
            {
                return Some(hint);
            }
        }
        let session = context.session?;
        let cursor_pos = text[..call.name_end].chars().count();
        // A completion asked at the same `(` can still hold the kernel.
        for attempt in 0..4 {
            let inspect = context.daemon.call::<InspectReply>(Call::Inspect {
                session: session.clone(),
                code: text.clone(),
                cursor_pos,
            });
            match inspect.await {
                Ok(reply) => {
                    return reply
                        .found
                        .then(|| reply.data["text/plain"].as_str())
                        .flatten()
                        .and_then(|text| signature::from_inspect(text, &call));
                }
                Err(err) if attempt < 3 && format!("{err:#}").contains("kernel_busy") => {
                    cx.background_executor()
                        .timer(Duration::from_millis(150))
                        .await;
                }
                Err(_) => return None,
            }
        }
        None
    })
}

impl Workbench {
    pub(crate) fn take_language_definition(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) {
        let Some((path, range)) = self.language.pending_definition.clone() else {
            return;
        };
        let Some(open) = self.open.as_ref().filter(|open| open.notebook.path == path) else {
            return;
        };
        let Some((cell, line)) = open.notebook.source_location(range.start.line as usize) else {
            return;
        };
        let Some(editor) = open.editors.get(cell) else {
            return;
        };
        let text = editor.read(cx).text().to_string();
        let start = position_byte(
            &text,
            lsp_types::Position::new(line as u32, range.start.character),
        );
        self.language.pending_definition = None;
        self.jump_to_source(cell, line, Some(start..start), window, cx);
    }

    /// Tab after a name, `$`, `.`, `::`, `(`, `,` or `[` in an R or Python
    /// editor or the Console opens completions; elsewhere it indents.
    pub(crate) fn complete_on_tab(
        &mut self,
        window: &mut Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> bool {
        let focused = |editor: &Entity<EditorState>, cx: &App| {
            let state = editor.read(cx);
            matches!(state.language_name().as_ref(), "r" | "python")
                && state.focus_handle(cx).is_focused(window)
        };
        let editor = self
            .open
            .as_ref()
            .and_then(|open| open.editors.iter().find(|editor| focused(editor, cx)))
            .or_else(|| focused(&self.console_input, cx).then_some(&self.console_input))
            .cloned();
        let Some(editor) = editor else {
            return false;
        };
        let state = editor.read(cx);
        let cursor = state.cursor();
        let text = state.text();
        if !state.selected_range().is_empty()
            || cursor > text.len()
            || !text.is_char_boundary(cursor)
            || !text.chars_at(cursor).reversed().next().is_some_and(|ch| {
                ch.is_alphanumeric() || matches!(ch, '_' | '.' | '$' | '@' | ':' | '(' | ',' | '[')
            })
        {
            return false;
        }
        editor.update(cx, |editor, cx| editor.show_completions(window, cx));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionItem, CompletionItemKind, CompletionReply, CompletionTextEdit, Document,
        HELP_TOPIC, InsertTextFormat, Position, Range, TextEdit, Value, coalesce_edits, finish,
        kernel_items, map_completion, match_tier, pauses, rank, snippet_text, trigger,
    };
    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }
    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            ..Default::default()
        }
    }
    #[test]
    fn ranking_filters_and_orders_by_match() {
        let items = [
            "palette",
            "calibrate",
            "lib_rename",
            "LIBRARY_X",
            "libraryX_long",
            "library",
        ]
        .map(item)
        .to_vec();
        assert_eq!(
            labels(&rank(items, "x <- libr", 9, 5, false, false)),
            vec![
                "library",
                "libraryX_long",
                "LIBRARY_X",
                "lib_rename",
                "calibrate"
            ]
        );
        assert_eq!(match_tier("rN", "rowNames"), Some(2));
        assert_eq!(match_tier("rn", "rowNames"), Some(2));
        assert_eq!(match_tier("rwn", "rowNames"), Some(3));
        assert_eq!(match_tier("x", "rowNames"), None);
        // Ties keep the server's order, then shorter names.
        let mut late = item("mean");
        late.sort_text = Some("1".into());
        let mut early = item("median");
        early.sort_text = Some("0".into());
        assert_eq!(
            labels(&rank(
                vec![late, early, item("me")],
                "me",
                2,
                0,
                false,
                false
            )),
            vec!["median", "mean", "me"]
        );
        // An empty word keeps everything.
        assert_eq!(
            labels(&rank(vec![item("b"), item("a")], "x$", 2, 2, false, false)).len(),
            2
        );
        // An item inserted at the cursor must still match the typed name:
        // Ark's `...` in `seq(le|` is not offered.
        let dots = CompletionItem {
            label: "...".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(0, 6), Position::new(0, 6)),
                new_text: "...".into(),
            })),
            ..Default::default()
        };
        assert_eq!(
            labels(&rank(
                vec![dots, item("length")],
                "seq(le",
                6,
                4,
                true,
                false
            )),
            vec!["length"]
        );
    }
    #[test]
    fn arguments_and_packages_come_first() {
        let items = ["org.Hs.eg.db", "orgDb", "organism = ", "color = ", "order"]
            .map(item)
            .to_vec();
        // Inside `getKEGG(or|`: arguments lead each match group.
        assert_eq!(
            labels(&rank(items.clone(), "getKEGG(or", 10, 8, true, false)),
            vec!["organism = ", "order", "org.Hs.eg.db", "orgDb", "color = "]
        );
        // Just after `(`: every argument before every other name.
        assert_eq!(
            labels(&rank(items, "getKEGG(", 8, 8, true, false))[..2],
            ["color = ", "organism = "]
        );
        let mut package = item("ggplot2");
        package.kind = Some(CompletionItemKind::MODULE);
        assert_eq!(
            labels(&rank(
                vec![item("gg"), item("ggg"), package],
                "library(gg",
                10,
                8,
                true,
                true
            )),
            vec!["ggplot2", "gg", "ggg"]
        );
    }
    #[test]
    fn duplicate_results_keep_the_server_item() {
        let edit = |label: &str, start: u32, detail: &str| CompletionItem {
            label: label.into(),
            detail: Some(detail.into()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(0, start), Position::new(0, 5)),
                new_text: label.into(),
            })),
            ..Default::default()
        };
        let items = rank(
            vec![
                edit("mean", 3, "server"),
                edit("np.mean", 0, "kernel"),
                edit("np.median", 0, "kernel"),
            ],
            "np.me",
            5,
            3,
            false,
            false,
        );
        assert_eq!(labels(&items), vec!["mean", "np.median"]);
        assert_eq!(items[0].detail.as_deref(), Some("server"));
    }
    #[test]
    fn cell_mapping_keeps_items_spanning_the_cursor() {
        let document = Document::new(
            "/p/a.ipynb",
            "r",
            false,
            None,
            [(0, "x <- 1".to_string()), (1, "getKEGG(org".to_string())],
        );
        // Cell 1 starts on the virtual document's third line.
        let cursor = document.position(1, 11).unwrap();
        let token = Range::new(Position::new(0, 8), Position::new(0, 11));
        let item = |start: Position, end: Position| CompletionItem {
            label: "organism = ".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(start, end),
                new_text: "organism = ".into(),
            })),
            ..Default::default()
        };
        let mut across = item(Position::new(0, 0), Position::new(2, 11));
        assert!(map_completion(&mut across, &document, 1, cursor, token));
        let Some(CompletionTextEdit::Edit(edit)) = &across.text_edit else {
            panic!("expected edit");
        };
        assert_eq!(edit.range, token);
        let mut past = item(Position::new(2, 8), Position::new(2, 40));
        assert!(map_completion(&mut past, &document, 1, cursor, token));
        let mut inside = item(Position::new(2, 8), Position::new(2, 11));
        assert!(map_completion(&mut inside, &document, 1, cursor, token));
        let Some(CompletionTextEdit::Edit(edit)) = &inside.text_edit else {
            panic!("expected edit");
        };
        assert_eq!(edit.range, token);
        let mut elsewhere = item(Position::new(0, 0), Position::new(0, 1));
        assert!(!map_completion(&mut elsewhere, &document, 1, cursor, token));
    }
    #[test]
    fn automatic_trigger_rules() {
        for special in ["$", "@", ".", "[", "(", ","] {
            assert!(trigger("x", special));
        }
        assert!(trigger("pkg:", ":"));
        assert!(trigger("pkg", "::"));
        assert!(!trigger("pkg", ":"));
        // Plain words from their first letter.
        assert!(trigger("", "l"));
        assert!(trigger("x <- ", "l"));
        assert!(trigger("np.", "a"));
        assert!(trigger("a", "b"));
        assert!(trigger("名列", "名"));
        assert!(trigger("getKEGG(", "o"));
        assert!(!trigger("abc", " "));
        // Not in numbers or comments; a `#` in a string is no comment.
        assert!(!trigger("x <- 1", "2"));
        assert!(!trigger("x <- 1", "."));
        assert!(!trigger("1.", "5"));
        assert!(trigger("x1", "."));
        assert!(!trigger("# see ", "x"));
        assert!(!trigger("f(x) # ", "("));
        assert!(trigger("f(\"#\", ", "y"));
    }
    #[test]
    fn first_letters_of_a_plain_word_wait_for_a_pause() {
        assert!(pauses("", "l"));
        assert!(pauses("l", "i"));
        assert!(pauses("x <- ", "g"));
        assert!(!pauses("li", "b"));
        assert!(!pauses("x <- libr", "a"));
        // After a trigger character every letter refines at once.
        assert!(!pauses("np.", "l"));
        assert!(!pauses("np.l", "i"));
        assert!(!pauses("df$", "a"));
        assert!(!pauses("obj@", "s"));
        assert!(!pauses("stats::", "l"));
        assert!(!pauses("f(", "x"));
        assert!(!pauses("f(a, ", "b"));
        assert!(!pauses("x[", "i"));
        // Trigger characters themselves never wait.
        assert!(!pauses("np", "."));
        assert!(!pauses("f", "("));
    }
    #[test]
    fn snippets_become_text_with_the_cursor_at_their_stop() {
        assert_eq!(snippet_text("library($0)"), ("library()".into(), Some(8)));
        assert_eq!(
            snippet_text("ggplot2::ggplot($0)"),
            ("ggplot2::ggplot()".into(), Some(16))
        );
        assert_eq!(snippet_text("f($2, $1)"), ("f(, )".into(), Some(4)));
        assert_eq!(snippet_text("f(${1:x}, $0)"), ("f(x, )".into(), Some(5)));
        assert_eq!(
            snippet_text("f(${1:g(${2:y})})"),
            ("f(g(y))".into(), Some(2))
        );
        assert_eq!(snippet_text("f(${1|a,b|})"), ("f(a)".into(), Some(2)));
        assert_eq!(snippet_text("f(${1})"), ("f()".into(), Some(2)));
        assert_eq!(snippet_text("\\$x \\} \\\\"), ("$x } \\".into(), None));
        assert_eq!(snippet_text("a$ ${TM_FILENAME:n}$x"), ("a$ n".into(), None));
        assert_eq!(snippet_text("plain"), ("plain".into(), None));
        assert_eq!(snippet_text("名($0)"), ("名()".into(), Some(4)));
    }
    #[test]
    fn calls_get_parentheses_with_the_cursor_inside() {
        let edit = |new_text: &str, kind, snippet: bool, line: &str| {
            let at = line.find('|').unwrap();
            let text = line.replace('|', "");
            let start = super::word_start(&text, at, false);
            let mut item = CompletionItem {
                label: new_text.into(),
                kind: Some(kind),
                insert_text_format: snippet.then_some(InsertTextFormat::SNIPPET),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: Range::new(
                        Position::new(0, text[..start].chars().count() as u32),
                        Position::new(0, text[..at].chars().count() as u32),
                    ),
                    new_text: new_text.into(),
                })),
                ..Default::default()
            };
            let accept = finish(&mut item, &text, true);
            (
                super::inserted_text(&item).to_string(),
                accept.map(|accept| accept.cursor),
            )
        };
        let function = CompletionItemKind::FUNCTION;
        assert_eq!(
            edit("print", function, false, "pr|"),
            ("print()".into(), Some(6))
        );
        assert_eq!(
            edit("mean", CompletionItemKind::METHOD, false, "x = np.me|"),
            ("mean()".into(), Some(5))
        );
        // Not before an existing `(`, nor for other kinds.
        assert_eq!(
            edit("print", function, false, "pr|(x)"),
            ("print".into(), None)
        );
        assert_eq!(
            edit("DataFrame", CompletionItemKind::CLASS, false, "pd.Da|"),
            ("DataFrame".into(), None)
        );
        // Imports, definitions and decorators name functions without calling.
        assert_eq!(
            edit("path", function, false, "from os import pa|"),
            ("path".into(), None)
        );
        assert_eq!(
            edit("wraps", function, false, "@wr|"),
            ("wraps".into(), None)
        );
        // A snippet's own parentheses, dropped before an existing `(`.
        assert_eq!(
            edit("library($0)", function, true, "lib|"),
            ("library()".into(), Some(8))
        );
        assert_eq!(
            edit("library($0)", function, true, "lib|(x)"),
            ("library".into(), None)
        );
    }
    #[test]
    fn kernel_unicode_offsets_are_scalar_not_wire_units() {
        let items = kernel_items(
            "名😀abc",
            CompletionReply {
                matches: vec!["abcdef".into()],
                cursor_start: 2,
                cursor_end: 5,
                metadata: Value::Null,
            },
            false,
        );
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            panic!("expected edit");
        };
        assert_eq!(
            edit.range,
            Range::new(
                lsp_types::Position::new(0, 2),
                lsp_types::Position::new(0, 5)
            )
        );
    }
    #[test]
    fn python_kernel_items_name_the_dotted_topic_they_complete() {
        let topic = |text: &str, start: usize, label: &str| {
            let items = kernel_items(
                text,
                CompletionReply {
                    matches: vec![label.into()],
                    cursor_start: start,
                    cursor_end: text.chars().count(),
                    metadata: Value::Null,
                },
                true,
            );
            items[0].data.as_ref().map(|data| data[HELP_TOPIC].clone())
        };
        assert_eq!(topic("x = sc.Ann", 7, "AnnData"), Some("sc.AnnData".into()));
        assert_eq!(
            topic("print(sc.pp.ne", 12, "neighbors"),
            Some("sc.pp.neighbors".into())
        );
        assert_eq!(topic("sc.Ann", 0, "sc.AnnData"), Some("sc.AnnData".into()));
        // After an index or call, there is no name to look up.
        assert_eq!(topic("x[0].sh", 5, "shape"), None);
    }
    #[test]
    fn accepting_import_completion_preserves_all_edits() {
        let mut item = CompletionItem {
            label: "os".into(),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range: Range::new(
                    lsp_types::Position::new(1, 0),
                    lsp_types::Position::new(1, 2),
                ),
                new_text: "os".into(),
            })),
            additional_text_edits: Some(vec![TextEdit {
                range: Range::new(
                    lsp_types::Position::new(0, 0),
                    lsp_types::Position::new(0, 0),
                ),
                new_text: "import os\n".into(),
            }]),
            ..Default::default()
        };
        assert!(coalesce_edits(&mut item, "# comment\nos"));
        let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
            panic!("expected edit");
        };
        assert_eq!(edit.new_text, "import os\n# comment\nos");
        assert!(item.additional_text_edits.is_none());
    }
}
