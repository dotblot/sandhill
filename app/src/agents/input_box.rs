//! The message box's workings, as the Tauri app's `AgentComposer.tsx` and
//! `AgentsPane.tsx` have them: drafts kept per conversation, the @ and #
//! menus and their chips, attaching files, and the model, effort and mode
//! choices, for a new conversation or the one shown.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use sandhill_proto::{
    AgentOptions, BLOB_KEY, Blob, Call, FileText, SessionInfo, SessionPlots, SessionStatus,
    TerminalInfo, Variables,
};
use serde_json::{Value, json};

use crate::agents::composer::{
    self, ATTACHMENTS, Mention, Mode, ModelChoice, Models, cell_documents, cell_mentions,
    document_cells, filter_cells, filter_commands, filter_mentions, ref_name, same_ref,
    typed_mention,
};
use crate::agents::{
    Agents, Choice, Menu, START_COLUMNS, START_ROWS, SUBMIT_DELAY, Show, drives_chat, terminal_only,
};

mod model_choices;

/// How long the @ menu waits for a session's variables and plots.
const ASK_LIMIT: Duration = Duration::from_secs(4);
/// Plots the @ menu offers, newest first.
const PLOTS_OFFERED: usize = 12;
/// The image types a plot can be attached as, best first.
const PLOT_MIMES: [&str; 4] = ["image/png", "image/svg+xml", "image/jpeg", "image/gif"];

/// "umap · analysis.qmd" for a cell's plot, "Console plot · 14:31" for one
/// from the Console.
pub(crate) fn plot_name(cell: Option<(&str, &str)>, by: Option<&str>, ts_ms: i64) -> String {
    use chrono::TimeZone;
    if let Some((file, cell)) = cell {
        return format!("{cell} · {file}");
    }
    let time = chrono::Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_default();
    match by {
        Some(by) => format!("{by}'s plot · {time}"),
        None => format!("Console plot · {time}"),
    }
}

/// A file name for a pasted image with none: "Pasted image 2026-11-02 at
/// 14.31.05.png", as the Mac names screenshots.
pub(crate) fn pasted_name(now: chrono::DateTime<chrono::Local>, extension: &str) -> String {
    format!(
        "Pasted image {} at {}.{extension}",
        now.format("%Y-%m-%d"),
        now.format("%H.%M.%S")
    )
}

/// Effort changes wait this long for the next one (the slider can be dragged).
const EFFORT_SETTLE: Duration = Duration::from_millis(600);
/// Switching Claude Code's mode presses ⇧Tab until its screen shows it.
const MODE_TRIES: usize = 6;
const MODE_STEP: Duration = Duration::from_millis(350);

impl Agents {
    /// The draft the box should hold now: the shown conversation's, or the
    /// new one's.
    pub(crate) fn wanted_draft(&self) -> String {
        match (&self.show, &self.selected) {
            (Show::New, _) | (_, None) => "new".into(),
            (_, Some(id)) => id.clone(),
        }
    }

