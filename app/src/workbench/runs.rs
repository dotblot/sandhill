//! Workbench kernel sessions and runs: choosing kernels, running code, kernel events.
use super::*;

mod view;

impl Workbench {
    /// The project's running kernels, as the daemon last listed them.
    pub(crate) fn project_kernels(&self) -> Vec<String> {
        let Some(project) = self.project.as_deref() else {
            return Vec::new();
        };
        self.machine
            .sessions
            .iter()
            .filter(|s| {
                s.status != sandhill_proto::SessionStatus::Ended
                    && s.project.as_deref() == Some(project)
            })
            .map(|s| s.id.clone())
            .collect()
    }

    /// Stops every kernel of the project; documents using them see it stop.
    pub(crate) fn shut_down_project_kernels(&mut self, cx: &mut Context<Self>) {
        let ids = self.project_kernels();
        self.shut_down(ids, cx);
    }

    /// The shown document's kernel, else the Console's own: what Restart,
    /// Interrupt and the kernel picker act on.
    pub(crate) fn active_session(&self) -> Option<&str> {
        match &self.open {
            Some(open) => open.session.id(),
            None => self.loose.id(),
        }
    }

    /// Visible runs drive the Explorer's busy/idle notifications.
    pub(crate) fn session_is_busy(&self, session: &str) -> bool {
        self.runs
            .values()
            .any(|run| run.session == session && !run.finished && !run.quiet)
    }

    /// The kernel Variables, Help and Packages show: the Console's.
    /// Activating or running a document moves the Console to its kernel; a
    /// document without one (or opening another file) leaves it where it is.
    pub(crate) fn variables_session(&self) -> Option<&str> {
        self.console_target()
    }

    pub(crate) fn active_kernel(&self) -> Option<&str> {
        match &self.open {
            Some(open) => open.kernel.as_deref(),
            None => self.loose_kernel.as_deref(),
        }
    }

    pub(crate) fn active_state(&self) -> &Session {
        match &self.open {
            Some(open) => &open.session,
            None => &self.loose,
        }
    }

    /// Prefer the running session's own language over the selected kernelspec.
    pub(crate) fn session_language(&self, session: &str) -> Option<&str> {
        self.console_sessions
            .live
            .iter()
            .chain(&self.machine.sessions)
            .find(|info| info.id == session)
            .map(|info| info.language.as_str())
            .or_else(|| {
                let kernel = self
                    .open
                    .iter()
                    .chain(&self.others)
                    .find(|open| open.session.id() == Some(session))
                    .and_then(|open| open.kernel.as_deref())
                    .or_else(|| {
                        self.console_sessions
                            .states
                            .iter()
                            .find(|(_, state)| state.id() == Some(session))
                            .map(|(kernel, _)| kernel.as_str())
                    })
                    .or_else(|| {
                        (self.loose.id() == Some(session))
                            .then_some(self.loose_kernel.as_deref())
                            .flatten()
                    })?;
                self.kernels
                    .iter()
                    .find(|spec| spec.name == kernel)
                    .map(|spec| spec.language.as_str())
            })
    }

