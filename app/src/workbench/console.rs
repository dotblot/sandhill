//! Unbound Console sessions and a bounded, non-destructive command index.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_daemon::outputs::OutputBuilder;
use sandhill_proto::{
    Call, Event, HistoryPage, SessionInfo, SessionRuns, SessionStatus, Subscribed,
};
use serde_json::{Value, json};

use super::{Question, Run, Session, Workbench};
use crate::look::{self, Palette, icon_button};

const HISTORY_STORED: usize = 100;
const HISTORY_SHOWN: usize = 50;

#[derive(Default)]
pub(crate) struct Commands(Vec<(String, String, i64)>);

impl Commands {
    fn push(&mut self, run: String, code: String, seq: i64) {
        if code.trim().is_empty() || self.0.iter().any(|(id, _, _)| *id == run) {
            return;
        }
        self.0.push((run, code, seq));
        let excess = self.0.len().saturating_sub(HISTORY_STORED);
        self.0.drain(..excess);
    }

    fn merge_snapshot(&mut self, older: impl IntoIterator<Item = (String, String)>, seq: i64) {
        let newer = std::mem::take(&mut self.0);
        for (run, code) in older {
            self.push(run, code, seq);
        }
        for (run, code, event_seq) in newer {
            if event_seq > seq {
                self.push(run, code, event_seq);
            }
        }
    }

    fn newest(&self) -> impl Iterator<Item = &(String, String, i64)> {
        self.0.iter().rev().take(HISTORY_SHOWN)
    }
}

/// One Up (`older`) or Down step through `history` (newest first). `recall`
/// is the recalled position and text; the walk only continues while `current`
/// still equals that text, or starts from an empty input. `None`: not a
/// history step (the key moves the cursor); `Some(None)`: empty the input.
fn history_step(
    history: &[&str],
    recall: Option<(usize, &str)>,
    current: &str,
    older: bool,
) -> Option<Option<usize>> {
    if current.is_empty() {
        return (older && !history.is_empty()).then_some(Some(0));
    }
    let (at, _) = recall.filter(|(_, text)| *text == current)?;
    let last = history.len().checked_sub(1)?;
    let at = at.min(last);
    Some(match (older, at) {
        (true, _) => Some((at + 1).min(last)),
        (false, 0) => None,
        (false, _) => Some(at - 1),
    })
}

/// The session's commands for Up/Down, newest first, without consecutive
/// duplicates.
fn history_codes(commands: &Commands) -> Vec<&str> {
    let mut codes: Vec<&str> = commands
        .0
        .iter()
        .rev()
        .map(|(_, code, _)| code.as_str())
        .collect();
    codes.dedup();
    codes
}

/// Unexecuted Console input per session, kept while the Console shows
/// another one. The shown session's draft lives in the input itself, so
/// running its command (which empties the input) clears it.
#[derive(Default)]
struct Drafts {
    saved: HashMap<String, String>,
    /// The session whose text the input holds.
    shown: Option<String>,
    /// Text the input is about to show, not yet set (that needs a window).
    pending: Option<String>,
}

impl Drafts {
    /// The Console moved to `to`, with `input` in the field: keeps that text
    /// for the session it was typed for and sets `pending` when the field
    /// must change. Text typed with no session (or kept through a restart)
    /// goes to the next session unless that one has a draft of its own.
    fn switch(&mut self, input: &str, to: Option<&str>) {
        if self.shown.as_deref() == to {
            return;
        }
        let current = self.pending.take().unwrap_or_else(|| input.to_string());
        let restored = to.and_then(|id| self.saved.remove(id));
        let next = match self.shown.take() {
            Some(from) => {
                if !current.is_empty() {
                    self.saved.insert(from, current);
                }
                restored.unwrap_or_default()
            }
            None => restored.unwrap_or(current),
        };
        self.shown = to.map(str::to_string);
        self.pending = (next != input).then_some(next);
    }

    /// A session stops: its draft goes, and if it is the shown one, its text
    /// stays in the input, unowned, for the next session.
    fn release(&mut self, id: &str) {
        self.saved.remove(id);
        if self.shown.as_deref() == Some(id) {
            self.shown = None;
        }
    }

    /// Forgets drafts of sessions that are no longer running.
    fn retain_live(&mut self, live: &[SessionInfo]) {
        self.saved.retain(|id, _| live.iter().any(|s| s.id == *id));
    }
}

#[derive(Default)]
pub(crate) struct Sessions {
    pub states: HashMap<String, Session>,
    starts: HashMap<String, u64>,
    pub live: Vec<SessionInfo>,
    /// Sessions explicitly used by this unbound Console, not the live inventory.
    pub retained: HashSet<String>,
    commands: HashMap<String, Commands>,
    loading: HashSet<String>,
    replay_cutoffs: HashMap<String, i64>,
    pub history_open: bool,
    recovering: bool,
    pending_pick: bool,
    pub followed_document: Option<(String, String)>,
    restart_documents: HashMap<String, Vec<String>>,
    pub picker_console: bool,
    pub variables_session: Option<String>,
    /// Up/Down history position and the recalled text in the Console input.
    recall: Option<(usize, String)>,
    drafts: Drafts,
}

impl Sessions {
    fn remembered_state(&self, kernel: &str, id: &str) -> Session {
        if self
            .live
            .iter()
            .any(|s| s.id == id && s.kernel == kernel && s.status == SessionStatus::Running)
        {
            Session::Ready(id.to_string())
        } else {
            Session::Failed("The selected session stopped. Select a live session or choose the kernel to start a new one.".into())
        }
    }

    fn complete_start(&mut self, kernel: &str, request: u64, state: Session) -> bool {
        if self.starts.get(kernel) != Some(&request) {
            return false;
        }
        self.starts.remove(kernel);
        if self
            .states
            .get(kernel)
            .is_some_and(|s| matches!(s, Session::Ready(_)))
        {
            return false;
        }
        self.states.insert(kernel.to_string(), state);
        true
    }

