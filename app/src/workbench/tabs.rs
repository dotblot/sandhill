//! Workbench document tabs: opening, showing, editor wrap, saving and closing files.
use super::*;

impl Workbench {
    /// The open notebook with this path, shown or in another tab.
    pub(crate) fn open_at(&mut self, path: &str) -> Option<&mut Open> {
        if self.open.as_ref().is_some_and(|o| o.notebook.path == path) {
            return self.open.as_mut();
        }
        self.others.iter_mut().find(|o| o.notebook.path == path)
    }

    /// Shows the notebook open in another tab.
    pub(crate) fn show_file_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        self.reveal_middle();
        self.hide_object_explorer();
        self.notebook_ai_menu = false;
        self.explorer_shown = false;
        self.diff_shown = false;
        self.history_shown = false;
        if self.document_loads.contains_key(path) {
            self.put_away();
            self.opening_shown = Some(path.to_string());
            cx.notify();
            return;
        }
        self.opening_shown = None;
        if self.media.contains_key(path) {
            self.put_away();
            self.media_shown = Some(path.to_string());
            self.session_changed(cx);
            cx.notify();
            return;
        }
        self.media_shown = None;
        if self.open.as_ref().is_some_and(|o| o.notebook.path == path) {
            cx.notify();
            return;
        }
        let Some(ix) = self.others.iter().position(|o| o.notebook.path == path) else {
            return;
        };
        let next = self.others.remove(ix);
        self.put_away();
        self.notebook_scroll.set_offset(next.scroll);
        self.open = Some(next);
        self.session_changed(cx);
        cx.notify();
    }

    /// Whether the notebook in this tab has edits not saved.
    pub(crate) fn tab_dirty(&self, path: &str) -> bool {
        self.open
            .iter()
            .chain(&self.others)
            .any(|o| o.notebook.path == path && o.dirty)
    }

    /// Moves the shown notebook to the other tabs.
    pub(crate) fn put_away(&mut self) {
        self.opening_shown = None;
        self.hide_object_explorer();
        self.notebook_ai_menu = false;
        self.media_shown = None;
        self.history_shown = false;
        if let Some(mut open) = self.open.take() {
            open.scroll = self.notebook_scroll.offset();
            self.others.push(open);
        }
    }

    /// Closes a tab, saving its notebook first if it was edited.
    pub(crate) fn close_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.document_loads.remove(path).is_some() {
            if self
                .pending_history
                .as_ref()
                .is_some_and(|(file, _)| file == path)
            {
                self.pending_history = None;
            }
            self.tabs.retain(|tab| tab != path);
            if self.opening_shown.as_deref() == Some(path) {
                self.opening_shown = None;
                if let Some(next) = self.tabs.last().cloned() {
                    self.show_file_tab(&next, cx);
                }
            }
            cx.notify();
            return;
        }
        if self.close_media(path, cx) {
            return;
        }
        if self.open_at(path).is_none() {
            return;
        }
        if self.tab_dirty(path) {
            let Some(save) = self.save_file(path, cx) else {
                return;
            };
            let path = path.to_string();
            let generation = self.generation;
            let project = self.project.clone();
            cx.spawn(async move |this, cx| {
                save.await;
                this.update(cx, |this, cx| {
                    if this.generation == generation
                        && this.project == project
                        && !this.tab_dirty(&path)
                    {
                        this.close_tab(&path, cx);
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        // A notebook is closed by showing it first. Closing a tab that isn't
        // in view then brings back the document that was, and anything that
        // covered it (Data Explorer, diff, Object Explorer).
        let kept = self
            .active_document_path()
            .filter(|shown| *shown != path)
            .map(str::to_string);
        let (explorer, diff, objects) = (self.explorer_shown, self.diff_shown, self.objects.shown);
        self.show_file_tab(path, cx);
        self.close_notebook(cx);
        if let Some(kept) = kept {
            self.show_file_tab(&kept, cx);
        }
        self.explorer_shown = explorer;
        self.diff_shown = diff;
        self.objects.shown = objects;
    }

    pub(crate) fn open_file(&mut self, rel: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.reveal_middle();
        let path = format!("{project}/{rel}");
        if crate::editor::media::is_media(&path) {
            self.open_media(path, cx);
            return;
        }
        if self.open_at(&path).is_some() || self.document_loads.contains_key(&path) {
            self.show_file_tab(&path, cx);
            return;
        }
        if notebook::format_of(&path) == Some(Format::Delimited) {
            self.offer_delimited(rel, cx);
            return;
        }
        if crate::panes::viewer::is_html(&path) {
            self.open_html(path, rel, cx);
            return;
        }
        let format = notebook::format_of(&path);
        if matches!(format, None | Some(Format::Text | Format::Markdown)) {
            self.open_shell_or_external(path, rel, cx);
            return;
        }
        let generation = self.generation;
        let load = self.next_document_load;
        self.next_document_load += 1;
        self.document_loads.insert(path.clone(), load);
        self.put_away();
        self.explorer_shown = false;
        self.diff_shown = false;
        self.problem = None;
        self.tabs.push(path.clone());
        self.opening_shown = Some(path.clone());
        cx.notify();
        let runnable = matches!(
            format,
            Some(Format::Ipynb | Format::Quarto | Format::Script)
        );
        let saved =
            runnable.then(|| daemon.call::<FileOutputs>(Call::FileOutputs { file: path.clone() }));
        // Live sessions are attached to only in a trusted folder.
        let sessions = (runnable && !self.restricted())
            .then(|| daemon.call::<Vec<SessionInfo>>(Call::ListSessions));
        cx.spawn(async move |this, cx| {
            let read = daemon
                .read_document(path.clone(), Some(project.clone()), None)
                .await;
            // Parsing can be expensive even though the daemon strips outputs.
            // Only GPUI editor/view construction must stay on the UI thread.
            let read = cx
                .background_spawn(async move {
                    read.and_then(|file| {
                        Ok((notebook::parse(&file.path, &file.text)?, file.mtime_ms))
                    })
                })
                .await;
            let saved = match saved {
                Some(saved) => Some(saved.await),
                None => None,
            };
            let sessions = match sessions {
                Some(sessions) => sessions.await.unwrap_or_default(),
                None => Vec::new(),
            };
            this.update_in(cx, |this, window, cx| {
                if this.document_loads.get(&path) != Some(&load) {
                    return;
                }
                this.document_loads.remove(&path);
                if this.generation != generation
                    || this.project.as_deref() != Some(project.as_str())
                {
                    this.tabs.retain(|tab| tab != &path);
                    if this.opening_shown.as_deref() == Some(&path) {
                        this.opening_shown = None;
                    }
                    cx.notify();
                    return;
                }
                let selected = this.opening_shown.as_deref() == Some(&path);
                match read {
                    Ok((nb, mtime_ms)) => {
                        if this.open_at(&nb.path).is_some() {
                            return;
                        }
                        // A background load must not steal the user's new tab.
                        let previous = this.open.take();
                        if selected {
                            this.opening_shown = None;
                            this.notebook_scroll.set_offset(Point::default());
                        }
                        this.show_notebook(nb, mtime_ms, window, cx);
                        if let Some(saved) = saved {
                            match saved {
                                Ok(saved) => {
                                    this.show_saved_outputs(saved, &sessions, selected, cx)
                                }
                                Err(err) => {
                                    this.problem =
                                        Some(format!("Reading saved outputs failed: {err:#}"))
                                }
                            }
                        }
                        if selected {
                            this.session_changed(cx);
                            this.take_pending_location(window, cx);
                            this.finish_history_restore(window, cx);
                        } else {
                            this.others
                                .push(this.open.take().expect("document just constructed"));
                            this.open = previous;
                        }
                    }
                    Err(err) => {
                        if this
                            .pending_history
                            .as_ref()
                            .is_some_and(|(file, _)| file == &path)
                        {
                            this.pending_history = None;
                        }
                        this.tabs.retain(|tab| tab != &path);
                        if selected {
                            this.opening_shown = None;
                            if let Some(next) = this.tabs.last().cloned() {
                                this.show_file_tab(&next, cx);
                            }
                        }
                        this.problem = Some(format!("Opening {rel} failed: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens a project file and puts the cursor at a 1-based line and column.
    pub(crate) fn open_file_location(
        &mut self,
        path: String,
        line: Option<u32>,
        column: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let Some(rel) = path
            .strip_prefix(&format!("{project}/"))
            .map(str::to_string)
        else {
            return;
        };
        if self.open_at(&path).is_some() {
            self.show_file_tab(&path, cx);
            self.pending_location = Some((path, line, column));
            self.take_pending_location(window, cx);
            return;
        }
        self.pending_location = Some((path, line, column));
        self.open_file(rel, cx);
    }

    pub(crate) fn take_pending_location(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, line, column)) = self.pending_location.take() else {
            return;
        };
        let Some(open) = self.open.as_ref().filter(|open| open.notebook.path == path) else {
            return;
        };
        let line0 = line.unwrap_or(1).saturating_sub(1) as usize;
        let Some((cell, cell_line)) = open.notebook.source_location(line0) else {
            return;
        };
        let editor = open.editors[cell].clone();
        let offset = column.map(|column| {
            let text = editor.read(cx).value().to_string();
            let start = text
                .split_inclusive('\n')
                .take(cell_line)
                .map(str::len)
                .sum::<usize>();
            let row = text[start..].split('\n').next().unwrap_or("");
            let byte = row
                .char_indices()
                .nth(column.saturating_sub(1) as usize)
                .map_or(row.len(), |(byte, _)| byte);
            start + byte
        });
        self.jump_to_source(cell, cell_line, offset.map(|at| at..at), window, cx);
        self.take_language_definition(window, cx);
    }

    /// The editor that import code would be inserted into: a script, or the
    /// notebook's current code cell.
    pub(super) fn insertion_editor(&self) -> Option<Entity<EditorState>> {
        if self.objects.is_shown()
            || self.media_shown.is_some()
            || self.explorer_shown
            || self.diff_shown
        {
            return None;
        }
        let open = self.open.as_ref()?;
        if !open.notebook.can_run() {
            return None;
        }
        let cell = if open.single_editor() {
            0
        } else {
            open.current?
        };
        (open.notebook.cells.get(cell)?.kind == CellKind::Code)
            .then(|| open.editors.get(cell).cloned())
            .flatten()
    }

    pub(crate) fn can_insert_into_active_document(&self, _cx: &App) -> bool {
        self.insertion_editor().is_some()
    }

    /// Inserts text at the cursor of the active script or current code cell.
    pub(crate) fn insert_into_active_document(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.insertion_editor() else {
            return false;
        };
        editor.update(cx, |editor, cx| {
            editor.insert(text, window, cx);
            editor.focus(window, cx);
        });
        true
    }

    /// An editor for each cell of a document, and what follows their edits.
    pub(crate) fn cell_editors(
        &mut self,
        nb: &Notebook,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<Entity<EditorState>>, Vec<Subscription>) {
        let mut doc = DocKind::of(nb);
        let mut editors = Vec::with_capacity(nb.cells.len());
        let mut subscriptions = Vec::with_capacity(nb.cells.len() + 1);
        for (ix, cell) in nb.cells.iter().enumerate() {
            doc.language = nb.cell_language(ix).map(str::to_string);
            let (editor, subscription) =
                self.cell_editor(&doc, cell.kind, cell.source.clone(), window, cx);
            editors.push(editor);
            subscriptions.push(subscription);
        }
        (editors, subscriptions)
    }

    /// An editor for one cell of a document, and what follows its edits.
    pub(crate) fn cell_editor(
        &mut self,
        nb: &DocKind,
        kind: CellKind,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<EditorState>, Subscription) {
        let language = match kind {
            CellKind::Code => editor_language(nb.language.as_deref()),
            CellKind::Text => "markdown",
        };
        let wrap = editor_wrap_type(nb.format, kind, nb.language.as_deref())
            .is_some_and(crate::prefs::editor_soft_wrap);
        let script = nb.script;
        let editor = cx.new(|cx| {
            let editor = EditorState::new(window, cx)
                .language(language)
                .folding(true)
                .searchable(false)
                .soft_wrap(wrap);
            if script {
                // A script is one editor that scrolls itself.
                editor
                    .line_number(nb.language.is_some())
                    .default_value(source)
            } else {
                // The notebook scrolls, not the cell, and a cell is as
                // tall as its lines.
                editor
                    .line_number(false)
                    .scroll_beyond_last_line(Some(0))
                    .default_value(source)
            }
        });
        if script {
            // Local structural folds parse the complete file. A 50k-line
            // Python script took 278ms here, before its first editor appeared.
            let text = editor.read(cx).text().clone();
            let editor = editor.downgrade();
            cx.spawn(async move |_, cx| {
                let (text, folds) = cx
                    .background_spawn(async move {
                        let folds =
                            crate::editor::outline::fold_ranges(&text.to_string(), language);
                        (text, folds)
                    })
                    .await;
                editor
                    .update(cx, |editor, cx| {
                        if editor.text() == &text {
                            editor.apply_highlighter_fold_candidates(folds, cx);
                        }
                    })
                    .ok();
            })
            .detach();
        } else {
            editor.update(cx, |editor, cx| {
                let text = editor.value();
                editor.apply_highlighter_fold_candidates(
                    crate::editor::outline::fold_ranges(&text, language),
                    cx,
                );
            });
        }
        if kind == CellKind::Code && matches!(language, "r" | "python") {
            self.install_language(&editor, Some(&nb.path), cx);
            crate::editor::rainbow::attach(&editor, cx);
        }
        let path = nb.path.clone();
        let subscription = cx.subscribe(&editor, move |this, editor, event: &InputEvent, cx| {
            let Some(open) = this.open_at(&path) else {
                return;
            };
            // Cells move, so the editor says which cell this is.
            let Some(ix) = open.editors.iter().position(|e| *e == editor) else {
                return;
            };
            match event {
                InputEvent::Change => {
                    if open.notebook.truncated {
                        return;
                    }
                    open.dirty = true;
                    let language = open
                        .notebook
                        .cell_language(ix)
                        .unwrap_or("markdown")
                        .to_string();
                    editor.update(cx, |editor, cx| {
                        let text = editor.value();
                        editor.apply_highlighter_fold_candidates(
                            crate::editor::outline::fold_ranges(&text, &language),
                            cx,
                        );
                    });
                }
                // A text cell shows formatted again.
                InputEvent::Blur => {
                    let id = open.notebook.cells[ix].id.clone();
                    open.editing.remove(&id);
                }
                InputEvent::Focus => {}
                _ => return,
            }
            if matches!(event, InputEvent::Change) {
                this.refresh_find(cx);
            }
            if kind == CellKind::Code {
                this.language_event(&path, event, cx);
            }
            cx.notify();
        });
        (editor, subscription)
    }

    /// Wrap follows the current cell's language in mixed-language notebooks.
    pub(crate) fn editor_wrap_type(open: &Open) -> Option<&'static str> {
        let ix = open.current.unwrap_or(0);
        let cell = open.notebook.cells.get(ix)?;
        editor_wrap_type(
            open.notebook.format,
            cell.kind,
            open.notebook.cell_language(ix),
        )
    }

    pub(crate) fn toggle_editor_wrap(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.open.as_ref().and_then(Self::editor_wrap_type) else {
            return;
        };
        let wrap = !crate::prefs::editor_soft_wrap(kind);
        crate::shell::choices::choose(crate::shell::choices::Choice::Wrap(kind, wrap), cx);
    }

    pub(crate) fn apply_editor_wrap(
        &mut self,
        kind: &str,
        wrap: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for open in self.open.iter().chain(&self.others) {
            let mut apply = |notebook: &Notebook, editors: &[Entity<EditorState>]| {
                for (ix, editor) in editors.iter().enumerate() {
                    let Some(cell) = notebook.cells.get(ix) else {
                        continue;
                    };
                    if editor_wrap_type(notebook.format, cell.kind, notebook.cell_language(ix))
                        == Some(kind)
                    {
                        editor.update(cx, |editor, cx| editor.set_soft_wrap(wrap, window, cx));
                    }
                }
            };
            apply(&open.notebook, &open.editors);
            // The hidden notebook editors are reused when leaving Source view.
            if let Some((notebook, view)) = open
                .notebook
                .source_notebook()
                .zip(open.quarto_view.as_ref())
            {
                apply(notebook, &view.editors);
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_quarto_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self
            .open
            .as_ref()
            .filter(|open| open.notebook.format == Format::Quarto)
        else {
            return;
        };
        let source_mode = open.notebook.quarto_source();
        let sources = open.sources(cx);
        let result = if source_mode {
            open.notebook.as_quarto_notebook(&sources[0])
        } else {
            open.notebook
                .as_quarto_source(&sources)
                .map(|nb| (nb, Vec::new()))
        };
        let (nb, matched) = match result {
            Ok(result) => result,
            Err(error) => {
                self.problem = Some(format!("Switching Quarto view failed: {error:#}"));
                cx.notify();
                return;
            }
        };
        let reusable: HashMap<_, _> = if source_mode {
            let open = self.open.as_ref().expect("checked above");
            matched
                .iter()
                .filter_map(|(old, new)| {
                    let editor = open.quarto_view.as_ref()?.editors.get(*old)?;
                    (editor.read(cx).value().as_ref() == nb.cells[*new].source
                        && open
                            .notebook
                            .source_notebook()
                            .and_then(|nb| nb.cell_language(*old))
                            == nb.cell_language(*new))
                    .then(|| (*new, editor.clone()))
                })
                .collect()
        } else {
            HashMap::new()
        };
        let mut editors = Vec::with_capacity(nb.cells.len());
        let mut subscriptions = Vec::new();
        let mut doc = DocKind::of(&nb);
        for (ix, cell) in nb.cells.iter().enumerate() {
            if let Some(editor) = reusable.get(&ix) {
                editors.push(editor.clone());
            } else {
                doc.language = nb.cell_language(ix).map(str::to_string);
                let (editor, subscription) =
                    self.cell_editor(&doc, cell.kind, cell.source.clone(), window, cx);
                editors.push(editor);
                subscriptions.push(subscription);
            }
        }
        let open = self.open.as_mut().expect("checked above");
        let mut renames = HashMap::new();
        let (current, position) = if source_mode {
            let position = open.editors[0].read(cx).cursor_position();
            let location = nb.source_location(position.line as usize).unwrap_or((0, 0));
            if let Some(view) = open.quarto_view.take() {
                for (old, new) in &matched {
                    // Retain the journal association even if this cell was edited.
                    renames.insert(view.ids[*old].clone(), nb.cells[*new].id.clone());
                }
                open.cell_runs
                    .retain(|id, _| renames.contains_key(id) || id.starts_with("deleted-"));
                open.collapsed.retain(|id| renames.contains_key(id));
                open.editing.retain(|id| renames.contains_key(id));
                open.waiting.retain(|id| renames.contains_key(id));
                open.sending.retain(|id| renames.contains_key(id));
                self.notebook_scroll.set_offset(view.scroll);
                let current = (!nb.cells.is_empty())
                    .then_some(location.0)
                    .or(view.current.filter(|ix| *ix < nb.cells.len()));
                (
                    current,
                    Position::new(location.1 as u32, position.character),
                )
            } else {
                (
                    Some(location.0),
                    Position::new(location.1 as u32, position.character),
                )
            }
        } else {
            let current = open
                .current
                .unwrap_or(0)
                .min(open.editors.len().saturating_sub(1));
            let position = open
                .editors
                .get(current)
                .map(|editor| editor.read(cx).cursor_position())
                .unwrap_or_default();
            let line = match &nb.source_notebook() {
                Some(notebook) => notebook.cell_source_line(current),
                None => 0,
            };
            open.quarto_view = Some(QuartoView {
                editors: open.editors.clone(),
                current: open.current,
                scroll: self.notebook_scroll.offset(),
                ids: open
                    .notebook
                    .cells
                    .iter()
                    .map(|cell| cell.id.clone())
                    .collect(),
            });
            (
                Some(0),
                Position::new(line as u32 + position.line, position.character),
            )
        };
        open.notebook = nb;
        open.editors = editors;
        open.current = current;
        open._subscriptions.extend(subscriptions);
        if source_mode {
            self.quarto_sources.remove(&open.notebook.path);
        } else {
            self.quarto_sources.insert(open.notebook.path.clone());
        }
        let editor = current.and_then(|ix| open.editors.get(ix)).cloned();
        self.rename_quarto_cells(&renames);
        if let Some(editor) = editor {
            editor.update(cx, |editor, cx| {
                editor.set_cursor_position(position, window, cx);
                editor.focus(window, cx);
            });
        }
        self.refresh_find(cx);
        cx.notify();
    }

    pub(crate) fn show_notebook(
        &mut self,
        nb: Notebook,
        mtime_ms: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_middle();
        // A colouring chosen for this file on this Mac wins over its type's.
        let nb = match crate::prefs::text_colouring(&self.host(), &nb.path) {
            Some(choice) if nb.can_recolour() => nb.recoloured(&choice),
            _ => nb,
        };
        let project = self.project_kernel(nb.language.as_deref());
        let kernel = notebook::recover_kernel(
            &nb,
            &self.kernels,
            None,
            self.project_state["documents"][&nb.path]["kernel"].as_str(),
            project.as_deref(),
        );
        let (editors, mut subscriptions) = self.cell_editors(&nb, window, cx);
        if nb.format == Format::Text {
            subscriptions.extend(crate::editor::bio_text::decorate_editor(
                &editors[0],
                nb.language.as_deref(),
                cx,
            ));
        }
        if nb.single_editor() {
            // The action row's size and selection count follow the editor:
            // redraw on a new selection or length, not on caret blinks.
            let seen = std::cell::Cell::new((0..0, usize::MAX));
            subscriptions.push(cx.observe(&editors[0], move |_, editor, cx| {
                let editor = editor.read(cx);
                let now = (editor.selected_range(), editor.text().len());
                if seen.replace(now.clone()) != now {
                    cx.notify();
                }
            }));
        }
        let delimited_header = if nb.format == Format::Delimited {
            let (header, header_subscriptions) = crate::editor::delimited::decorate_editor(
                &editors[0],
                crate::editor::delimited::separator(nb.language.as_deref()),
                window,
                cx,
            );
            subscriptions.extend(header_subscriptions);
            Some(header)
        } else {
            None
        };
        self.open = Some(Open {
            notebook: nb,
            editors,
            dirty: false,
            mtime_ms,
            markdown_preview: false,
            md_images: Default::default(),
            quarto_view: None,
            delimited_header,
            kernel,
            session: Session::None,
            waiting: Vec::new(),
            sending: HashSet::new(),
            cell_runs: HashMap::new(),
            scroll: Point::default(),
            editing: HashSet::new(),
            collapsed: HashSet::new(),
            deleted: Vec::new(),
            agent_changes: HashMap::new(),
            agent_deleted: Vec::new(),
            lit: None,
            ghost: None,
            ghost_after: None,
            next_slot: 0,
            queued: Vec::new(),
            current: None,
            _subscriptions: subscriptions,
            history_edits: Default::default(),
        });
        if self.open.as_ref().is_some_and(|open| {
            open.notebook.format == Format::Quarto
                && self.quarto_sources.contains(&open.notebook.path)
        }) {
            self.toggle_quarto_view(window, cx);
        }
        self.problem = None;
    }

    /// Puts each cell's latest journaled run under it, and carries on in the
    /// kernel that ran them if it is still running.
    pub(super) fn show_saved_outputs(
        &mut self,
        saved: FileOutputs,
        sessions: &[SessionInfo],
        activate: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let latest = saved.cells.iter().max_by_key(|c| c.started_ms).cloned();
        // A script carries on after the slot its last run used.
        if let Some(n) = saved
            .cells
            .iter()
            .filter_map(|c| Some((c.started_ms, script_slot(&c.cell_id)?)))
            .max()
            .map(|(_, n)| n)
        {
            open.next_slot = (n + 1) % SCRIPT_SLOTS;
        }
        let cell_ids: HashSet<&str> = open
            .notebook
            .cells
            .iter()
            .map(|cell| cell.id.as_str())
            .collect();
        for cell in saved.cells {
            if !cell_ids.contains(cell.cell_id.as_str()) {
                continue;
            }
            self.runs.insert(
                cell.run.clone(),
                Run {
                    code: cell.code,
                    language: sessions
                        .iter()
                        .find(|session| session.id == cell.session)
                        .map(|session| editor_language(Some(&session.language))),
                    cell: Some((saved.file.clone(), cell.cell_id.clone())),
                    outputs: cell.outputs,
                    finished: cell.finished_ms.is_some(),
                    status: cell.status,
                    execution_count: cell.execution_count,
                    session: cell.session.clone(),
                    queued_ms: Some(cell.started_ms),
                    finished_ms: cell.finished_ms,
                    snapshot_seq: saved.seq,
                    last_seq: saved.seq,
                    ..Default::default()
                },
            );
            open.cell_runs.insert(cell.cell_id, cell.run);
        }
        let running = latest.and_then(|latest| {
            sessions
                .iter()
                .find(|s| s.id == latest.session && s.status == SessionStatus::Running)
        });
        if let Some(session) = running {
            open.kernel = notebook::recover_kernel(
                &open.notebook,
                &self.kernels,
                Some(&session.kernel),
                open.kernel.as_deref(),
                None,
            );
            open.session = Session::Ready(session.id.clone());
        }
        if let Some(session) = running {
            self.remember_document_kernel(&saved.file, &session.kernel);
            if activate {
                self.follow_console_session(session.id.clone(), cx);
            }
        }
    }

    /// Closes the open notebook, saving it first if it was edited. Its
    /// kernel keeps running, so reopening carries on where it left off.
    pub(crate) fn close_notebook(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self
            .open
            .as_ref()
            .filter(|o| o.dirty)
            .map(|o| o.notebook.path.clone())
        {
            self.close_tab(&path, cx);
            return;
        }
        if let Some(path) = self.open.as_ref().map(|open| open.notebook.path.clone()) {
            self.close_language(Some(&path), cx);
        }
        if let Some(closed) = self.open.take() {
            let ix = self.tabs.iter().position(|p| *p == closed.notebook.path);
            self.tabs.retain(|p| *p != closed.notebook.path);
            // The tab that was next to it, as browsers do.
            let next = ix
                .and_then(|ix| self.tabs.get(ix.min(self.tabs.len().saturating_sub(1))))
                .cloned();
            if let Some(next) = next {
                self.show_file_tab(&next, cx);
            }
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.open.as_ref().map(|o| o.notebook.path.clone())
            && let Some(task) = self.save_file(&path, cx)
        {
            task.detach();
        }
    }

    /// Saves every open document with unsaved changes (⌥⌘S).
    pub(crate) fn save_all(&mut self, cx: &mut Context<Self>) {
        for task in self.save_all_tasks(cx) {
            task.detach();
        }
    }

    /// Saves every edited document; the tasks end when they are written.
    pub(crate) fn save_all_tasks(&mut self, cx: &mut Context<Self>) -> Vec<Task<()>> {
        let dirty: Vec<String> = self
            .open
            .iter()
            .chain(&self.others)
            .filter(|o| o.dirty)
            .map(|o| o.notebook.path.clone())
            .collect();
        dirty
            .iter()
            .filter_map(|path| self.save_file(path, cx))
            .collect()
    }

    /// Whether any open document has unsaved changes.
    pub(crate) fn any_dirty(&self) -> bool {
        self.open.iter().chain(&self.others).any(|o| o.dirty)
    }

    /// How many open documents have unsaved changes.
    pub(crate) fn unsaved_count(&self) -> usize {
        self.open
            .iter()
            .chain(&self.others)
            .filter(|o| o.dirty)
            .count()
    }

    /// Writes one open document; the task ends when it is written.
    pub(crate) fn save_file(&mut self, path: &str, cx: &mut Context<Self>) -> Option<Task<()>> {
        let daemon = self.daemon();
        let open = self
            .open
            .iter()
            .chain(&self.others)
            .find(|o| o.notebook.path == path);
        let (Some(daemon), Some(open)) = (daemon, open) else {
            return None;
        };
        if open.notebook.truncated {
            return None;
        }
        let sources = open.sources(cx);
        let text = match open.notebook.render(&sources) {
            Ok(text) => text,
            Err(err) => {
                self.problem = Some(format!("Saving failed: {err:#}"));
                cx.notify();
                return None;
            }
        };
        let path = open.notebook.path.clone();
        let expected_mtime_ms = Some(open.mtime_ms);
        let write = match open.notebook.format {
            Format::Ipynb => Call::WriteNotebook {
                path: path.clone(),
                text: text.clone(),
                expected_mtime_ms,
            },
            Format::Quarto
            | Format::Script
            | Format::Markdown
            | Format::Text
            | Format::Delimited => Call::WriteFile {
                path: path.clone(),
                text: text.clone(),
                expected_mtime_ms,
                base64: false,
                create: false,
            },
        };
        let clear_outputs: Vec<_> = open
            .history_edits
            .clear_outputs
            .iter()
            .filter(|id| !open.cell_runs.contains_key(*id))
            .cloned()
            .collect();
        let written = daemon.call_in_project::<FileWritten>(self.project.clone(), write);
        let task = cx.spawn(async move |this, cx| {
            let written = written.await;
            let cleared = if written.is_ok() && !clear_outputs.is_empty() {
                Some(daemon.call::<usize>(Call::ClearCellOutputs {
                    file: path.clone(),
                    cell_ids: clear_outputs.clone(),
                }).await)
            } else {
                None
            };
            this.update(cx, |this, cx| {
                match written {
                    Ok(written) => {
                        if let Some(open) = this.open_at(&path) {
                            if matches!(&cleared, Some(Ok(_))) {
                                open.history_edits.clear_outputs.retain(|id| !clear_outputs.contains(id));
                            }
                            open.mtime_ms = written.mtime_ms;
                            let current = open.sources(cx);
                            open.dirty = current != sources
                                && open.notebook.render(&current).map_or(true, |current| current != text);
                            // Later saves start from what is on disk now.
                            // Unless the cells were added to or moved since.
                            let ids = |nb: &Notebook| {
                                nb.cells.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
                            };
                            match notebook::parse_as(&path, &text, open.notebook.format) {
                                Ok(_) if open.notebook.quarto_source() => {}
                                Ok(mut nb) if ids(&nb) == ids(&open.notebook) => {
                                    // A chosen colouring is the document's,
                                    // not the file name's.
                                    if nb.can_recolour() {
                                        nb.format = open.notebook.format;
                                        nb.language = open.notebook.language.clone();
                                    }
                                    // Cleared outputs stay cleared.
                                    open.notebook = nb
                                }
                                // Adjacent Quarto prose editors (including an
                                // empty added one) merge when parsed. Retain
                                // their byte spans and existing editor history.
                                Ok(_) if open.notebook.can_add_cells() => {}
                                _ => {
                                    this.problem = Some(
                                        "Saved. The cells changed shape; reopen the file to edit further."
                                            .into(),
                                    )
                                }
                            }
                        }
                        let saved = this.open_at(&path).is_some_and(|open| !open.dirty);
                        if let Some(Err(error)) = cleared {
                            this.problem = Some(format!("Saved, but could not clear earlier outputs: {error:#}"));
                        }
                        this.sync_language(&path, saved, cx);
                    }
                    Err(err) if format!("{err:#}").contains(CONFLICT_ERROR) => {
                        this.problem = Some(
                            "Not saved: the file changed on disk. Reopen it to see the change."
                                .into(),
                        )
                    }
                    Err(err) => this.problem = Some(format!("Saving failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        });
        Some(task)
    }

    /// Closes a document's tab. A live kernel or AI conversations about it
    /// bring up the close card (with the save question in it); otherwise
    /// only unsaved changes ask.
    pub(crate) fn ask_close(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_media(&path, cx) {
            return;
        }
        let Some(open) = self.open_at(&path) else {
            return;
        };
        let dirty = open.dirty;
        if let Some(ask) = self.close_ask(&path, dirty, cx) {
            self.ask_close_with(ask, window, cx);
            return;
        }
        if !dirty {
            self.close_tab(&path, cx);
            return;
        }
        let name = crate::panes::files::base_name(&path).to_string();
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Save changes to {name}?"),
            Some("Running code keeps running either way."),
            &["Save", "Don't save", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            let answer = answer.await;
            this.update(cx, |this, cx| match answer {
                Ok(0) => this.close_tab(&path, cx),
                Ok(1) => {
                    if let Some(open) = this.open_at(&path) {
                        open.dirty = false;
                    }
                    this.close_tab(&path, cx);
                }
                _ => {}
            })
            .ok();
        })
        .detach();
    }
}

/// CSV/TSV's coloured columns require an unwrapped display. Unknown code
/// languages stay distinct from plain text even when their highlighter is text.
fn editor_wrap_type(
    format: Format,
    kind: CellKind,
    language: Option<&str>,
) -> Option<&'static str> {
    if format == Format::Delimited {
        return None;
    }
    if kind == CellKind::Text || format == Format::Markdown {
        return Some("markdown");
    }
    if format == Format::Text {
        match language {
            Some("fasta") => return Some("fasta"),
            Some("fastq") => return Some("fastq"),
            Some("bio-table") => return Some("bio-table"),
            Some("tree") => return Some("tree"),
            Some("nextflow") => return Some("nextflow"),
            Some("wdl") => return Some("wdl"),
            Some("bibtex") => return Some("bibtex"),
            _ => {}
        }
    }
    let language = editor_language(language);
    Some(if language == "text" && format != Format::Text {
        "code"
    } else {
        language
    })
}

#[cfg(test)]
mod opening_tests {
    use gpui_kit::{AppContext, Point, TestAppContext, WeakEntity, WindowOptions, px};
    use sandhill_proto::{CellOutputs, FileOutputs};
    use serde_json::json;
    use std::sync::Arc;

    use super::{Workbench, editor_wrap_type};
    use crate::editor::notebook::{CellKind, Format};

    #[test]
    fn wrapping_types_follow_cells_and_exclude_delimited_data() {
        assert_eq!(
            editor_wrap_type(Format::Text, CellKind::Code, Some("text")),
            Some("text")
        );
        assert_eq!(
            editor_wrap_type(Format::Text, CellKind::Code, Some("json")),
            Some("json")
        );
        assert_eq!(
            editor_wrap_type(Format::Ipynb, CellKind::Code, Some("julia")),
            Some("code")
        );
        assert_eq!(
            editor_wrap_type(Format::Quarto, CellKind::Text, Some("r")),
            Some("markdown")
        );
        assert_eq!(
            editor_wrap_type(Format::Quarto, CellKind::Code, Some("python")),
            Some("python")
        );
        assert_eq!(
            editor_wrap_type(Format::Delimited, CellKind::Code, None),
            None
        );
    }

    #[gpui_kit::test]
    fn opening_keeps_complete_sources_and_outputs_with_lazy_views(cx: &mut TestAppContext) {
        let cells: Vec<_> = (0..1500)
            .map(|ix| {
                json!({
                    "cell_type": "code", "id": format!("cell-{ix}"), "metadata": {},
                    "source": format!("value = {ix}\nprint(value)\n"),
                    "execution_count": null, "outputs": []
                })
            })
            .collect();
        let text = json!({
            "nbformat": 4, "nbformat_minor": 5,
            "metadata": {"language_info": {"name": "python"}}, "cells": cells
        })
        .to_string();
        let nb = crate::editor::notebook::parse("/large.ipynb", &text).unwrap();
        let saved = FileOutputs {
            file: nb.path.clone(),
            seq: 1,
            cells: nb
                .cells
                .iter()
                .map(|cell| CellOutputs {
                    cell_id: cell.id.clone(),
                    run: cell.id.clone(),
                    session: "saved".into(),
                    language: "python".into(),
                    code: cell.source.clone(),
                    started_ms: 0,
                    finished_ms: Some(1),
                    status: Some("ok".into()),
                    execution_count: Some(1),
                    outputs: vec![json!({
                        "output_type": "stream", "name": "stdout",
                        "text": "saved output\n".repeat(20)
                    })],
                })
                .collect(),
        };
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            // Never connect to or change the user's daemon or project.
            crate::welcome::resource_manager::block_host(sandhill_link::LOCAL, cx);
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
                this.tabs.push(nb.path.clone());
                this.show_notebook(nb, 0, window, cx);
                this.show_saved_outputs(saved, &[], false, cx);
                this.place_cells(window);
                let near = this.placed.near.iter().filter(|near| **near).count();
                assert!(near > 0 && near < 50, "initial cell views: {near}");
                assert!(!this.placed.shows(1499));
                let open = this.open.as_ref().unwrap();
                assert_eq!(open.editors.len(), 1500);
                assert_eq!(open.cell_runs.len(), 1500);
                assert_eq!(this.runs["cell-1499"].outputs.len(), 1);
                // Saving and every editor-based action still see the last cell.
                let sources = open.sources(cx);
                assert_eq!(sources.len(), 1500);
                assert_eq!(sources[1499], "value = 1499\nprint(value)\n");
                let written = open.notebook.render(&sources).unwrap();
                let reopened = crate::editor::notebook::parse("/large.ipynb", &written).unwrap();
                assert_eq!(reopened.cells.len(), 1500);
                assert_eq!(reopened.cells[1499].source, sources[1499]);
                cx.notify();
            });
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.read(|cx| {
            let count = workbench.read(cx).notebook_scroll.children_count();
            assert!(count < 100, "offscreen layout reservations: {count}");
        });
        // Newly measured output views above the viewport must not move its anchor.
        let anchor = cx
            .update_window(handle, |_, _, cx| {
                workbench.update(cx, |this, cx| {
                    let scroll = &this.notebook_scroll;
                    scroll.set_offset(Point {
                        x: px(0.),
                        y: -scroll.max_offset().y / 2.,
                    });
                    let anchor = this.placed.logical_scroll_top(scroll).unwrap();
                    cx.notify();
                    anchor
                })
            })
            .unwrap();
        for _ in 0..3 {
            cx.update_window(handle, |_, window, cx| {
                workbench.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        cx.read(|cx| {
            let this = workbench.read(cx);
            let after = this
                .placed
                .logical_scroll_top(&this.notebook_scroll)
                .unwrap();
            assert_eq!(after.0, anchor.0);
            assert!((after.1 - anchor.1).abs() <= px(0.5));
        });
        // Outline/find navigation must still reach the unrendered final cell.
        cx.update_window(handle, |_, window, cx| {
            workbench.update(cx, |this, cx| {
                this.jump_to_source(1499, 0, None, window, cx);
                cx.notify();
            });
        })
        .unwrap();
        for _ in 0..2 {
            cx.update_window(handle, |_, window, cx| {
                workbench.update(cx, |_, cx| cx.notify());
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        cx.read(|cx| assert!(workbench.read(cx).placed.shows(1499)));
    }
}