    pub(crate) fn console_language(&self) -> &'static str {
        let language = self
            .console_target()
            .and_then(|session| self.session_language(session))
            .or_else(|| {
                self.loose_kernel
                    .as_deref()
                    .and_then(|kernel| self.kernels.iter().find(|spec| spec.name == kernel))
                    .map(|spec| spec.language.as_str())
            })
            .or_else(|| {
                self.open
                    .as_ref()
                    .and_then(|open| open.notebook.language.as_deref())
            });
        editor_language(language)
    }

    pub(crate) fn run_language(&self, run: &Run) -> &'static str {
        run.language
            .unwrap_or_else(|| editor_language(self.session_language(&run.session)))
    }

    /// Follows a change of the active kernel: the Console shows its runs
    /// and Variables its variables.
    pub(crate) fn session_changed(&mut self, cx: &mut Context<Self>) {
        let document = self.open.as_ref().and_then(|open| {
            open.session
                .id()
                .map(|id| (open.notebook.path.clone(), id.to_string()))
        });
        if document != self.console_sessions.followed_document {
            self.console_sessions.followed_document = document.clone();
            if let Some((_, id)) = document {
                let kernel = self.open.as_ref().and_then(|open| open.kernel.clone());
                if let Some(previous) = &self.loose_kernel {
                    self.console_sessions
                        .states
                        .insert(previous.clone(), self.loose.clone());
                }
                self.loose = Session::Ready(id.clone());
                self.loose_kernel = kernel;
                if let Some(kernel) = &self.loose_kernel {
                    self.console_sessions
                        .states
                        .insert(kernel.clone(), self.loose.clone());
                }
                self.console_sessions.retained.insert(id);
                self.save_console_choice();
            }
        }
        let active = self.console_target().map(str::to_string);
        let language = self.console_language();
        if self.console_input.read(cx).language_name() != language {
            self.console_input
                .update(cx, |input, cx| input.set_highlighter(language, cx));
        }
        if let Some(path) = self.open.as_ref().map(|open| open.notebook.path.clone())
            && self.language_status(&path).is_some()
        {
            self.language_event(&path, &InputEvent::Focus, cx);
        }
        let variables = self.variables_session().map(str::to_string);
        let variables_changed = variables != self.console_sessions.variables_session;
        self.console_sessions.variables_session = variables.clone();
        if active != self.console_session {
            self.console_session = active.clone();
            self.console_sessions.history_open = false;
            self.switch_console_draft(cx);
            self.console.clear();
            self.viewer_index = None;
            if let Some(session) = active {
                self.restore_console_runs(&session);
                self.follow_console_session(session, cx);
            }
        }
        if variables_changed {
            self.vars = Vars::default();
            if self
                .explorer
                .as_ref()
                .is_some_and(|e| Some(e.read(cx).session()) != variables.as_deref())
            {
                self.explorer = None;
                self.explorer_shown = false;
            }
            self.refresh_variables(cx);
        }
        cx.notify();
    }

    /// A remembered missing kernel remains a visible choice, never a fallback.
    pub(crate) fn kernel_unavailable(&self, name: &str) -> Option<String> {
        if self.open.is_none()
            && self
                .console_sessions
                .live
                .iter()
                .any(|session| session.kernel == name && session.status == SessionStatus::Running)
        {
            return None;
        }
        let Some(spec) = self.kernels.iter().find(|spec| spec.name == name) else {
            return Some(format!(
                "Kernel “{name}” is unavailable on this machine. Select an installed kernel to run this document; its remembered choice has not been changed."
            ));
        };
        if let Some(open) = self.open.as_ref().filter(|o| o.is_script())
            && open
                .notebook
                .language
                .as_deref()
                .is_some_and(|language| !spec.language.eq_ignore_ascii_case(language))
        {
            return Some(format!(
                "Kernel “{name}” cannot run this script's language. Select a matching kernel."
            ));
        }
        None
    }

    /// The kernel this document used last time, from the project's state.
    pub(crate) fn remembered_kernel(&self, path: &str) -> Option<&str> {
        self.project_state["documents"][path]["kernel"].as_str()
    }

    pub(super) fn remember_document_kernel(&mut self, file: &str, kernel: &str) {
        if self.project_state["documents"][file]["kernel"].as_str() == Some(kernel) {
            return;
        }
        if !self.project_state.is_object() {
            self.project_state = serde_json::json!({});
        }
        if !self.project_state["documents"].is_object() {
            self.project_state["documents"] = serde_json::json!({});
        }
        if !self.project_state["documents"][file].is_object() {
            self.project_state["documents"][file] = serde_json::json!({});
        }
        self.project_state["documents"][file]["kernel"] = Value::String(kernel.to_string());
        if let (Some(daemon), Some(path)) = (self.daemon(), self.project.clone()) {
            daemon.fire(Call::SaveProjectState {
                path,
                state: self.project_state.clone(),
            });
        }
    }

    pub(crate) fn choose_kernel(&mut self, name: String, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| !o.notebook.can_run()) {
            return;
        }
        if self.open.is_none() {
            let has_live = self
                .console_sessions
                .live
                .iter()
                .any(|s| s.kernel == name && s.status == SessionStatus::Running);
            if !has_live && let Some(reason) = self.kernel_unavailable(&name) {
                self.problem = Some(reason);
                cx.notify();
                return;
            }
            self.choose_console_kernel(name, cx);
            return;
        }
        if let Some(reason) = self.kernel_unavailable(&name) {
            self.problem = Some(reason);
            cx.notify();
            return;
        }
        if let Some(path) = self.open.as_ref().map(|o| o.notebook.path.clone()) {
            self.remember_document_kernel(&path, &name);
        }
        let daemon = self.daemon();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let (kernel, session) = (&mut open.kernel, &mut open.session);
        if kernel.as_deref() == Some(&name) {
            return;
        }
        if let (Some(daemon), Some(id)) = (daemon, session.id()) {
            daemon.fire(Call::Shutdown {
                session: id.to_string(),
            });
        }
        *session = Session::None;
        *kernel = Some(name);
        if let Some(open) = self.open.as_mut() {
            open.waiting.clear();
            open.sending.clear();
            open.queued.clear();
        }
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn run_cell(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.objects.is_shown() {
            return;
        }
        self.reveal_middle();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.notebook.can_run() {
            return;
        }
        match open.notebook.cells.get(index).map(|c| c.kind) {
            Some(CellKind::Code) => {}
            // Running a Markdown cell shows it formatted, as in Jupyter.
            Some(CellKind::Text) => {
                let id = open.notebook.cells[index].id.clone();
                if open.editing.remove(&id) {
                    cx.notify();
                }
                return;
            }
            None => return,
        }
        if open.is_script() {
            let code = open.editors[index].read(cx).value().to_string();
            self.run_script(code, cx);
            return;
        }
        match &open.session {
            Session::Ready(_) => {
                let path = open.notebook.path.clone();
                self.execute(&path, index, cx)
            }
            Session::Starting => open.waiting.push(open.notebook.cells[index].id.clone()),
            Session::None | Session::Failed(_) => {
                open.waiting.push(open.notebook.cells[index].id.clone());
                self.start_session(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn run_all(&mut self, cx: &mut Context<Self>) {
        let count = self.open.as_ref().map_or(0, |o| o.notebook.cells.len());
        for index in 0..count {
            self.run_cell(index, cx);
        }
    }

    /// Runs the cell whose editor has focus, then moves to the next code
    /// cell, as Shift+Enter does in Jupyter.
    pub(crate) fn run_and_advance(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_cell(index, cx);
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let next = (index + 1..open.notebook.cells.len())
            .find(|&i| open.notebook.cells[i].kind == CellKind::Code);
        if let Some(next) = next {
            open.editors[next].update(cx, |editor, cx| editor.focus(window, cx));
        }
    }

    /// In a script, runs the selection, or the statement at the cursor and
    /// moves the cursor to the next statement, as RStudio does.
    pub(crate) fn run_statement(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.objects.is_shown() {
            return;
        }
        let Some(open) = self.open.as_ref().filter(|o| o.is_script()) else {
            return;
        };
        let language = open.notebook.language.clone().unwrap_or_default();
        let editor = open.editors[0].clone();
        let (text, selected, line) = {
            let editor = editor.read(cx);
            (
                editor.value().to_string(),
                editor.selected_value().to_string(),
                editor.cursor_position().line as usize,
            )
        };
        if !selected.is_empty() {
            if !selected.trim().is_empty() {
                self.run_script(selected, cx);
            }
            return;
        }
        let all = statements::statements(&text, &language);
        let here = all.iter().find(|s| s.from <= line && line <= s.to);
        if let Some(here) = here {
            let code: Vec<&str> = text
                .split('\n')
                .skip(here.from)
                .take(here.to - here.from + 1)
                .collect();
            self.run_script(code.join("\n"), cx);
        }
        let after = here.map_or(line, |s| s.to);
        let lines = text.split('\n').count();
        let target = all
            .iter()
            .find(|s| s.from > after)
            .map_or((after + 1).min(lines.saturating_sub(1)), |s| s.from);
        editor.update(cx, |editor, cx| {
            editor.set_cursor_position(Position::new(target as u32, 0), window, cx)
        });
    }

    /// Runs code from the shown script in its kernel, starting it if needed.
    /// The runs show in the Console.
    pub(crate) fn run_script(&mut self, code: String, cx: &mut Context<Self>) {
        self.reveal_middle();
        let collapsed = self.panel_collapsed(crate::shell::pane_sizes::Panel::Console);
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.is_script() {
            return;
        }
        self.console_tab = ConsoleTab::Console;
        self.show_console = true;
        if collapsed {
            self.middle_vertical = crate::shell::pane_sizes::Vertical::Split;
        }
        match &open.session {
            Session::Ready(_) => {
                let path = open.notebook.path.clone();
                self.send_script_run(&path, code, cx);
            }
            Session::Starting => open.queued.push(code),
            Session::None | Session::Failed(_) => {
                open.queued.push(code);
                self.start_session(cx);
            }
        }
        cx.notify();
    }

    /// Sends a script's code to its kernel, journaled under the script and
    /// its next slot, so reopening the script finds the kernel again.
    pub(super) fn send_script_run(&mut self, path: &str, code: String, cx: &mut Context<Self>) {
        self.bind_console_document(path, cx);
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(open) = self.open_at(path) else {
            return;
        };
        let Some(session) = open.session.id().map(str::to_string) else {
            return;
        };
        let slot = format!("console-{}", open.next_slot);
        open.next_slot = (open.next_slot + 1) % SCRIPT_SLOTS;
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session,
            code,
            cell: Some(CellRef {
                file: path.to_string(),
                cell_id: slot,
            }),
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = started.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Running the script failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Starts the active kernel: the notebook's (in its folder) or the
    /// Console's (in the project folder).
    pub(super) fn start_session(&mut self, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| !o.notebook.can_run()) {
            return;
        }
        if self.held_back("run code", cx) {
            if let Some(open) = self.open.as_mut() {
                open.waiting.clear();
                open.queued.clear();
            }
            return;
        }
        if self.open.is_none() {
            self.start_console_session(cx);
            return;
        }
        if let Some(reason) = self
            .active_kernel()
            .and_then(|name| self.kernel_unavailable(name))
        {
            match self.open.as_mut() {
                Some(open) => {
                    open.session = Session::Failed(reason);
                    open.waiting.clear();
                    open.queued.clear();
                }
                None => self.loose = Session::Failed(reason),
            }
            cx.notify();
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let notebook_path = self.open.as_ref().map(|o| o.notebook.path.clone());
        let missing = match self.open.as_ref().filter(|o| o.is_script()) {
            Some(open) if open.notebook.language.as_deref() == Some("python") => {
                "No Python kernel is installed"
            }
            Some(_) => "No R kernel is installed",
            None => "No kernel is installed",
        };
        let documents_in_project = self.documents_in_project();
        let (kernel, session, cwd) = match self.open.as_mut() {
            // Scripts run in the project's folder, as in RStudio.
            Some(open) if open.is_script() => {
                (open.kernel.clone(), &mut open.session, project.clone())
            }
            Some(open) if documents_in_project => {
                (open.kernel.clone(), &mut open.session, project.clone())
            }
            Some(open) => (
                open.kernel.clone(),
                &mut open.session,
                open.notebook.folder().to_string(),
            ),
            None => (self.loose_kernel.clone(), &mut self.loose, project.clone()),
        };
        let Some(kernel) = kernel else {
            *session = Session::Failed(missing.into());
            if let Some(open) = self.open.as_mut() {
                open.waiting.clear();
                open.queued.clear();
            }
            return;
        };
        *session = Session::Starting;
        let kernel_name = kernel.clone();
        let generation = self.generation;
        let started = daemon.call::<SessionInfo>(Call::StartSession {
            kernel,
            cwd: Some(cwd),
            project: Some(project),
        });
        cx.spawn(async move |this, cx| {
            let session = match started.await {
                Ok(info) => {
                    // Subscribe before running anything, so no output is missed.
                    let subscribed = daemon.call::<Subscribed>(Call::Subscribe {
                        session: info.id.clone(),
                        since_seq: None,
                    });
                    subscribed.await.map(|_| info.id)
                }
                Err(err) => Err(err),
            };
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                // The notebook may be in another tab by now.
                let slot = match &notebook_path {
                    Some(path) => match this.open_at(path) {
                        Some(open)
                            if open.kernel.as_deref() != Some(&kernel_name)
                                || !matches!(open.session, Session::Starting) =>
                        {
                            return;
                        }
                        Some(open) => &mut open.session,
                        None => return,
                    },
                    None => &mut this.loose,
                };
                match session {
                    Ok(id) => {
                        *slot = Session::Ready(id);
                        if let Some(path) = &notebook_path {
                            if let Some(kernel) = this.open_at(path).and_then(|o| o.kernel.clone())
                            {
                                this.remember_document_kernel(path, &kernel);
                            }
                            let waiting = this
                                .open_at(path)
                                .map(|o| std::mem::take(&mut o.waiting))
                                .unwrap_or_default();
                            for id in waiting {
                                let index = this.open_at(path).and_then(|o| {
                                    o.notebook
                                        .source_notebook()
                                        .unwrap_or(&o.notebook)
                                        .cells
                                        .iter()
                                        .position(|c| c.id == id)
                                });
                                if let Some(index) = index {
                                    this.execute(path, index, cx);
                                }
                            }
                            let queued = this
                                .open_at(path)
                                .map(|o| std::mem::take(&mut o.queued))
                                .unwrap_or_default();
                            for code in queued {
                                this.send_script_run(path, code, cx);
                            }
                        }
                        this.session_changed(cx);
                    }
                    Err(err) => {
                        *slot = Session::Failed(format!("{err:#}"));
                        if let Some(open) = notebook_path.as_deref().and_then(|p| this.open_at(p)) {
                            open.waiting.clear();
                            open.queued.clear();
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a cell of the notebook with this path, shown or not.
    pub(super) fn execute(&mut self, path: &str, index: usize, cx: &mut Context<Self>) {
        self.bind_console_document(path, cx);
        let Some(daemon) = self.daemon() else {
            return;
        };
        let by_user = !self.agent_running;
        let Some(open) = self.open_at(path) else {
            return;
        };
        let (Some(session), Some(cell), Some(editor)) = (
            open.session.id(),
            open.notebook
                .source_notebook()
                .unwrap_or(&open.notebook)
                .cells
                .get(index),
            open.quarto_view
                .as_ref()
                .map_or(&open.editors, |view| &view.editors)
                .get(index),
        ) else {
            return;
        };
        let cell_id = cell.id.clone();
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session: session.to_string(),
            code: editor.read(cx).value().to_string(),
            cell: Some(CellRef {
                file: open.notebook.path.clone(),
                cell_id: cell_id.clone(),
            }),
            quiet: false,
            by: None,
        });
        open.sending.insert(cell_id.clone());
        // A new run takes the place of the last suggestion.
        open.ghost = None;
        open.ghost_after = by_user.then(|| cell_id.clone());
        let path = open.notebook.path.clone();
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                let Some(open) = this.open_at(&path) else {
                    return;
                };
                open.sending.remove(&cell_id);
                match started {
                    Ok(run) => {
                        open.cell_runs.insert(cell_id, run.run.clone());
                        this.runs.entry(run.run).or_default();
                    }
                    Err(err) => this.problem = Some(format!("Running the cell failed: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs what is typed in the Console in the active kernel, or answers
    /// the question its running code asked.
    pub(crate) fn run_console(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.answer_console_input(window, cx) {
            return;
        }
        let code = self.console_input.read(cx).value().to_string();
        if code.trim().is_empty() {
            return;
        }
        // `?topic` and `help(x)` open the Help pane, as in the Tauri app.
        if let Some(asked) = crate::panes::help::console_help(&code, self.console_language()) {
            self.console_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.ask_help(asked, window, cx);
            return;
        }
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.console_target().map(str::to_string))
        else {
            self.problem =
                Some("Choose a kernel and wait for it to be ready before running code.".into());
            cx.notify();
            return;
        };
        self.console_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session,
            code,
            cell: None,
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = started.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Running the code failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        self.problem = None;
    }

    /// Stops the shown document's kernel (or the Console's) for good; the
    /// next run starts a new one.
    pub(crate) fn shut_down_session(&mut self, cx: &mut Context<Self>) {
        if self.open.is_none() {
            self.stop_console_session(cx);
            return;
        }
        let daemon = self.daemon();
        let slot = match self.open.as_mut() {
            Some(open) => &mut open.session,
            None => &mut self.loose,
        };
        if let (Some(daemon), Some(id)) = (daemon, slot.id()) {
            let id = id.to_string();
            self.machine.sessions.retain(|s| s.id != id);
            daemon.fire(Call::Shutdown { session: id });
        }
        *slot = Session::None;
        self.session_changed(cx);
        cx.notify();
    }

    /// Restarts the shown document's kernel: a new one of the same kind,
    /// with nothing defined.
    pub(crate) fn restart_session(&mut self, cx: &mut Context<Self>) {
        if self.active_session().is_none() {
            return;
        }
        self.shut_down_session(cx);
        self.start_session(cx);
        cx.notify();
    }

    pub(crate) fn interrupt(&mut self) {
        if let (Some(daemon), Some(session)) = (self.daemon(), self.active_session()) {
            daemon.fire(Call::Interrupt {
                session: session.to_string(),
            });
        }
    }

    pub(super) fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        // Any open notebook's kernel, shown or in another tab, or the
        // Console's own.
        let ours = self
            .open
            .iter()
            .chain(&self.others)
            .any(|o| o.session.id() == Some(event.session.as_str()))
            || self.loose.id() == Some(event.session.as_str())
            || self.console_sessions.retained.contains(&event.session);
        if !ours {
            return;
        }
        if !self.accepts_console_replay(&event) {
            return;
        }
        if event.msg_type == sandhill_proto::EVENT_VIEW {
            self.view_from_code(&event, cx);
            return;
        }
        let active = self.console_target() == Some(event.session.as_str());
        if event.msg_type == EVENT_SESSION_ENDED {
            for run in self
                .runs
                .values_mut()
                .filter(|r| !r.finished && r.session == event.session)
            {
                run.finished = true;
                run.status = Some("the kernel stopped".into());
            }
            let ended = || Session::Failed("The kernel stopped".into());
            for open in self.open.iter_mut().chain(&mut self.others) {
                if open.session.id() == Some(event.session.as_str()) {
                    open.session = ended();
                }
            }
            if self.loose.id() == Some(event.session.as_str()) {
                self.loose = ended();
            }
            for state in self.console_sessions.states.values_mut() {
                if state.id() == Some(event.session.as_str()) {
                    *state = ended();
                }
            }
            if self.console_sessions.retained.remove(&event.session) {
                self.save_console_choice();
            }
            self.console_sessions.live.retain(|s| s.id != event.session);
            cx.notify();
            return;
        }
        // Events can arrive before the run id does, so keep them by run.
        let Some(run_id) = event.run else {
            return;
        };
        let queue_moved = matches!(
            event.msg_type.as_str(),
            EVENT_RUN_STARTED | EVENT_RUN_FINISHED
        );
        let (session, ts_ms) = (event.session.clone(), event.ts_ms);
        if self
            .runs
            .get(&run_id)
            .is_some_and(|run| event.seq <= run.last_seq)
        {
            return;
        }
        if active && !event.msg_type.starts_with("status") {
            self.console_scroll.scroll_to_bottom();
        }
        match event.msg_type.as_str() {
            EVENT_RUN_STARTED if !event.content["quiet"].as_bool().unwrap_or(false) => {
                self.explorer_run(&event.session, true, cx)
            }
            EVENT_RUN_FINISHED if !self.runs.get(&run_id).is_some_and(|r| r.quiet) => {
                self.explorer_run(&event.session, false, cx)
            }
            _ => {}
        }
        let language = self
            .session_language(&event.session)
            .map(|language| editor_language(Some(language)));
        let run = self.runs.entry(run_id.clone()).or_default();
        run.last_seq = event.seq;
        if language.is_some() {
            run.language = language;
        }
        match event.msg_type.as_str() {
            EVENT_RUN_STARTED => {
                run.session = event.session.clone();
                run.queued_ms = Some(event.ts_ms);
                let content = &event.content;
                run.code = content["code"].as_str().unwrap_or_default().to_string();
                run.quiet = content["quiet"].as_bool().unwrap_or(false);
                run.by = content["by"].as_str().map(str::to_string);
                run.cell = match (
                    content["cell"]["file"].as_str(),
                    content["cell"]["cell_id"].as_str(),
                ) {
                    (Some(file), Some(id)) => Some((file.to_string(), id.to_string())),
                    _ => None,
                };
                if !run.quiet && active && !self.console.contains(&run_id) {
                    self.console.push(run_id.clone());
                    if self.console.len() > CONSOLE_RUNS {
                        self.console.remove(0);
                    }
                }
                if !run.quiet && event.seq > run.snapshot_seq {
                    let code = run.code.clone();
                    self.record_console_command(&session, run_id.clone(), code, event.seq);
                }
            }
            EVENT_RUN_FINISHED => {
                self.plots_run_finished(&event.session, &run_id, cx);
                let run = self.runs.entry(run_id.clone()).or_default();
                run.finished = true;
                run.asking = None;
                run.status = event.content["status"].as_str().map(str::to_string);
                run.execution_count = event.content["execution_count"].as_u64();
                run.finished_ms = Some(event.ts_ms);
                if !run.quiet && self.variables_session() == Some(event.session.as_str()) {
                    self.refresh_variables(cx);
                    self.packages_run_finished(cx);
                }
                self.suggest_after_run(&run_id, cx);
                self.explore_when_made(&run_id, cx);
            }
            sandhill_proto::EVENT_INPUT_REQUEST | sandhill_proto::EVENT_INPUT_REPLY => {
                run.append_output(event.seq, event.msg_type, event.content);
                run.question_from_events();
                // The question is answered in the Console.
                if active && run.asking.is_some() {
                    self.console_tab = ConsoleTab::Console;
                }
            }
            _ if run.quiet => {}
            msg_type => {
                let is_plot = matches!(
                    msg_type,
                    "display_data" | "execute_result" | "update_display_data"
                );
                run.append_output(event.seq, event.msg_type, event.content);
                if is_plot {
                    let for_viewer = run.cell.as_ref().is_none_or(|(file, _)| is_script(file));
                    let outputs = run.outputs.clone();
                    let page = outputs.iter().any(|o| matches!(show(o), Shown::Page(_)));
                    if page && for_viewer && active {
                        self.bottom_tab = BottomTab::Viewer;
                        self.viewer_index = None;
                    }
                    self.fetch_plots(&outputs, cx);
                    self.plots_from_run(&event.session, &run_id, cx);
                }
            }
        }
        if queue_moved {
            self.advance_queue(&session, ts_ms, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod view_tests {
    use std::sync::Arc;

    use gpui_kit::{AppContext, TestAppContext, WeakEntity, WindowOptions};
    use sandhill_link::Target;
    use sandhill_proto::Event;

    use super::{Session, Workbench, notebook};

    #[gpui_kit::test]
    fn view_events_from_a_script_session_are_handled(cx: &mut TestAppContext) {
        let rt = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap(),
        );
        let (handle, workbench) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::look::install(cx);
            // Never connect to the user's daemon or project.
            crate::welcome::resource_manager::block_host(sandhill_link::LOCAL, cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    Workbench::new(
                        rt,
                        Target::Local,
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
                let nb = notebook::parse("/wide_table.py", "counts = None\n").unwrap();
                this.tabs.push(nb.path.clone());
                this.show_notebook(nb, 0, window, cx);
                this.open.as_mut().unwrap().session = Session::Ready("file-session".into());
                // The event must use the script's session, not the current
                // Variables or Console session.
                this.loose = Session::Ready("other-session".into());
                for (seq, run) in [(1, Some("view-run".into())), (2, None)] {
                    this.problem = None;
                    this.on_event(
                        Event {
                            seq,
                            session: "file-session".into(),
                            run,
                            msg_type: sandhill_proto::EVENT_VIEW.into(),
                            ts_ms: 0,
                            content: serde_json::json!({"title": "counts"}),
                        },
                        cx,
                    );
                    // With no connection, a handled request reports why it
                    // cannot open; it must not silently become Console output.
                    assert_eq!(
                        this.problem.as_deref(),
                        Some("Could not open counts: not connected to the daemon.")
                    );
                    assert!(!this.runs.contains_key("view-run"));
                }
            });
        })
        .unwrap();
    }
}