    fn accepts_replay(&self, event: &Event) -> bool {
        self.replay_cutoffs
            .get(&event.session)
            .is_none_or(|seq| event.seq > *seq)
    }
}
/// Historical source is parsed once, not once per frame or through an editor.
pub(super) struct HighlightedCode {
    text: SharedString,
    language: &'static str,
    theme: Arc<HighlightTheme>,
    styles: crate::editor::output_text::Highlights,
}

impl HighlightedCode {
    fn new(code: &str, language: &'static str, theme: Arc<HighlightTheme>) -> Self {
        Self {
            text: SharedString::from(code),
            language,
            styles: crate::editor::output_text::code_highlights(code, language, &theme),
            theme,
        }
    }
}

impl Run {
    /// The run's code in the editor's colours, selectable and copyable like
    /// its output (Huan, 2026-11-16).
    pub(crate) fn highlighted_code(
        &self,
        id: impl Into<ElementId>,
        language: &'static str,
        cx: &App,
    ) -> crate::editor::output_text::OutputText {
        let code = self.code.trim_end();
        let theme = &cx.theme().highlight_theme;
        let mut cached = self.code_highlights.borrow_mut();
        let cached =
            cached.get_or_insert_with(|| HighlightedCode::new(code, language, theme.clone()));
        if cached.text.as_ref() != code
            || cached.language != language
            || !Arc::ptr_eq(&cached.theme, theme)
        {
            *cached = HighlightedCode::new(code, language, theme.clone());
        }
        crate::editor::output_text::OutputText::new(id, cached.text.clone())
            .highlights(cached.styles.clone())
    }

    pub(super) fn append_output(&mut self, seq: i64, msg_type: String, content: Value) {
        self.events.insert(seq, (msg_type, content));
        self.rebuild_output();
    }

    fn install_output_replay(&mut self, seq: i64, events: BTreeMap<i64, (String, Value)>) {
        self.events.retain(|event_seq, _| *event_seq > seq);
        self.events.extend(events);
        self.output_replay_seq = seq;
        self.rebuild_output();
        self.question_from_events();
    }

    /// The question still open: the last asked with no answer after it,
    /// while the run goes on.
    pub(super) fn question_from_events(&mut self) {
        self.asking = None;
        if self.finished {
            return;
        }
        for (msg_type, content) in self.events.values() {
            match msg_type.as_str() {
                sandhill_proto::EVENT_INPUT_REQUEST => {
                    self.asking = Some(Question {
                        prompt: content["prompt"].as_str().unwrap_or_default().to_string(),
                        password: content["password"].as_bool().unwrap_or(false),
                    })
                }
                sandhill_proto::EVENT_INPUT_REPLY => self.asking = None,
                _ => {}
            }
        }
    }

    fn rebuild_output(&mut self) {
        if self.output_replay_seq >= self.snapshot_seq {
            let mut builder = OutputBuilder::new();
            for (msg_type, content) in self.events.values() {
                builder.push(msg_type, content);
            }
            self.outputs = builder.finish();
        }
    }
}

impl Workbench {
    /// The Console target is independent of the document being edited.
    pub(crate) fn console_target(&self) -> Option<&str> {
        self.loose.id()
    }

    /// The Console's session's open question, and the run asking it.
    pub(crate) fn console_question(&self) -> Option<(&str, &Question)> {
        let session = self.console_target()?;
        self.runs.iter().find_map(|(id, run)| {
            (run.session == session && !run.finished)
                .then_some(run.asking.as_ref())
                .flatten()
                .map(|question| (id.as_str(), question))
        })
    }

    /// Sends what was typed as the answer to the Console's open question.
    /// An empty answer counts: in R's `browser()` it means "go on".
    fn answer_console(&mut self, value: String, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.console_target().map(str::to_string))
        else {
            return;
        };
        // Answered: a second Enter must not answer the next question
        // before it is asked.
        if let Some(run) = self.console_question().map(|(run, _)| run.to_string())
            && let Some(run) = self.runs.get_mut(&run)
        {
            run.asking = None;
        }
        let answered = daemon.call_in_order::<Value>(Call::Input { session, value });
        cx.spawn(async move |this, cx| {
            if let Err(err) = answered.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Answering the code failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        self.problem = None;
    }

    /// Enter in the Console input while the code waits for an answer.
    /// Returns false when nothing is being asked.
    pub(crate) fn answer_console_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.console_question().is_none_or(|(_, q)| q.password) {
            return false;
        }
        let value = self.console_input.read(cx).value().to_string();
        self.console_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.answer_console(value, cx);
        true
    }

