//! Workbench side panels: variables, viewer pages, data explorer, terminal and plots.
use super::*;

impl Workbench {
    /// Lists the active kernel's variables again, and the parts open.
    pub(crate) fn refresh_variables(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.variables_session().map(str::to_string))
        else {
            return;
        };
        self.vars.session = Some(session.clone());
        self.vars.loading = true;
        let mut paths = vec![Vec::new()];
        paths.extend(self.vars.expanded.iter().cloned());
        for path in paths {
            let listed = daemon.call::<Variables>(Call::ListVariables {
                session: session.clone(),
                path: path.clone(),
            });
            let session = session.clone();
            cx.spawn(async move |this, cx| {
                let listed = listed.await;
                this.update(cx, |this, cx| {
                    if this.vars.session.as_deref() != Some(&session) {
                        return;
                    }
                    match listed {
                        Ok(listed) if path.is_empty() => {
                            this.vars.list = listed.variables;
                            this.vars.loading = false;
                            this.vars.error = None;
                        }
                        Ok(listed) => {
                            this.vars.children.insert(path, listed.variables);
                        }
                        // Busy with other code: the list updates when it ends.
                        Err(err) if path.is_empty() => {
                            this.vars.loading = false;
                            if !format!("{err:#}").contains("busy") {
                                this.vars.error = Some(format!("{err:#}"));
                            }
                        }
                        Err(_) => {}
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    pub(crate) fn toggle_variable(&mut self, path: Vec<String>, cx: &mut Context<Self>) {
        if self.vars.expanded.remove(&path) {
            cx.notify();
            return;
        }
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            return;
        };
        self.vars.expanded.insert(path.clone());
        let listed = daemon.call::<Variables>(Call::ListVariables {
            session,
            path: path.clone(),
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                match listed {
                    Ok(listed) => {
                        this.vars.children.insert(path, listed.variables);
                    }
                    Err(err) => {
                        this.vars.expanded.remove(&path);
                        this.problem = Some(format!("Listing its parts failed: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn show_tab(
        &mut self,
        tab: ConsoleTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_middle();
        self.console_tab = tab;
        self.show_console = true;
        if self.panel_collapsed(crate::shell::pane_sizes::Panel::Console) {
            self.middle_vertical = crate::shell::pane_sizes::Vertical::Split;
        }
        if tab == ConsoleTab::Terminal {
            self.ensure_terminal(cx);
            if let Some(terminal) = &self.terminal {
                let focus = terminal.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
        }
        cx.notify();
    }

    /// The pages the Console's runs (and scripts') showed, oldest first,
    /// with their titles.
    pub(crate) fn run_pages(&self) -> Vec<crate::panes::viewer::ViewerItem> {
        self.console
            .iter()
            .filter_map(|id| self.runs.get(id))
            .filter(|run| run.cell.as_ref().is_none_or(|(file, _)| is_script(file)))
            .flat_map(|run| run.outputs.iter())
            .filter_map(|output| match show(output) {
                Shown::Page(page) => Some(crate::panes::viewer::ViewerItem {
                    page: Some(page),
                    title: page_title(output),
                    note: None,
                    extra: None,
                    stoppable: false,
                }),
                _ => None,
            })
            .collect()
    }

    /// Keeps a web view for each page about to be drawn, and drops the rest:
    /// a web view is a native view over the window, so one left over would
    /// stay on screen.
    pub(crate) fn sync_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_media(cx);
        let mut wanted: Vec<Page> = Vec::new();
        if self.panel_visible(crate::shell::pane_sizes::Panel::Editor)
            && !self.panel_collapsed(crate::shell::pane_sizes::Panel::Editor)
            && !self.objects.is_shown()
            && self.explorer.is_none()
            && let Some(open) = &self.open
        {
            for (ix, cell) in open.notebook.cells.iter().enumerate() {
                // A page under a cell not drawn would stay where it was.
                if cell.kind != CellKind::Code || !self.placed.shows(ix) {
                    continue;
                }
                let run = open
                    .cell_runs
                    .get(&cell.id)
                    .and_then(|id| self.runs.get(id));
                let outputs = run.map_or(&open.notebook.cells[ix].outputs, |r| &r.outputs);
                wanted.extend(outputs.iter().filter_map(|o| match show(o) {
                    Shown::Page(page) => Some(page),
                    _ => None,
                }));
            }
        }
        let viewer_page = (self.panel_visible(crate::shell::pane_sizes::Panel::LeftBottom)
            && !self.panel_collapsed(crate::shell::pane_sizes::Panel::LeftBottom)
            && self.bottom_tab == BottomTab::Viewer)
            .then(|| self.viewer_page())
            .flatten()
            .and_then(|(item, ..)| item.page);
        let viewer_shown = viewer_page.is_some();
        wanted.extend(viewer_page);
        let keys: HashSet<String> = wanted.iter().map(Page::key).collect();
        // Pages no longer drawn are parked, hidden, and come back when they
        // are drawn again (a cell run again with the same output). Dropping
        // a web view soon after it was drawn can end the app in X11
        // (BadWindow), so only the oldest parked ones go, a while later.
        let gone: Vec<(String, Entity<PageView>)> = self
            .pages
            .extract_if(|key, _| !keys.contains(key))
            .collect();
        for (key, view) in gone {
            view.update(cx, |page, cx| page.hide(cx));
            self.parked.push((key, view));
        }
        for key in &keys {
            if self.pages.contains_key(key) {
                continue;
            }
            if let Some(ix) = self.parked.iter().position(|(k, _)| k == key) {
                let (key, view) = self.parked.remove(ix);
                view.update(cx, |page, cx| page.show(cx));
                self.pages.insert(key, view);
            }
        }
        if self.parked.len() > PARKED_PAGES {
            let old: Vec<_> = self
                .parked
                .drain(..self.parked.len() - PARKED_PAGES)
                .collect();
            cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                drop(old);
            })
            .detach();
        }
        let remote = self.target != Target::Local;
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        let in_notebook = wanted.len() - usize::from(viewer_shown);
        for (ix, page) in wanted.into_iter().enumerate() {
            let key = page.key();
            if self.pages.contains_key(&key) {
                continue;
            }
            let daemon = self.daemon();
            let near = near.clone();
            let wheel = (ix < in_notebook).then(|| self.page_wheel.clone());
            let view = cx.new(|cx| PageView::new(page, daemon, remote, near, wheel, window, cx));
            self.pages.insert(key, view);
        }
    }

    pub(crate) fn show_viewer_page(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.viewer_index = Some(ix);
        cx.notify();
    }

    /// Opens a data frame or matrix from the Variables pane in the Data
    /// Explorer.
    pub(crate) fn open_explorer(
        &mut self,
        path: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_object_explorer();
        let (Some(daemon), Some(session)) = (self.daemon(), self.vars.session.clone()) else {
            return;
        };
        self.reveal_middle();
        let language = self
            .active_kernel()
            .and_then(|k| self.kernels.iter().find(|s| s.name == k))
            .map_or_else(|| "python".to_string(), |s| s.language.to_ascii_lowercase());
        let project = self.project.clone();
        self.explorer =
            Some(cx.new(|cx| Explorer::new(daemon, session, language, path, project, window, cx)));
        if let Some(explorer) = &self.explorer {
            cx.subscribe_in(
                explorer,
                window,
                |this, _, _: &crate::panes::explorer::FocusChanged, window, cx| {
                    this.report_focus(window, cx);
                },
            )
            .detach();
            cx.subscribe(
                explorer,
                |this, explorer, _: &crate::panes::explorer::PopOut, cx| {
                    this.pop_out_explorer(explorer, cx);
                },
            )
            .detach();
        }
        self.explorer_shown = true;
        self.diff_shown = false;
        cx.notify();
    }

    /// Code starts or ends in a session: the Data Explorer waits for it,
    /// then reads its table again.
    pub(super) fn explorer_run(&mut self, session: &str, busy: bool, cx: &mut Context<Self>) {
        self.object_explorers_run(session, busy, cx);
        self.popped_explorers
            .retain(|(_, explorer)| explorer.upgrade().is_some());
        let explorers = self
            .explorer
            .iter()
            .cloned()
            .chain(
                self.popped_explorers
                    .iter()
                    .filter_map(|(_, e)| e.upgrade()),
            )
            .collect::<Vec<_>>();
        for explorer in explorers {
            if explorer.read(cx).session() == session {
                explorer.update(cx, |e, cx| e.set_busy(busy, cx));
            }
        }
    }

    pub(crate) fn close_explorer(&mut self, cx: &mut Context<Self>) {
        self.explorer = None;
        self.explorer_shown = false;
        cx.notify();
    }

    /// Moves the Data Explorer into a window of its own; its tab goes.
    pub(super) fn pop_out_explorer(&mut self, explorer: Entity<Explorer>, cx: &mut Context<Self>) {
        if self.explorer.as_ref() != Some(&explorer) {
            return;
        }
        self.explorer = None;
        self.explorer_shown = false;
        explorer.update(cx, |e, cx| e.set_popped(cx));
        let weak = explorer.downgrade();
        if let Some(handle) = crate::shell::app_window::open_explorer_window(explorer, cx) {
            self.popped_explorers.push((handle, weak));
        }
        cx.notify();
    }

    /// Closes the explorer windows this project opened.
    pub(super) fn close_popped_explorers(&mut self, cx: &mut Context<Self>) {
        for (handle, _) in std::mem::take(&mut self.popped_explorers) {
            cx.update_window(handle, |_, window, _| window.remove_window())
                .ok();
        }
    }

    /// Finds the project's shell terminal, or starts one, and follows it.
    pub(super) fn ensure_terminal(&mut self, cx: &mut Context<Self>) {
        if self.terminal.is_some() || self.terminal_starting || self.restricted() {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.terminal_starting = true;
        self.terminal_problem = None;
        let listed = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
        let generation = self.generation;
        let project_root = project.clone();
        cx.spawn(async move |this, cx| {
            let existing = listed
                .await
                .unwrap_or_default()
                .into_iter()
                .rev()
                .find(|t| {
                    t.kind == TERMINAL_SHELL && t.running && t.project.as_deref() == Some(&project)
                });
            let info = match existing {
                Some(info) => Ok(info),
                None => {
                    daemon
                        .call::<TerminalInfo>(Call::StartTerminal {
                            kind: TERMINAL_SHELL.into(),
                            cwd: Some(project.clone()),
                            project: Some(project),
                            cols: Some(80),
                            rows: Some(24),
                            options: Default::default(),
                            prompt: None,
                        })
                        .await
                }
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.terminal_starting = false;
                match info {
                    Ok(info) => {
                        let id = info.id.clone();
                        let input = daemon.ordered({
                            let id = id.clone();
                            move |data| Call::TerminalInput {
                                terminal: id.clone(),
                                data,
                            }
                        });
                        let resize = {
                            let daemon = daemon.clone();
                            let id = id.clone();
                            Box::new(move |cols, rows| {
                                daemon.fire(Call::ResizeTerminal {
                                    terminal: id.clone(),
                                    cols,
                                    rows,
                                })
                            })
                        };
                        let handle = this.window_handle;
                        let view = cx.new(|cx| {
                            let mut view = TerminalView::new(id, input, resize, handle, cx);
                            view.set_file_links(
                                daemon.clone(),
                                project_root.clone(),
                                info.cwd.clone(),
                                cx,
                            );
                            view
                        });
                        cx.subscribe_in(
                            &view,
                            window,
                            |this, _, link: &gpui_cn::terminal::FileLink, window, cx| {
                                this.open_file_location(
                                    link.path.clone(),
                                    link.line,
                                    link.column,
                                    window,
                                    cx,
                                );
                            },
                        )
                        .detach();
                        if this.console_tab == ConsoleTab::Terminal {
                            let focus = view.read(cx).focus_handle();
                            window.focus(&focus, cx);
                        }
                        this.terminal = Some(view);
                        // Install the view before subscribing: the daemon's
                        // first replay event may follow its response at once.
                        let subscribed =
                            daemon.call::<TerminalSubscribed>(Call::SubscribeTerminal {
                                terminal: info.id,
                                from: None,
                                chat_from: None,
                            });
                        cx.spawn(async move |this, cx| {
                            if let Err(err) = subscribed.await {
                                this.update(cx, |this, cx| {
                                    if this.generation == generation {
                                        this.terminal_problem = Some(format!("{err:#}"));
                                        cx.notify();
                                    }
                                })
                                .ok();
                            }
                        })
                        .detach();
                    }
                    Err(err) => this.terminal_problem = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn on_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        if let Some(quota) = &self.quota {
            quota.update(cx, |quota, cx| quota.terminal_event(&event, cx));
        }
        if let Some(agents) = self.agents.clone()
            && agents.read(cx).owns(&event.terminal)
        {
            agents.update(cx, |agents, cx| agents.on_terminal_event(event, cx));
            // The title bar's dot shows an agent waiting.
            cx.notify();
            return;
        }
        if self.on_job_event(&event, cx) {
            return;
        }
        let Some(view) = self.terminal.clone() else {
            return;
        };
        if view.read(cx).id != event.terminal {
            return;
        }
        view.update(cx, |view, cx| {
            if let Some(data) = &event.data {
                view.feed(data, event.replay, cx);
            }
            if let Some(exit) = &event.exit {
                view.exited(format!("ended: {exit:?}"), cx);
            }
        });
    }

    /// Offscreen cells keep their output model, but image requests only start
    /// when their views are needed. Opening a notebook must not queue all plots.
    pub(crate) fn fetch_notebook_plots(&mut self, cx: &mut Context<Self>) {
        let mut outputs = Vec::new();
        if let Some(open) = &self.open {
            for (ix, cell) in open.notebook.cells.iter().enumerate() {
                if !self.placed.shows(ix) {
                    continue;
                }
                let run = open
                    .cell_runs
                    .get(&cell.id)
                    .and_then(|id| self.runs.get(id));
                outputs.extend(
                    run.map_or(&cell.outputs, |run| &run.outputs)
                        .iter()
                        .cloned(),
                );
            }
        }
        self.fetch_plots(&outputs, cx);
    }

    /// Starts loading the plots in `outputs` that aren't loaded yet.
    pub(super) fn fetch_plots(&mut self, outputs: &[Value], cx: &mut Context<Self>) {
        for output in outputs {
            if let Shown::Plot { mime, value } = show(output) {
                self.fetch_image(&mime, &value, cx);
            }
        }
    }

    /// Reads an image (inline, or a blob from the daemon) into the plot
    /// cache, under [`plot_key`], unless it is there.
    pub(crate) fn fetch_image(&mut self, mime: &str, value: &Value, cx: &mut Context<Self>) {
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        let key = plot_key(value);
        if self.plots.contains_key(&key) {
            return;
        }
        let format = match mime {
            "image/png" => ImageFormat::Png,
            "image/jpeg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            _ => ImageFormat::Svg,
        };
        let text = match (value, self.daemon()) {
            (Value::String(text), _) => {
                let text = text.clone();
                Box::pin(async move { anyhow::Ok(text) })
                    as std::pin::Pin<Box<dyn Future<Output = anyhow::Result<String>>>>
            }
            (blob, Some(daemon)) if blob[BLOB_KEY].is_string() => {
                let fetch = daemon.call::<Blob>(Call::GetBlob {
                    hash: blob[BLOB_KEY].as_str().unwrap_or_default().to_string(),
                    near,
                });
                Box::pin(async move { Ok(fetch.await?.data) })
            }
            _ => return,
        };
        self.plots.insert(key.clone(), Plot::Loading);
        cx.spawn(async move |this, cx| {
            let plot = match text.await.and_then(|text| decode(format, text)) {
                Ok(bytes) => Plot::Ready(Arc::new(Image::from_bytes(format, bytes))),
                Err(err) => Plot::Failed(format!("{err:#}")),
            };
            this.update(cx, |this, cx| {
                this.plots.insert(key, plot);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The plot's bytes: base64 for bitmaps, the text itself for SVG.
fn decode(format: ImageFormat, text: String) -> anyhow::Result<Vec<u8>> {
    if format == ImageFormat::Svg {
        return Ok(text.into_bytes());
    }
    // Ark sends PNG data without base64 padding; use the shared lenient
    // decoder the daemon uses for kernel output.
    let clean: String = text.split_whitespace().collect();
    Ok(sandhill_proto::decode_base64(&clean)?)
}