    /// Keeps the box's text and chips under the conversation they were for
    /// and shows the ones of the conversation shown now.
    pub(crate) fn swap_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.wanted_draft();
        if key == self.draft_key {
            return;
        }
        let text = self.composer.read(cx).value().to_string();
        let old = std::mem::replace(&mut self.draft_key, key.clone());
        if text.is_empty() && self.refs.is_empty() {
            self.drafts.remove(&old);
        } else {
            self.drafts
                .insert(old, (text, std::mem::take(&mut self.refs)));
        }
        let (text, refs) = self.drafts.remove(&key).unwrap_or_default();
        self.refs = refs;
        self.menu = None;
        self.composer
            .update(cx, |c, cx| c.set_value(text, window, cx));
    }

    /// The text changed: opens, narrows or closes the @, # and / menus.
    pub(crate) fn composer_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (text, caret) = {
            let c = self.composer.read(cx);
            (c.value().to_string(), c.cursor())
        };
        // Posit Assistant's own screen has its "/" list.
        let typed = typed_mention(&text, caret)
            .filter(|(_, _, sign)| *sign != '/' || !terminal_only(self.box_kind()));
        match typed {
            Some((_, query, sign)) => self.open_mention(sign, query, cx),
            None => {
                if matches!(self.menu, Some(Menu::Mention { .. })) {
                    self.menu = None;
                    cx.notify();
                }
            }
        }
    }

    /// Opens the @ menu (or # or /), reading what it offers the first time.
    fn open_mention(&mut self, sign: char, query: String, cx: &mut Context<Self>) {
        if let Some(Menu::Mention {
            sign: open,
            query: q,
            active,
            ..
        }) = &mut self.menu
            && *open == sign
        {
            if *q != query {
                *q = query;
                *active = 0;
                self.mention_scroll.scroll_to_item(0);
            }
            cx.notify();
            return;
        }
        self.menu = Some(Menu::Mention {
            sign,
            query,
            items: None,
            active: 0,
        });
        self.mention_scroll.scroll_to_item(0);
        cx.notify();
        match sign {
            '#' => self.load_cells(cx),
            '/' => self.load_commands(cx),
            _ => self.load_mentions(cx),
        }
    }

    fn loaded(&mut self, sign: char, list: Vec<Mention>, cx: &mut Context<Self>) {
        if let Some(Menu::Mention {
            sign: open, items, ..
        }) = &mut self.menu
            && *open == sign
        {
            *items = Some(list);
            cx.notify();
        }
    }

    /// The "/" list: Compact while a conversation of an agent that can
    /// compact runs, the commands a running OpenScience or Oh My Pi
    /// conversation reported, then what the daemon finds for the agent (its
    /// commands and skills, and Sandhill's guides).
    fn load_commands(&mut self, cx: &mut Context<Self>) {
        let running = self.box_terminal().filter(|t| t.running);
        let mut list: Vec<Value> = Vec::new();
        if let Some(t) = running {
            if composer::can_compact(&t.kind) {
                list.push(json!({
                    "kind": "compact",
                    "name": "compact",
                    "description": "Replace the conversation with a summary, to make room",
                }));
            }
            list.extend(composer::reported_commands(t));
        }
        let listed = self.daemon.call::<Vec<Value>>(Call::AgentCommands {
            kind: self.box_kind().to_string(),
            cwd: self.project.clone(),
        });
        cx.spawn(async move |this, cx| {
            list.extend(listed.await.unwrap_or_default());
            let list = list
                .into_iter()
                .map(|item| Mention {
                    label: Some(format!("/{}", ref_name(&item))),
                    detail: item["description"]
                        .as_str()
                        .filter(|d| !d.is_empty())
                        .map(str::to_string),
                    item,
                    source: None,
                })
                .collect();
            this.update(cx, |this, cx| this.loaded('/', list, cx)).ok();
        })
        .detach();
    }

    /// Saves `text` as `name` in the attachments folder, then adds it as a
    /// chip (a conversation's summary, for the new one).
    pub(crate) fn attach_text(&mut self, name: String, text: String, cx: &mut Context<Self>) {
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        cx.spawn(async move |this, cx| {
            let saved = save_attachment(&daemon, &folder, &name, text, false).await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(()) => this.add_ref(json!({
                        "kind": "attach",
                        "name": name,
                        "path": format!("{ATTACHMENTS}/{name}"),
                    })),
                    Err(err) => this.problem = Some(format!("Could not save the summary: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The project's files and the variables of its running kernels.
    fn load_mentions(&mut self, cx: &mut Context<Self>) {
        let daemon = self.daemon.clone();
        let project = self.project.clone();
        let files = daemon.call::<Vec<String>>(Call::ListFiles {
            path: project.clone(),
            limit: None,
        });
        let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let mut list: Vec<Mention> = files
                .await
                .unwrap_or_default()
                .iter()
                .map(|f| Mention::file(f))
                .collect();
            let sessions = sessions.await.unwrap_or_default();
            let mut plots: Vec<(i64, Mention)> = Vec::new();
            let root = format!("{}/", project.trim_end_matches('/'));
            // Every session is asked at once; one that doesn't answer (busy,
            // or its kernel gone) is left out rather than holding the menu.
            let asked: Vec<_> = sessions
                .iter()
                .filter(|s| {
                    s.status == SessionStatus::Running && s.project.as_deref() == Some(&project)
                })
                .map(|s| {
                    let plots = daemon.call_within::<SessionPlots>(
                        Call::SessionPlots {
                            session: s.id.clone(),
                            limit: Some(PLOTS_OFFERED as u32),
                        },
                        ASK_LIMIT,
                    );
                    let vars = daemon.call_within::<Variables>(
                        Call::ListVariables {
                            session: s.id.clone(),
                            path: Vec::new(),
                        },
                        ASK_LIMIT,
                    );
                    (s.clone(), plots, vars)
                })
                .collect();
            for (s, read, listed) in asked {
                if let Ok(read) = read.await {
                    for plot in read.plots {
                        let Some(mime) =
                            PLOT_MIMES.into_iter().find(|m| plot.data.get(*m).is_some())
                        else {
                            continue;
                        };
                        let file = plot
                            .cell
                            .as_ref()
                            .map(|c| c.file.strip_prefix(&root).unwrap_or(&c.file).to_string());
                        let name = plot_name(
                            file.as_deref()
                                .zip(plot.cell.as_ref().map(|c| c.cell_id.as_str())),
                            plot.by.as_deref(),
                            plot.ts_ms,
                        );
                        let source =
                            json!({"run": plot.run, "mime": mime, "value": plot.data[mime]});
                        plots.push((
                            plot.ts_ms,
                            Mention {
                                item: json!({"kind": "plot", "name": name}),
                                source: Some(source),
                                label: None,
                                detail: None,
                            },
                        ));
                    }
                }
                if let Ok(vars) = listed.await {
                    list.extend(vars.variables.iter().map(|v| Mention {
                        item: json!({
                            "kind": "variable",
                            "name": v.name,
                            "session": s.id,
                            "language": s.language,
                        }),
                        source: None,
                        label: None,
                        detail: None,
                    }));
                }
            }
            plots.sort_by_key(|(ts, _)| std::cmp::Reverse(*ts));
            list.extend(plots.into_iter().take(PLOTS_OFFERED).map(|(_, m)| m));
            this.update(cx, |this, cx| this.loaded('@', list, cx)).ok();
        })
        .detach();
    }

    /// Saves a plot picked with @ in the attachments folder, where the
    /// agent can look at it, then adds it as a chip.
    fn attach_plot(&mut self, name: String, source: Value, cx: &mut Context<Self>) {
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        cx.spawn(async move |this, cx| {
            let result = async {
                let mime = source["mime"].as_str().unwrap_or("image/png");
                let svg = mime == "image/svg+xml";
                let value = &source["value"];
                let data = match value.as_str() {
                    Some(text) => text.to_string(),
                    None => {
                        let hash = value[BLOB_KEY].as_str().unwrap_or_default().to_string();
                        daemon
                            .call::<Blob>(Call::GetBlob { hash, near: None })
                            .await?
                            .data
                    }
                };
                let run: String = source["run"]
                    .as_str()
                    .unwrap_or("plot")
                    .chars()
                    .take(8)
                    .collect();
                let extension = match mime {
                    "image/svg+xml" => "svg",
                    "image/jpeg" => "jpg",
                    "image/gif" => "gif",
                    _ => "png",
                };
                let file = format!("plot-{run}.{extension}");
                save_attachment(&daemon, &folder, &file, data, !svg).await?;
                anyhow::Ok(
                    json!({"kind": "plot", "name": name, "path": format!("{ATTACHMENTS}/{file}")}),
                )
            }
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(item) => this.add_ref(item),
                    Err(err) => this.problem = Some(format!("Could not add the plot: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Files dropped on the message box, or pasted: saved in the
    /// attachments folder for the agent to read.
    pub(crate) fn attach_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        cx.spawn(async move |this, cx| {
            for path in paths {
                let result = attach_file(&daemon, &folder, &path).await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(item) => this.add_ref(item),
                        Err(err) => {
                            this.problem =
                                Some(format!("Could not attach {}: {err:#}", path.display()))
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Pasting an image (a screenshot) or files copied in Finder attaches
    /// them; says whether the clipboard held any, so text pastes as usual.
    pub(crate) fn paste_attachments(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let mut images = Vec::new();
        let mut paths = Vec::new();
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(image) => images.push(image.clone()),
                ClipboardEntry::ExternalPaths(list) => paths.extend(list.paths().iter().cloned()),
                _ => {}
            }
        }
        if images.is_empty() && paths.is_empty() {
            return false;
        }
        if !paths.is_empty() {
            self.attach_paths(paths, cx);
        }
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        for (i, image) in images.into_iter().enumerate() {
            let extension = match image.format() {
                ImageFormat::Jpeg => "jpg",
                ImageFormat::Gif => "gif",
                ImageFormat::Svg => "svg",
                ImageFormat::Webp => "webp",
                ImageFormat::Tiff => "tiff",
                ImageFormat::Bmp => "bmp",
                _ => "png",
            };
            let mut name = pasted_name(chrono::Local::now(), extension);
            if i > 0 {
                name = name.replace(&format!(".{extension}"), &format!(" {}.{extension}", i + 1));
            }
            let data = base64::engine::general_purpose::STANDARD.encode(image.bytes());
            let daemon = daemon.clone();
            let folder = folder.clone();
            cx.spawn(async move |this, cx| {
                let result = save_attachment(&daemon, &folder, &name, data, true).await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(()) => this.add_ref(json!({
                            "kind": "attach",
                            "name": name,
                            "path": format!("{ATTACHMENTS}/{name}"),
                        })),
                        Err(err) => {
                            this.problem = Some(format!("Could not attach the image: {err:#}"))
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        true
    }

    /// The cells of the documents named with @: as edited in their tab,
    /// else as saved.
    fn load_cells(&mut self, cx: &mut Context<Self>) {
        let mut list = Vec::new();
        let mut unread = Vec::new();
        for path in cell_documents(&self.refs) {
            let full = self.full_path(&path);
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let open = self
                .workbench
                .upgrade()
                .and_then(|w| w.read(cx).open_cells(&full, cx));
            match open {
                Some(cells) => list.extend(cell_mentions(&path, &name, &cells)),
                None => unread.push((path, name, full)),
            }
        }
        if unread.is_empty() {
            return self.loaded('#', list, cx);
        }
        let daemon = self.daemon.clone();
        cx.spawn(async move |this, cx| {
            for (path, name, full) in unread {
                if let Ok(file) = daemon.call::<FileText>(Call::ReadFile { path: full }).await {
                    list.extend(cell_mentions(
                        &path,
                        &name,
                        &document_cells(&path, &file.text),
                    ));
                }
            }
            this.update(cx, |this, cx| this.loaded('#', list, cx)).ok();
        })
        .detach();
    }

    /// A path the agent was given, on the daemon's machine.
    pub(crate) fn full_path(&self, path: &str) -> String {
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{path}", self.project.trim_end_matches('/'))
        }
    }

    /// A file the agent named, relative to the project; `None` when it is
    /// outside it (another folder, the home folder, or up with "..").
    pub(crate) fn project_file(&self, path: &str) -> Option<String> {
        let root = format!("{}/", self.project.trim_end_matches('/'));
        let rel = path.strip_prefix(&root).unwrap_or(path);
        let outside = rel.starts_with(['/', '~']) || rel.split('/').any(|part| part == "..");
        (!outside).then(|| rel.to_string())
    }

    /// Opens a file the agent named in the editor, if it is in the project.
    pub(crate) fn open_path(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(rel) = self.project_file(path) else {
            self.problem = Some(format!("{path} is outside the project"));
            cx.notify();
            return;
        };
        if let Some(w) = self.workbench.upgrade() {
            w.update(cx, |w, cx| w.open_file(rel, cx));
        }
    }

    /// The open @, # or / menu's groups, as shown.
    pub(crate) fn mention_groups(&self) -> Vec<(String, Vec<Mention>)> {
        let Some(Menu::Mention {
            sign,
            query,
            items: Some(items),
            ..
        }) = &self.menu
        else {
            return Vec::new();
        };
        let groups = match sign {
            '#' => filter_cells(items, query),
            '/' => filter_commands(items, query),
            _ => filter_mentions(items, query),
        };
        groups
            .into_iter()
            .map(|(label, list)| (label, list.into_iter().cloned().collect()))
            .collect()
    }

    fn shown_mentions(&self) -> Vec<Mention> {
        self.mention_groups()
            .into_iter()
            .flat_map(|(_, list)| list)
            .collect()
    }

    /// Arrows in either mention menu, including headings in the scroll index.
    pub(crate) fn move_mention(&mut self, down: bool, cx: &mut Context<Self>) {
        let groups = self.mention_groups();
        let n = groups.iter().map(|(_, items)| items.len()).sum::<usize>();
        if let Some(Menu::Mention { active, .. }) = &mut self.menu
            && n > 0
        {
            *active = if down {
                (*active + 1) % n
            } else {
                (*active + n - 1) % n
            };
            let mut remaining = *active;
            let mut row = 0;
            for (_, items) in &groups {
                row += 1; // Group heading.
                if remaining < items.len() {
                    self.mention_scroll.scroll_to_item(row + remaining);
                    break;
                }
                remaining -= items.len();
                row += items.len();
            }
            cx.notify();
        }
    }

    /// Whether the "/" list is open with nothing matching what was typed:
    /// Enter then sends the text as it is.
    pub(crate) fn slash_unmatched(&self) -> bool {
        matches!(
            self.menu,
            Some(Menu::Mention {
                sign: '/',
                items: Some(_),
                ..
            })
        ) && self.mention_groups().is_empty()
    }

    /// Adds the picked item (or the highlighted one) as a chip; the sign
    /// and what was typed after it go. Compact runs at once.
    pub(crate) fn pick_mention(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (active, sign) = match &self.menu {
            Some(Menu::Mention { active, sign, .. }) => (*active, *sign),
            _ => return,
        };
        let picked = self
            .shown_mentions()
            .into_iter()
            .nth(index.unwrap_or(active));
        self.close_menu(window, cx);
        if let Some(m) = picked {
            match m.source {
                Some(source) => {
                    let name = composer::ref_name(&m.item).to_string();
                    self.attach_plot(name, source, cx);
                }
                None if composer::ref_kind(&m.item) == "compact" => self.compact(cx),
                None if sign == '/' => {
                    // One per message, first.
                    self.problem = None;
                    self.refs.retain(|r| !composer::is_slash(r));
                    self.refs.insert(0, m.item);
                }
                None => self.add_ref(m.item),
            }
        }
        cx.notify();
    }

    /// The @ list's "Commands and skills…": the "/" list, typed when the
    /// box is empty (so typing narrows it), else opened beside the text.
    pub(crate) fn open_slash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(window, cx);
        let empty = self.composer.read(cx).value().is_empty();
        if empty {
            self.composer.update(cx, |c, cx| {
                c.insert("/", window, cx);
                c.focus(window, cx);
            });
            self.composer_changed(window, cx);
        } else {
            self.open_mention('/', String::new(), cx);
        }
    }

    pub(crate) fn add_ref(&mut self, item: Value) {
        self.problem = None;
        if !self.refs.iter().any(|r| same_ref(r, &item)) {
            self.refs.push(item);
        }
    }

    /// Closes the open menu; the @, # or / menu takes what was typed for it
    /// along (nothing when it was opened without typing).
    pub(crate) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Menu::Mention { sign, .. }) = self.menu.take() {
            self.composer.update(cx, |c, cx| {
                let caret = c.cursor();
                if let Some((at, _, typed)) = typed_mention(&c.value(), caret)
                    && typed == sign
                {
                    c.set_selected_range(at..caret, cx);
                    c.replace("", window, cx);
                }
                c.focus(window, cx);
            });
        }
        cx.notify();
    }

    /// The + button: types @ at the caret, which opens the @ list as typing
    /// it does; clicked again, it closes the list and takes the @ away.
    pub(crate) fn toggle_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.menu, Some(Menu::Mention { .. })) {
            return self.close_menu(window, cx);
        }
        self.composer.update(cx, |c, cx| {
            let caret = c.cursor();
            // The sign must start the text or follow a space.
            let after_word = c
                .value()
                .get(..caret)
                .and_then(|before| before.chars().next_back())
                .is_some_and(|ch| !ch.is_whitespace());
            c.insert(if after_word { " @" } else { "@" }, window, cx);
            c.focus(window, cx);
        });
        self.composer_changed(window, cx);
    }

    pub(crate) fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        self.gear = false;
        self.confirm_end = false;
        let same = self
            .menu
            .as_ref()
            .is_some_and(|m| std::mem::discriminant(m) == std::mem::discriminant(&menu));
        self.close_menu(window, cx);
        if !same {
            if menu == Menu::Model {
                self.model_tab = None;
            }
            if menu == Menu::Context && self.compact_focus.is_none() {
                self.compact_focus =
                    Some(cx.new(|cx| {
                        InputState::new(window, cx).placeholder("Keep in mind… (optional)")
                    }));
            }
            self.menu = Some(menu);
        }
        cx.notify();
    }

    pub(crate) fn remove_ref(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.refs.len() {
            self.refs.remove(index);
        }
        cx.notify();
    }

    /// Picks files on this computer and saves them in the project's
    /// attachments folder (kept out of git), where the agent can read them.
    pub(crate) fn attach(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        let daemon = self.daemon.clone();
        let folder = self.full_path(ATTACHMENTS);
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            for path in paths {
                let result = attach_file(&daemon, &folder, &path).await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(item) => this.add_ref(item),
                        Err(err) => {
                            this.problem =
                                Some(format!("Could not attach {}: {err:#}", path.display()))
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Types a command into the agent's terminal, then Enter. While the
    /// agent waits for an answer it would answer it, so it waits too.
    pub(crate) fn command(&mut self, t: &TerminalInfo, text: String, cx: &mut Context<Self>) {
        if self.waiting(t) {
            self.held = Some((t.id.clone(), text));
        } else {
            self.type_command(&t.id, text, cx);
        }
    }

    pub(crate) fn type_command(&mut self, id: &str, text: String, cx: &mut Context<Self>) {
        let typed = self.daemon.call_in_order::<Value>(Call::TerminalInput {
            terminal: id.to_string(),
            data: text,
        });
        let enter = Call::TerminalInput {
            terminal: id.to_string(),
            data: "\r".into(),
        };
        let daemon = self.daemon.clone();
        cx.spawn(async move |_, cx| {
            if typed.await.is_ok() {
                cx.background_executor().timer(SUBMIT_DELAY).await;
                daemon.fire(enter);
            }
        })
        .detach();
    }

    /// What a new conversation starts with: the model and effort the box
    /// shows (see [`Models::start_effort`]).
    pub(crate) fn new_options(&self, refs: Vec<Value>) -> AgentOptions {
        let c = &self.new_choice;
        let model = self.new_model().id;
        let effort = if terminal_only(self.new_kind) {
            None
        } else {
            self.models_of(self.new_kind)
                .start_effort(model.as_deref(), c.effort.as_deref())
        };
        AgentOptions {
            model,
            effort,
            permission: composer::mode(self.new_kind, c.mode).map(|m| m.permission.into()),
            code: c.console.unwrap_or(false).then(|| "console".into()),
            account: crate::prefs::agent_account(&self.host, self.new_kind),
            refs,
            ..AgentOptions::default()
        }
    }

    /// What an ended conversation goes on with: as it last ran, with what
    /// was picked since.
    pub(crate) fn resume_options(&self, t: &TerminalInfo) -> AgentOptions {
        let mut options = composer::live_options(t);
        let c = self.picked_for(t);
        if let Some(model) = c.model {
            options.model = model.id;
        }
        if let Some(effort) = c.effort {
            options.effort = Some(effort);
        }
        if let Some(mode) = c.mode.and_then(|p| composer::mode(&t.kind, Some(p))) {
            options.permission = Some(mode.permission.into());
        }
        if let Some(console) = c.console {
            options.code = console.then(|| "console".into());
        }
        if composer::running_models(t, options.model.as_deref())
            .variant_effort(options.model.as_deref())
            .is_some()
        {
            options.effort = None;
        }
        options
    }

    /// Starts the agent again on its conversation, with `options`: after it
    /// ended, or to change what it can't while it runs.
    pub(crate) fn restart(
        &mut self,
        t: &TerminalInfo,
        mut options: AgentOptions,
        prompt: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = t.id.clone();
        let name = crate::agents::kind_of(&t.kind).map_or("the agent", |k| k.name);
        let (prompt, first) = crate::agents::first_message(&t.kind, prompt, &mut options);
        let restarted = self.daemon.call::<TerminalInfo>(Call::RestartTerminal {
            terminal: id.clone(),
            options: Some(options),
            cols: Some(START_COLUMNS),
            rows: Some(START_ROWS),
            prompt,
        });
        self.starting = Some(name);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let restarted = restarted.await;
            this.update(cx, |this, cx| {
                this.starting = None;
                this.account_switching = None;
                match restarted {
                    Ok(info) => {
                        this.problem = None;
                        this.picked.remove(&id);
                        if let Some(first) = first {
                            this.typing.insert(id.clone(), first);
                        }
                        if let Some(view) = this.view(&id) {
                            view.update(cx, |v, cx| v.revive(cx));
                        }
                        if let Some(t) = this.terminals.iter_mut().find(|t| t.id == id) {
                            *t = info;
                        }
                        this.follow_again(&id, cx);
                        this.continue_resumed_view(&id, cx);
                    }
                    Err(err) => {
                        this.view_switches.remove(&id);
                        this.problem = Some(format!("Could not start {name} again: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Agents {
    pub(crate) fn dismiss_popups(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let open = self.menu.is_some() || self.gear;
        if open {
            self.close_menu(window, cx);
            self.gear = false;
            self.confirm_end = false;
            cx.notify();
        }
        open
    }

    pub(crate) fn toggle_gear(&mut self, cx: &mut Context<Self>) {
        self.gear = !self.gear;
        self.confirm_end = false;
        self.menu = None;
        cx.notify();
    }

    /// Names the conversation shown: its tab becomes a field.
    pub(crate) fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gear = false;
        let Some(t) = self.current().cloned() else {
            return;
        };
        let title = self.title(&t);
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Empty: named after its first message")
                .default_value(title)
        });
        let id = t.id.clone();
        self._rename_subscription = Some(cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { .. } => this.save_rename(cx),
                InputEvent::Blur => {
                    this.renaming = None;
                    cx.notify();
                }
                _ => {}
            },
        ));
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        self.renaming = Some((id, input));
        cx.notify();
    }

    fn save_rename(&mut self, cx: &mut Context<Self>) {
        let Some((id, input)) = self.renaming.take() else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
            t.name = (!name.is_empty()).then(|| name.clone());
        }
        let renamed = self.daemon.call::<TerminalInfo>(Call::RenameTerminal {
            terminal: id,
            name: (!name.is_empty()).then_some(name),
        });
        cx.spawn(async move |this, cx| {
            let renamed = renamed.await;
            this.update(cx, |this, cx| {
                this.problem = renamed
                    .err()
                    .map(|err| format!("Could not rename the conversation: {err:#}"));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Ends the conversation (the agent stops, with what it runs) and takes
    /// it out of the history; asks for a second click while it runs.
    pub(crate) fn end_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else {
            return;
        };
        if t.running && !self.confirm_end {
            self.confirm_end = true;
            cx.notify();
            return;
        }
        self.gear = false;
        self.confirm_end = false;
        self.forget_conversation(t.id.clone(), window, cx);
    }

    /// A conversation's delete button in the history: the first click asks,
    /// the second deletes it.
    pub(crate) fn delete_from_history(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.confirm_delete.as_deref() != Some(&id) {
            self.confirm_delete = Some(id);
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        let shown = self.show;
        self.forget_conversation(id, window, cx);
        // Deleting from the history stays in the history.
        if shown == Show::History && !self.terminals.is_empty() {
            self.show(Show::History, window, cx);
        }
    }

    /// Ends a conversation (the agent stops, with what it runs) and takes it
    /// out of the history, with its copy in the project.
    fn forget_conversation(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let closed = self.daemon.call::<Value>(Call::CloseTerminal {
            terminal: id.clone(),
        });
        self.terminals.retain(|t| t.id != id);
        self.chats.remove(&id);
        self.views.remove(&id);
        self.picked.remove(&id);
        self.drafts.remove(&id);
        self.selected = self
            .terminals
            .iter()
            .rev()
            .find(|t| t.running)
            .map(|t| t.id.clone());
        let next = if self.terminals.is_empty() {
            Show::New
        } else if self.selected.is_some() {
            Show::Chat
        } else {
            Show::History
        };
        self.show(next, window, cx);
        cx.spawn(async move |this, cx| {
            if let Err(err) = closed.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Could not end the conversation: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Starts a new conversation with the agent and choices an ended one had.
    pub(crate) fn start_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.current().cloned() else {
            return;
        };
        let Some(kind) = crate::agents::kind_of(&t.kind) else {
            return;
        };
        let options = self.resume_options(&t);
        self.start(kind.kind, None, options, window, cx);
    }
}

/// A starred model, as `kind:id`.
pub(crate) fn favourite_key(kind: &str, model: &ModelChoice) -> String {
    format!("{kind}:{}", model.id.as_deref().unwrap_or_default())
}

/// Saves a file from this computer in the project's attachments folder.
async fn attach_file(
    daemon: &crate::daemon::Daemon,
    folder: &str,
    path: &PathBuf,
) -> anyhow::Result<Value> {
    let bytes = std::fs::read(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().replace([':', '\\'], "-"))
        .unwrap_or_else(|| "attachment".into());
    let data = base64::engine::general_purpose::STANDARD.encode(bytes);
    save_attachment(daemon, folder, &name, data, true).await?;
    Ok(json!({"kind": "attach", "name": name, "path": format!("{ATTACHMENTS}/{name}")}))
}

/// Writes a file into the attachments folder, which keeps itself out of git.
async fn save_attachment(
    daemon: &crate::daemon::Daemon,
    folder: &str,
    name: &str,
    text: String,
    base64: bool,
) -> anyhow::Result<()> {
    daemon
        .call::<Value>(Call::WriteFile {
            path: format!("{folder}/{name}"),
            text,
            expected_mtime_ms: None,
            base64,
            create: false,
        })
        .await?;
    daemon
        .call::<Value>(Call::WriteFile {
            path: format!("{folder}/.gitignore"),
            text: "*\n".into(),
            expected_mtime_ms: None,
            base64: false,
            create: false,
        })
        .await?;
    Ok(())
}