    pub(crate) fn answer_console_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.console_question().is_some_and(|(_, q)| q.password) {
            return;
        }
        let value = self.console_password.read(cx).value().to_string();
        self.console_password
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.answer_console(value, cx);
    }

    /// A password question takes focus to its hidden field, and gives it
    /// back to the Console input once answered, when the Console had it.
    pub(crate) fn sync_console_question(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let asking = self
            .console_question()
            .filter(|(_, q)| q.password)
            .map(|(run, _)| run.to_string());
        if asking == self.console_password_for {
            return;
        }
        let had_focus = self
            .console_input
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
            || self
                .console_password
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx);
        if had_focus {
            match asking {
                Some(_) => self
                    .console_password
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx),
                None => self
                    .console_input
                    .read(cx)
                    .focus_handle(cx)
                    .focus(window, cx),
            }
        }
        self.console_password_for = asking;
    }

    pub(crate) fn console_bound_file(&self) -> Option<&str> {
        let id = self.console_target()?;
        self.open
            .iter()
            .chain(&self.others)
            .find(|open| open.session.id() == Some(id))
            .map(|open| open.notebook.path.as_str())
    }

    pub(crate) fn bind_console_document(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some((id, kernel)) = self
            .open
            .iter()
            .chain(&self.others)
            .find(|open| open.notebook.path == path)
            .and_then(|open| Some((open.session.id()?.to_string(), open.kernel.clone()?)))
        else {
            return;
        };
        if let Some(previous) = &self.loose_kernel {
            self.console_sessions
                .states
                .insert(previous.clone(), self.loose.clone());
        }
        self.loose = Session::Ready(id.clone());
        self.loose_kernel = Some(kernel.clone());
        self.console_sessions
            .states
            .insert(kernel, self.loose.clone());
        self.console_sessions.retained.insert(id);
        self.save_console_choice();
        self.session_changed(cx);
    }

    pub(crate) fn new_console_kernel(&mut self, kernel: String, cx: &mut Context<Self>) {
        self.console_sessions.states.remove(&kernel);
        if let Some(sessions) = self.project_state["console"]["sessions"].as_object_mut() {
            sessions.remove(&kernel);
        }
        self.loose_kernel = Some(kernel);
        self.loose = Session::None;
        self.save_console_choice();
        self.session_changed(cx);
        self.start_console_session(cx);
    }

    pub(crate) fn restart_console_session(&mut self, cx: &mut Context<Self>) {
        let Some(kernel) = self.loose_kernel.clone() else {
            return;
        };
        let paths = self
            .open
            .iter()
            .chain(&self.others)
            .filter(|open| open.session.id() == self.console_target())
            .map(|open| open.notebook.path.clone())
            .collect();
        self.console_sessions
            .restart_documents
            .insert(kernel, paths);
        self.stop_console_session(cx);
        self.start_console_session(cx);
    }
    pub(super) fn save_console_choice(&mut self) {
        if !self.project_state.is_object() {
            self.project_state = json!({});
        }
        let mut sessions = self.project_state["console"]["sessions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (kernel, state) in &self.console_sessions.states {
            if let Some(id) = state.id() {
                sessions.insert(kernel.clone(), json!(id));
            }
        }
        self.project_state["console"] = json!({
            "kernel": self.loose_kernel,
            "sessions": sessions,
            "retained": self.console_sessions.retained,
        });
        if let (Some(daemon), Some(path)) = (self.daemon(), self.project.clone()) {
            daemon.fire(Call::SaveProjectState {
                path,
                state: self.project_state.clone(),
            });
        }
    }

    pub(crate) fn recover_console_sessions(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.console_sessions = Sessions::default();
        self.console_sessions.recovering = true;
        self.loose = Session::None;
        self.loose_kernel = self.project_state["console"]["kernel"]
            .as_str()
            .map(str::to_string);
        let generation = self.generation;
        let listed = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project.as_deref() != Some(&project) {
                    return;
                }
                this.console_sessions.recovering = false;
                match listed {
                    Ok(sessions) => {
                        this.console_sessions.live = sessions
                            .into_iter()
                            .filter(|s| s.status == SessionStatus::Running)
                            .collect();
                        let remembered = this.project_state["console"]["sessions"]
                            .as_object()
                            .cloned()
                            .unwrap_or_default();
                        for (kernel, id) in remembered {
                            let Some(id) = id.as_str() else { continue };
                            let state = this.console_sessions.remembered_state(&kernel, id);
                            this.console_sessions.states.insert(kernel, state);
                        }
                        if let Some(kernel) = this.loose_kernel.clone() {
                            this.loose = this
                                .console_sessions
                                .states
                                .get(&kernel)
                                .cloned()
                                .unwrap_or_else(|| {
                                    this.console_sessions
                                        .live
                                        .iter()
                                        .filter(|s| s.kernel == kernel)
                                        .max_by_key(|s| s.created_ms)
                                        .map(|s| Session::Ready(s.id.clone()))
                                        .unwrap_or_default()
                                });
                            this.console_sessions
                                .states
                                .insert(kernel, this.loose.clone());
                        }
                        if let Some(retained) = this.project_state["console"]["retained"].as_array()
                        {
                            for id in retained.iter().filter_map(Value::as_str) {
                                if this.console_sessions.live.iter().any(|s| s.id == id) {
                                    this.console_sessions.retained.insert(id.to_string());
                                }
                            }
                        }
                        this.console_sessions.retained.extend(
                            this.console_sessions
                                .states
                                .values()
                                .filter_map(|s| s.id().map(str::to_string)),
                        );
                        let ids: Vec<_> = this.console_sessions.retained.iter().cloned().collect();
                        for id in ids {
                            this.follow_console_session(id, cx);
                        }
                        this.session_changed(cx);
                        if std::mem::take(&mut this.console_sessions.pending_pick)
                            && let Some(kernel) = this.loose_kernel.clone()
                        {
                            this.choose_console_kernel(kernel, cx);
                        }
                    }
                    Err(err) => {
                        this.loose =
                            Session::Failed(format!("Listing retained sessions failed: {err:#}"));
                        this.problem = Some(format!("Listing retained sessions failed: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn choose_console_kernel(&mut self, kernel: String, cx: &mut Context<Self>) {
        self.console_sessions.pending_pick = self.console_sessions.recovering;
        if let Some(previous) = &self.loose_kernel {
            self.console_sessions
                .states
                .insert(previous.clone(), self.loose.clone());
        }
        self.loose_kernel = Some(kernel.clone());
        self.loose = self
            .console_sessions
            .states
            .get(&kernel)
            .cloned()
            .unwrap_or_else(|| {
                self.console_sessions
                    .live
                    .iter()
                    .filter(|s| s.kernel == kernel && s.status == SessionStatus::Running)
                    .max_by_key(|s| s.created_ms)
                    .map(|s| Session::Ready(s.id.clone()))
                    .unwrap_or_default()
            });
        self.console_sessions
            .states
            .insert(kernel, self.loose.clone());
        if let Some(id) = self.loose.id() {
            self.console_sessions.retained.insert(id.to_string());
        }
        self.save_console_choice();
        self.session_changed(cx);
        if !self.console_sessions.recovering
            && matches!(self.loose, Session::None | Session::Failed(_))
        {
            self.start_console_session(cx);
        }
        cx.notify();
    }

    pub(crate) fn select_console_session(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(info) = self
            .console_sessions
            .live
            .iter()
            .chain(&self.machine.sessions)
            .find(|s| s.id == id && s.status == SessionStatus::Running)
            .cloned()
        else {
            self.problem = Some(
                "That session is no longer running. Choose another session explicitly.".into(),
            );
            cx.notify();
            return;
        };
        if let Some(previous) = &self.loose_kernel {
            self.console_sessions
                .states
                .insert(previous.clone(), self.loose.clone());
        }
        self.loose_kernel = Some(info.kernel.clone());
        self.console_sessions.retained.insert(info.id.clone());
        self.loose = Session::Ready(info.id);
        self.console_sessions
            .states
            .insert(info.kernel, self.loose.clone());
        self.save_console_choice();
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn start_console_session(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project), Some(kernel)) = (
            self.daemon(),
            self.project.clone(),
            self.loose_kernel.clone(),
        ) else {
            return;
        };
        if self.restricted()
            || self.console_sessions.recovering
            || matches!(self.loose, Session::Starting | Session::Ready(_))
        {
            return;
        }
        if !self.kernels.iter().any(|spec| spec.name == kernel) {
            self.loose =
                Session::Failed(format!("Kernel “{kernel}” is unavailable on this device."));
            cx.notify();
            return;
        }
        self.loose_start = self.loose_start.wrapping_add(1);
        let request = self.loose_start;
        self.console_sessions.starts.insert(kernel.clone(), request);
        self.console_sessions
            .states
            .insert(kernel.clone(), Session::Starting);
        self.loose = Session::Starting;
        let generation = self.generation;
        let started = daemon.call::<SessionInfo>(Call::StartSession {
            kernel: kernel.clone(),
            cwd: Some(project.clone()),
            project: Some(project.clone()),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                if this.generation != generation
                    || this.project.as_deref() != Some(&project)
                    || this.console_sessions.starts.get(&kernel) != Some(&request)
                {
                    // A detached start remains discoverable on the daemon. A changed
                    // selection is not permission to destroy a running session.
                    return;
                }
                let state = match started {
                    Ok(info) => {
                        let id = info.id.clone();
                        this.console_sessions.retained.insert(id.clone());
                        this.console_sessions.live.push(info);
                        this.follow_console_session(id.clone(), cx);
                        Session::Ready(id)
                    }
                    Err(err) => Session::Failed(format!("{err:#}")),
                };
                let accepted =
                    this.console_sessions
                        .complete_start(&kernel, request, state.clone());
                if accepted {
                    if let Some(paths) = this.console_sessions.restart_documents.remove(&kernel) {
                        for path in paths {
                            if let Some(open) = this.open_at(&path)
                                && matches!(open.session, Session::None)
                            {
                                open.session = state.clone();
                            }
                        }
                    }
                    if this.loose_kernel.as_deref() == Some(&kernel) {
                        this.loose = state;
                        this.session_changed(cx);
                    }
                }
                this.save_console_choice();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn stop_console_session(&mut self, cx: &mut Context<Self>) {
        let Some(kernel) = self.loose_kernel.clone() else {
            return;
        };
        if let Some(id) = self.loose.id().map(str::to_string) {
            for open in self.open.iter_mut().chain(&mut self.others) {
                if open.session.id() == Some(id.as_str()) {
                    open.session = Session::None;
                }
            }
            self.stop_kernel(id.clone(), cx);
            self.console_sessions.live.retain(|s| s.id != id);
            self.console_sessions.drafts.release(&id);
        }
        self.console_sessions.starts.remove(&kernel);
        self.console_sessions
            .states
            .insert(kernel.clone(), Session::None);
        if let Some(sessions) = self.project_state["console"]["sessions"].as_object_mut() {
            sessions.remove(&kernel);
        }
        self.loose = Session::None;
        self.save_console_choice();
        self.session_changed(cx);
        cx.notify();
    }

    pub(crate) fn accepts_console_replay(&self, event: &Event) -> bool {
        self.console_sessions.accepts_replay(event)
    }

    pub(crate) fn refresh_console_sessions(&mut self, cx: &mut Context<Self>) {
        if self.console_sessions.recovering {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let generation = self.generation;
        let known: HashSet<_> = self
            .console_sessions
            .live
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let listed = daemon.call::<Vec<SessionInfo>>(Call::ListSessions);
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project.as_deref() != Some(&project) {
                    return;
                }
                match listed {
                    Ok(sessions) => {
                        let mut live: Vec<_> = sessions
                            .into_iter()
                            .filter(|s| s.status == SessionStatus::Running)
                            .collect();
                        // A start completed after this list was requested.
                        for session in &this.console_sessions.live {
                            if !known.contains(&session.id)
                                && !live.iter().any(|s| s.id == session.id)
                            {
                                live.push(session.clone());
                            }
                        }
                        this.console_sessions.live = live;
                        let sessions = &mut this.console_sessions;
                        sessions.drafts.retain_live(&sessions.live);
                        let selected: Vec<_> = this
                            .console_sessions
                            .states
                            .iter()
                            .filter_map(|(kernel, state)| {
                                state.id().map(|id| (kernel.clone(), id.to_string()))
                            })
                            .collect();
                        for (kernel, id) in selected {
                            let state = this.console_sessions.remembered_state(&kernel, &id);
                            this.console_sessions
                                .states
                                .insert(kernel.clone(), state.clone());
                            if this.loose_kernel.as_deref() == Some(&kernel) {
                                this.loose = state;
                            }
                        }
                    }
                    Err(err) => {
                        this.problem = Some(format!("Listing live sessions failed: {err:#}"))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The journal remains authoritative and unbounded by the command index.
    /// A separate journal pass reconstructs output prefixes before live replay.
    pub(crate) fn follow_console_session(&mut self, session: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        // Deduplicate concurrent loads, not future visits. A closed document's
        // subscription can have missed commands issued by another client.
        if !self.console_sessions.loading.insert(session.clone()) {
            return;
        }
        let generation = self.generation;
        let project = self.project.clone();
        let history = daemon.call::<SessionRuns>(Call::SessionRuns {
            session: session.clone(),
            limit: Some(HISTORY_STORED as u32),
        });
        cx.spawn(async move |this, cx| {
            let history = history.await;
            let snapshot = this
                .update(cx, |this, cx| {
                    if this.generation != generation || this.project != project {
                        return None;
                    }
                    let history = match history {
                        Ok(history) => history,
                        Err(err) => {
                            this.console_sessions.loading.remove(&session);
                            this.problem = Some(format!("Loading session history failed: {err:#}"));
                            cx.notify();
                            return None;
                        }
                    };
                    let ids: HashSet<_> = history.runs.iter().map(|r| r.run.clone()).collect();
                    this.console_sessions
                        .replay_cutoffs
                        .insert(session.clone(), history.seq);
                    let commands = history.runs.iter().map(|r| (r.run.clone(), r.code.clone()));
                    this.console_sessions
                        .commands
                        .entry(session.clone())
                        .or_default()
                        .merge_snapshot(commands, history.seq);
                    let mut outputs = Vec::new();
                    let language = this
                        .session_language(&session)
                        .map(|language| super::editor_language(Some(language)));
                    for saved in history.runs {
                        outputs.extend(saved.outputs.iter().cloned());
                        let run = this.runs.entry(saved.run).or_default();
                        if run.last_seq > history.seq {
                            continue;
                        }
                        run.code = saved.code;
                        run.language = language.or(run.language);
                        run.cell = saved.cell.map(|c| (c.file, c.cell_id));
                        run.by = saved.by;
                        run.outputs = saved.outputs;
                        run.finished = saved.finished_ms.is_some();
                        run.status = saved.status;
                        run.execution_count = saved.execution_count;
                        run.session = session.clone();
                        run.queued_ms = Some(saved.started_ms);
                        run.finished_ms = saved.finished_ms;
                        run.snapshot_seq = history.seq;
                        run.last_seq = history.seq;
                    }
                    if this.console_session.as_deref() == Some(&session) {
                        this.restore_console_runs(&session);
                        this.console_scroll.scroll_to_bottom();
                    }
                    this.fetch_plots(&outputs, cx);
                    cx.notify();
                    Some((history.seq, ids))
                })
                .ok()
                .flatten();
            let Some((snapshot_seq, ids)) = snapshot else {
                return;
            };
            // Reconstruct independently of live delivery and its last_seq. A
            // notebook subscription may already have delivered a later message.
            let reconstructed: anyhow::Result<_> = async {
                let mut replay: HashMap<String, BTreeMap<i64, (String, Value)>> =
                    ids.into_iter().map(|id| (id, BTreeMap::new())).collect();
                let mut since_seq = 0;
                loop {
                    let page = daemon
                        .call::<HistoryPage>(Call::History {
                            session: session.clone(),
                            since_seq,
                            limit: Some(5000),
                        })
                        .await?;
                    for event in page.events {
                        if event.seq > snapshot_seq {
                            break;
                        }
                        if let Some(events) = event.run.as_ref().and_then(|id| replay.get_mut(id)) {
                            events.insert(event.seq, (event.msg_type, event.content));
                        }
                    }
                    if page.next_seq >= snapshot_seq || page.next_seq <= since_seq {
                        break;
                    }
                    since_seq = page.next_seq;
                }
                Ok(replay)
            }
            .await;
            let installed = this
                .update(cx, |this, cx| {
                    if this.generation != generation || this.project != project {
                        return false;
                    }
                    let replay = match reconstructed {
                        Ok(replay) => replay,
                        Err(err) => {
                            this.console_sessions.loading.remove(&session);
                            this.problem =
                                Some(format!("Reconstructing session output failed: {err:#}"));
                            cx.notify();
                            return false;
                        }
                    };
                    let mut outputs = Vec::new();
                    for (id, events) in replay {
                        if let Some(run) = this.runs.get_mut(&id) {
                            run.install_output_replay(snapshot_seq, events);
                            outputs.extend(run.outputs.iter().cloned());
                        }
                    }
                    this.fetch_plots(&outputs, cx);
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !installed {
                return;
            }
            let subscribed = daemon
                .call::<Subscribed>(Call::Subscribe {
                    session: session.clone(),
                    since_seq: Some(snapshot_seq),
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation == generation && this.project == project {
                    this.console_sessions.loading.remove(&session);
                    if let Err(err) = subscribed {
                        this.problem = Some(format!("Following the kernel failed: {err:#}"));
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn restore_console_runs(&mut self, session: &str) {
        let mut runs: Vec<_> = self
            .runs
            .iter()
            .filter(|(_, r)| r.session == session && !r.quiet)
            .map(|(id, r)| (r.queued_ms.unwrap_or_default(), id.clone()))
            .collect();
        runs.sort();
        let skip = runs.len().saturating_sub(super::CONSOLE_RUNS);
        self.console = runs.into_iter().skip(skip).map(|(_, id)| id).collect();
    }

    pub(crate) fn record_console_command(
        &mut self,
        session: &str,
        run: String,
        code: String,
        seq: i64,
    ) {
        self.console_sessions
            .commands
            .entry(session.to_string())
            .or_default()
            .push(run, code, seq);
    }

    /// The Console moved to another session: keeps the unexecuted input for
    /// the one it was typed for and shows the new one's (empty if none).
    /// Setting the input needs the window, so that waits for this update to
    /// finish; a later switch in the meantime takes over its text.
    pub(crate) fn switch_console_draft(&mut self, cx: &mut Context<Self>) {
        self.console_sessions.recall = None;
        let input = self.console_input.read(cx).value().to_string();
        let to = self.console_target().map(str::to_string);
        self.console_sessions.drafts.switch(&input, to.as_deref());
        if self.console_sessions.drafts.pending.is_none() {
            return;
        }
        let (handle, this) = (self.window_handle, cx.weak_entity());
        cx.defer(move |cx| {
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| {
                    let Some(text) = this.console_sessions.drafts.pending.take() else {
                        return;
                    };
                    this.console_input.update(cx, |input, cx| {
                        input.set_value(text, window, cx);
                        let end = input.text().offset_to_position(input.text().len());
                        input.set_cursor_position(end, window, cx);
                    });
                })
                .ok();
            })
            .ok();
        });
    }

    /// Up/Down in the Console input: walks the session's history from an
    /// empty input or an unedited recalled command. Returns false when the key
    /// should move the cursor instead (including while completion is open).
    pub(crate) fn console_history_step(
        &mut self,
        older: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let input = self.console_input.read(cx);
        if input.completion_menu_state().open {
            return false;
        }
        let current = input.value();
        let history = self
            .console_target()
            .and_then(|id| self.console_sessions.commands.get(id))
            .map(history_codes)
            .unwrap_or_default();
        let recall = self.console_sessions.recall.as_ref();
        let Some(step) = history_step(
            &history,
            recall.map(|(at, text)| (*at, text.as_str())),
            &current,
            older,
        ) else {
            return false;
        };
        let text = step.map_or(String::new(), |at| history[at].to_string());
        self.console_sessions.recall = step.map(|at| (at, text.clone()));
        self.console_input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            let end = input.text().offset_to_position(input.text().len());
            input.set_cursor_position(end, window, cx);
        });
        true
    }

    pub(crate) fn console_history_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let open = self.console_sessions.history_open;
        let entries: Vec<_> = self
            .console_target()
            .filter(|_| open)
            .and_then(|id| self.console_sessions.commands.get(id))
            .map(|history| history.newest().cloned().collect())
            .unwrap_or_default();
        let loading = self.console_target().is_some_and(|id| {
            self.console_sessions.loading.contains(id)
                && !self.console_sessions.commands.contains_key(id)
        });
        div()
            .relative()
            .flex_none()
            .child(
                icon_button("console-history", "history", &p, open)
                    .on_prepaint(self.popup_triggers.track("console-history"))
                    .tooltip(|w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Command history, newest 50")
                            .build(w, cx)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.console_sessions.history_open = !this.console_sessions.history_open;
                        this.kernel_menu = false;
                        cx.notify();
                    })),
            )
            .when(open, |d| {
                d.child(
                    div().absolute().right_0().top(px(24.)).child(
                        deferred(
                            anchored().snap_to_window_with_margin(px(8.)).child(
                                crate::agents::composer_view::popup(&p)
                                    .id("console-history-menu")
                                    .occlude()
                                    .w(px(360.))
                                    .max_h(px(360.))
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .text_color(p.muted)
                                            .child("Command history · newest first"),
                                    )
                                    .when(entries.is_empty(), |d| {
                                        d.child(div().px_2().py_1().text_color(p.muted).child(
                                            if loading {
                                                "Loading command history…"
                                            } else {
                                                "No commands in this session."
                                            },
                                        ))
                                    })
                                    .children(entries.into_iter().enumerate().map(
                                        |(ix, (_, code, _))| {
                                            let hover = p.ink(7.);
                                            h_flex()
                                                .id(("console-history-entry", ix))
                                                .px_2()
                                                .py_1()
                                                .rounded(px(6.))
                                                .cursor_pointer()
                                                .hover(move |d| d.bg(hover))
                                                .child(
                                                    v_flex()
                                                        .min_w_0()
                                                        .font_family(look::MONO)
                                                        .text_size(px(look::SMALL))
                                                        .child(code.clone()),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.console_input.update(
                                                            cx,
                                                            |input, cx| {
                                                                input.set_value(
                                                                    code.clone(),
                                                                    window,
                                                                    cx,
                                                                );
                                                                input.focus(window, cx);
                                                            },
                                                        );
                                                        this.console_sessions.history_open = false;
                                                        cx.notify();
                                                    },
                                                ))
                                        },
                                    ))
                                    .on_mouse_down_out(cx.listener(
                                        |this, event: &MouseDownEvent, _, cx| {
                                            if !this
                                                .popup_triggers
                                                .contains("console-history", event.position)
                                            {
                                                this.console_sessions.history_open = false;
                                                cx.notify();
                                            }
                                        },
                                    )),
                            ),
                        )
                        .with_priority(3),
                    ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Commands, Drafts, HighlightedCode, Run, Session, Sessions, history_codes, history_step,
    };
    use gpui_kit::component::highlighter::HighlightTheme;
    use sandhill_proto::{Event, SessionInfo, SessionStatus};
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// Switches as the Console does, then applies the text it asks for.
    fn switch(drafts: &mut Drafts, input: &mut String, to: Option<&str>) {
        drafts.switch(input, to);
        if let Some(text) = drafts.pending.take() {
            *input = text;
        }
    }

    #[test]
    fn console_drafts_follow_the_session() {
        let mut drafts = Drafts::default();
        let mut input = String::new();
        switch(&mut drafts, &mut input, Some("a"));
        input.push_str("x <- 1");
        switch(&mut drafts, &mut input, Some("b"));
        assert_eq!(input, "");
        input.push_str("print(y)");
        switch(&mut drafts, &mut input, Some("a"));
        assert_eq!(input, "x <- 1");
        switch(&mut drafts, &mut input, Some("b"));
        assert_eq!(input, "print(y)");
        // Running empties the input, and with it the draft.
        input.clear();
        switch(&mut drafts, &mut input, Some("a"));
        switch(&mut drafts, &mut input, Some("b"));
        assert_eq!(input, "");
        // Starting a new kernel passes through no session: the old text is
        // kept and the new session starts empty.
        switch(&mut drafts, &mut input, Some("a"));
        switch(&mut drafts, &mut input, None);
        assert_eq!(input, "");
        switch(&mut drafts, &mut input, Some("c"));
        assert_eq!(input, "");
        switch(&mut drafts, &mut input, Some("a"));
        assert_eq!(input, "x <- 1");
    }

    #[test]
    fn console_drafts_survive_quick_switches_and_restarts() {
        let mut drafts = Drafts::default();
        // Typed before any session: goes to the first one.
        let mut input = String::from("library(x)");
        switch(&mut drafts, &mut input, Some("a"));
        assert_eq!(input, "library(x)");
        // Two switches before the input is set: "b" never held "library(x)".
        drafts.switch(&input, Some("b"));
        drafts.switch(&input, Some("c"));
        assert_eq!(drafts.pending.take().as_deref(), Some(""));
        input.clear();
        switch(&mut drafts, &mut input, Some("b"));
        assert_eq!(input, "");
        switch(&mut drafts, &mut input, Some("a"));
        assert_eq!(input, "library(x)");
        // A restart stops "a": its text stays for the new session.
        drafts.release("a");
        switch(&mut drafts, &mut input, None);
        switch(&mut drafts, &mut input, Some("a2"));
        assert_eq!(input, "library(x)");
        assert!(drafts.saved.is_empty());
    }

    #[test]
    fn console_code_highlights_python_r_and_unicode_with_the_editor_theme() {
        crate::register_r();
        for theme in [
            HighlightTheme::default_light(),
            HighlightTheme::default_dark(),
        ] {
            // Sandhill adds these two captures to Kit's built-in themes.
            let mut themed = theme.as_ref().clone();
            themed.style.syntax.primary = themed.style.syntax.keyword;
            themed.style.syntax.hint = themed.style.syntax.function;
            let theme = Arc::new(themed);
            for (language, code, captures) in [
                (
                    "python",
                    "import os\nprint(\"étude\", count=42)\n# 中文",
                    [
                        ("import", "primary"),
                        ("42", "number"),
                        ("# 中文", "comment"),
                    ],
                ),
                (
                    "r",
                    "library(ggplot2)\nx <- 42\n# 中文",
                    [("library", "hint"), ("42", "number"), ("# 中文", "comment")],
                ),
            ] {
                let highlighted = HighlightedCode::new(code, language, theme.clone());
                assert_eq!(highlighted.text.as_ref(), code);
                for (token, capture) in captures {
                    let offset = code.find(token).unwrap();
                    let expected = theme.style(capture).unwrap();
                    assert!(
                        highlighted.styles.iter().any(|(range, style)| {
                            range.contains(&offset) && style == &expected
                        }),
                        "{language}: {token} should use {capture}"
                    );
                }
                assert!(highlighted.styles.iter().all(|(range, _)| {
                    code.is_char_boundary(range.start) && code.is_char_boundary(range.end)
                }));
            }
            let plain = HighlightedCode::new("print(42)", "text", theme);
            assert!(plain.styles.is_empty());
        }
    }

    #[test]
    fn console_reconstruction_preserves_display_ids_with_live_update_already_buffered() {
        let snapshot = json!({
            "output_type":"display_data", "data":{"text/plain":"before"}, "metadata":{},
        });
        let mut run = Run {
            snapshot_seq: 10,
            outputs: vec![snapshot.clone()],
            ..Default::default()
        };
        run.append_output(
            11,
            "update_display_data".into(),
            json!({
                "data":{"text/plain":"after"}, "metadata":{}, "transient":{"display_id":"plot"},
            }),
        );
        assert_eq!(run.outputs, [snapshot]);
        run.install_output_replay(10, BTreeMap::from([
            (1, ("display_data".into(), json!({
                "data":{"text/plain":"before"}, "metadata":{}, "transient":{"display_id":"plot"},
            }))),
        ]));
        assert_eq!(run.outputs.len(), 1);
        assert_eq!(run.outputs[0]["data"]["text/plain"], "after");
    }

    #[test]
    fn console_missing_selected_session_never_falls_back_to_another_live_session() {
        let mut sessions = Sessions::default();
        sessions.live.push(SessionInfo {
            id: "different-live-session".into(),
            kernel: "r".into(),
            language: "r".into(),
            cwd: "/project".into(),
            pid: None,
            status: SessionStatus::Running,
            created_ms: 0,
            ended_ms: None,
            last_seq: 0,
            project: Some("/project".into()),
            pending_runs: 0,
            language_version: None,
        });
        assert!(matches!(
            sessions.remembered_state("r", "missing-selection"),
            Session::Failed(_)
        ));
        assert_eq!(
            sessions
                .remembered_state("r", "different-live-session")
                .id(),
            Some("different-live-session")
        );
    }

    #[test]
    fn console_busy_output_reconstruction_keeps_snapshot_and_buffers_later_live_events() {
        let snapshot = json!({"output_type": "stream", "name": "stdout", "text": "first second "});
        let mut run = Run {
            last_seq: 3,
            snapshot_seq: 2,
            outputs: vec![snapshot.clone()],
            ..Default::default()
        };
        // A live message can arrive before reconstruction, and its sequence
        // must not prevent installation of the missing earlier messages.
        run.append_output(3, "stream".into(), json!({"name":"stdout", "text":"third"}));
        assert_eq!(run.outputs, [snapshot]);
        run.install_output_replay(
            2,
            BTreeMap::from([
                (
                    1,
                    ("stream".into(), json!({"name":"stdout", "text":"first "})),
                ),
                (
                    2,
                    ("stream".into(), json!({"name":"stdout", "text":"second "})),
                ),
            ]),
        );
        assert_eq!(run.outputs[0]["text"], "first second third");
        // Subscription replay and live delivery share sequence identity.
        run.append_output(3, "stream".into(), json!({"name":"stdout", "text":"third"}));
        assert_eq!(run.outputs[0]["text"], "first second third");
        run.append_output(4, "clear_output".into(), json!({"wait":false}));
        assert!(run.outputs.is_empty());
    }

    /// A Console reopened while code waits for an answer shows the open
    /// question, not one already answered, and none once the run ended.
    #[test]
    fn console_rebuilt_from_the_journal_shows_only_the_open_question() {
        let asked = |seq: i64, prompt: &str| {
            (
                seq,
                (
                    sandhill_proto::EVENT_INPUT_REQUEST.to_string(),
                    json!({"prompt": prompt, "password": false, "header": {}}),
                ),
            )
        };
        let answered = |seq: i64| {
            (
                seq,
                (sandhill_proto::EVENT_INPUT_REPLY.to_string(), json!({})),
            )
        };
        let echo = (
            2,
            (
                "stream".to_string(),
                json!({"name": "stdout", "text": "Browse[1]> n\n"}),
            ),
        );
        let mut run = Run {
            snapshot_seq: 4,
            ..Default::default()
        };
        run.install_output_replay(
            4,
            BTreeMap::from([
                asked(1, "Browse[1]> "),
                echo.clone(),
                answered(3),
                asked(4, "Browse[2]> "),
            ]),
        );
        assert_eq!(run.asking.as_ref().unwrap().prompt, "Browse[2]> ");
        // The question events are not outputs.
        assert_eq!(run.outputs.len(), 1, "{:?}", run.outputs);

        let mut run = Run {
            snapshot_seq: 3,
            ..Default::default()
        };
        run.install_output_replay(
            3,
            BTreeMap::from([asked(1, "name? "), echo.clone(), answered(3)]),
        );
        assert!(run.asking.is_none());

        let mut run = Run {
            snapshot_seq: 1,
            finished: true,
            ..Default::default()
        };
        run.install_output_replay(1, BTreeMap::from([asked(1, "name? ")]));
        assert!(run.asking.is_none());
    }

    #[test]
    fn console_live_delivery_ignores_snapshot_prefix_reconstructed_independently() {
        let mut sessions = Sessions::default();
        sessions.replay_cutoffs.insert("r".into(), 20);
        let mut event = Event {
            seq: 10,
            session: "r".into(),
            run: Some("old".into()),
            msg_type: "stream".into(),
            ts_ms: 0,
            content: json!({}),
        };
        assert!(!sessions.accepts_replay(&event));
        event.run = Some("busy".into());
        assert!(!sessions.accepts_replay(&event));
        event.run = Some("new".into());
        event.seq = 21;
        assert!(sessions.accepts_replay(&event));
    }

    #[test]
    fn console_starts_survive_switches_and_reject_stale_completions() {
        let mut sessions = Sessions::default();
        sessions.starts.insert("r".into(), 1);
        sessions.states.insert("r".into(), Session::Starting);
        sessions.starts.insert("python".into(), 2);
        sessions.states.insert("python".into(), Session::Starting);
        assert!(sessions.complete_start("r", 1, Session::Ready("retained-r".into())));
        assert!(matches!(sessions.states["python"], Session::Starting));
        assert!(sessions.complete_start("python", 2, Session::Ready("retained-python".into())));
        assert_eq!(sessions.states["r"].id(), Some("retained-r"));
        assert!(!sessions.complete_start("r", 1, Session::Ready("stale".into())));
        assert_eq!(sessions.states["r"].id(), Some("retained-r"));
    }

    #[test]
    fn console_live_selection_during_start_preserves_explicit_selection() {
        let mut sessions = Sessions::default();
        sessions.starts.insert("r".into(), 1);
        sessions
            .states
            .insert("r".into(), Session::Ready("chosen-live".into()));
        assert!(!sessions.complete_start("r", 1, Session::Ready("later-start".into())));
        assert_eq!(sessions.states["r"].id(), Some("chosen-live"));
        assert!(!sessions.starts.contains_key("r"));
    }

    #[test]
    fn console_history_walk_recalls_walks_and_stops_after_edit() {
        let history = ["c", "b\nmore", "a"];
        // Empty input: Up recalls the newest, Down does nothing.
        assert_eq!(history_step(&history, None, "", true), Some(Some(0)));
        assert_eq!(history_step(&history, None, "", false), None);
        assert_eq!(history_step(&[], None, "", true), None);
        // Unedited recalled text (multi-line too) walks older and stops at the oldest.
        assert_eq!(
            history_step(&history, Some((0, "c")), "c", true),
            Some(Some(1))
        );
        assert_eq!(
            history_step(&history, Some((1, "b\nmore")), "b\nmore", true),
            Some(Some(2))
        );
        assert_eq!(
            history_step(&history, Some((2, "a")), "a", true),
            Some(Some(2))
        );
        assert_eq!(
            history_step(&history, Some((1, "b\nmore")), "b\nmore", false),
            Some(Some(0))
        );
        // Down past the newest empties the input.
        assert_eq!(
            history_step(&history, Some((0, "c")), "c", false),
            Some(None)
        );
        // Any edit stops the walk; typed text without a recall never walks.
        assert_eq!(history_step(&history, Some((0, "c")), "cx", true), None);
        assert_eq!(history_step(&history, Some((0, "c")), "cx", false), None);
        assert_eq!(history_step(&history, None, "c", true), None);
        // Emptied again: Up restarts from the newest.
        assert_eq!(
            history_step(&history, Some((2, "a")), "", true),
            Some(Some(0))
        );
    }

    #[test]
    fn console_history_walk_skips_consecutive_duplicates() {
        let mut commands = Commands::default();
        for (n, code) in ["a", "b", "b", "a"].into_iter().enumerate() {
            commands.push(n.to_string(), code.into(), n as i64);
        }
        assert_eq!(history_codes(&commands), ["a", "b", "a"]);
    }

    #[test]
    fn console_history_is_bounded_newest_first_and_keeps_duplicate_commands() {
        let mut commands = Commands::default();
        for n in 0..105 {
            commands.push(n.to_string(), "print(1)".into(), n);
        }
        assert_eq!(commands.0.len(), 100);
        assert_eq!(commands.0.first().unwrap().0, "5");
        let shown: Vec<_> = commands.newest().map(|(id, _, _)| id.as_str()).collect();
        assert_eq!(shown.len(), 50);
        assert_eq!(shown[0], "104");
        assert_eq!(shown[49], "55");
        commands.push("104".into(), "duplicate event".into(), 104);
        assert_eq!(commands.0.len(), 100);
        assert_eq!(commands.newest().next().unwrap().1, "print(1)");
    }

    #[test]
    fn console_history_merges_snapshot_before_live_commands_without_duplicates() {
        let mut commands = Commands::default();
        commands.push("new".into(), "new code".into(), 3);
        commands.merge_snapshot(
            [
                ("old".into(), "old code".into()),
                ("new".into(), "new code".into()),
            ],
            2,
        );
        assert_eq!(
            commands.0,
            vec![
                ("old".into(), "old code".into(), 2),
                ("new".into(), "new code".into(), 2)
            ]
        );
        commands.push("blank".into(), " \n".into(), 4);
        assert_eq!(commands.0.len(), 2);
    }

    #[test]
    fn console_reopened_history_replaces_stale_prefix_and_keeps_only_newer_live_commands() {
        let mut commands = Commands::default();
        commands.push("obsolete".into(), "old command".into(), 1);
        commands.push("retained".into(), "retained command".into(), 2);
        commands.push("live".into(), "concurrent live command".into(), 21);
        commands.merge_snapshot(
            [
                ("retained".into(), "retained command".into()),
                (
                    "external".into(),
                    "command issued while document was closed".into(),
                ),
            ],
            20,
        );
        assert_eq!(
            commands
                .newest()
                .map(|(id, _, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["live", "external", "retained"]
        );
        assert!(!commands.0.iter().any(|(id, _, _)| id == "obsolete"));
    }
}
