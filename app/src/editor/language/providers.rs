use super::signature::{self, CallSite, Hint};
use super::virtual_doc::{Document, byte_position, component_position, position_byte, uri_path};
use crate::daemon::Daemon;
use crate::workbench::Workbench;
use anyhow::Result;
use gpui_kit::component::input::{
    CompletionProvider, DefinitionProvider, EditorState, HoverProvider, InputEvent, Rope,
    RopeExt as _, highlight_characters, set_completion_docs,
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

mod completion_items;
use completion_items::*;

/// After `ty` answers, how long the kernel's answer may still join it.
const KERNEL_GRACE: Duration = Duration::from_millis(300);
/// Calls whose first argument is an installed package's name.
const LIBRARY: &[&str] = &["library", "require", "requireNamespace"];
/// How long the first two letters of a plain word wait before the menu.
const TYPING_PAUSE: Duration = Duration::from_secs(3);
/// How long the pointer rests on a name before its explanation shows
/// (Huan, 2026-11-16).
const HOVER_PAUSE: Duration = Duration::from_secs(3);
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
            if !library {
                // The whole document's names, not only this cell's.
                let code = context.document.as_ref().map_or(text.as_str(), |d| &d.text);
                let mut names = assigned_names(code, r);
                if code != text {
                    names.extend(assigned_names(&text, r));
                }
                let _ = provider.workbench.read_with(cx, |this, _| {
                    names.extend(this.vars.list.iter().map(|v| v.name.clone()));
                });
                variables_first(&mut items, &names);
            }
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
    /// The explanation of the name under the pointer, once the pointer has
    /// rested on that name's letters for [`HOVER_PAUSE`]. Past the end of a
    /// line the editor counts the pointer as at the line's end, and the
    /// language server answers there with the whole list or call around it,
    /// so the pointer itself is checked against the name, and the box's area
    /// is the name, never the expression around it (Huan, 2026-11-16).
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let name = text.word_range(offset);
        let text = text.clone();
        let provider = self.clone();
        let window = window.window_handle();
        cx.spawn(async move |cx| {
            let Some(name) = name else {
                return Ok(None);
            };
            cx.background_executor().timer(HOVER_PAUSE).await;
            let on_name = window
                .update(cx, |_, window, cx| {
                    let pointer = window.mouse_position();
                    provider.editor.upgrade().is_some_and(|editor| {
                        editor
                            .read(cx)
                            .range_to_bounds(&name)
                            .is_some_and(|bounds| bounds.contains(&pointer))
                    })
                })
                .unwrap_or(false);
            if !on_name {
                return Ok(None);
            }
            let work = cx.update(|cx| provider.request(&text, offset, "textDocument/hover", cx));
            let (value, _, _) = work.await?;
            let mut hover = serde_json::from_value::<Option<Hover>>(value).unwrap_or(None);
            if let Some(hover) = &mut hover {
                // The editor then takes the name under the pointer.
                hover.range = None;
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
    /// editor or the Console opens completions with their documentation at
    /// once (typing waits three seconds for it); elsewhere it indents.
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
        gpui_kit::component::input::show_completion_docs_now(cursor);
        editor.update(cx, |editor, cx| editor.show_completions(window, cx));
        // An open menu shows its documentation now.
        window.refresh();
        true
    }
}
