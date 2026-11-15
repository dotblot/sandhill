//! The Git pane, as the Tauri app has it (`GitPane.tsx`): the project's
//! branch with fetch, pull and push; for the working tree, staged and
//! unstaged files and a commit box whose message an agent can write. A
//! file opens its diff as a tab. Git runs on the project's machine, through
//! the daemon.

use std::time::Duration;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Selectable};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, FileText, GitAction, GitCommitInfo, GitDone, GitFile, GitMessage, GitStatus,
};

use crate::git::{
    self, BIG_FILE, MarkKind, branch_name, commit_message, file_mark, file_size, line_counts,
    split_message, split_path, staged_files, too_big, unstaged_files,
};
use crate::look::{self, Palette, RADIUS, icon, icon_button, picture};
use crate::workbench::{BottomTab, Workbench};

mod asks;
mod menu;
mod stage;

/// How often the status is read while the pane shows.
const POLL: Duration = Duration::from_secs(3);

/// The pane's menus.
/// Turns git's output into the note shown after an action.
type DoneNote = Box<dyn Fn(&str) -> Option<String>>;
/// What a Git menu row does when picked.
type MenuRun = Box<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GitMenu {
    Branches,
    Merge,
    Rebase,
    Stash,
    More,
    Commit,
}

/// A question the pane asks before doing something that can't be undone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GitAsk {
    Discard(Vec<String>, bool),
    Undo(String, bool),
    /// A file too big for GitHub, before staging it: the files to stage
    /// after, and whether it was Stage all.
    Big(GitFile, Vec<String>, bool),
    Merge(String),
    Rebase(String),
    Tag(String),
}

/// A line of text the pane asks for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GitPrompt {
    NewBranch,
    NewTag,
    Stash,
    /// Git or ssh wants a password, a token or a passphrase.
    Credential {
        id: u64,
        prompt: String,
        secret: bool,
    },
}

pub(crate) struct Git {
    pub(crate) status: Option<GitStatus>,
    error: Option<String>,
    /// What the last action said, and whether it failed.
    note: Option<(String, bool)>,
    pub(crate) busy: Option<&'static str>,
    reading: bool,
    polling: bool,
    generation: u64,
    writing: bool,
    pub(crate) summary: Entity<InputState>,
    pub(crate) body: Entity<InputState>,
    amend: bool,
    pub(crate) menu: Option<GitMenu>,
    pub(crate) ask: Option<GitAsk>,
    pub(crate) prompt: Option<GitPrompt>,
    pub(crate) prompt_input: Entity<InputState>,
    secret_input: Entity<InputState>,
    user_name: Entity<InputState>,
    user_email: Entity<InputState>,
    /// The merge message put in the commit boxes.
    merge_filled: Option<String>,
    /// The Squash and reorder dialog, when open.
    pub(crate) rewrite: Option<crate::git::rewrite::Rewrite>,
    // History, read in `git/log.rs`.
    pub(crate) log: crate::git::log::Log,
}

pub(crate) fn new_git(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Git, Vec<Subscription>) {
    let summary = cx.new(|cx| InputState::new(window, cx).placeholder("Summary"));
    let body = cx.new(|cx| InputState::new(window, cx).placeholder("Why this change…"));
    let prompt_input = cx.new(|cx| InputState::new(window, cx));
    let secret_input = cx.new(|cx| InputState::new(window, cx).masked(true));
    let user_name = cx.new(|cx| InputState::new(window, cx).placeholder("Name"));
    let user_email = cx.new(|cx| InputState::new(window, cx).placeholder("Email"));
    let subscriptions = vec![
        cx.subscribe_in(
            &summary,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { secondary, .. } = event {
                    if *secondary {
                        this.commit(None, window, cx);
                    } else {
                        this.git
                            .body
                            .update(cx, |input, cx| input.focus(window, cx));
                    }
                }
            },
        ),
        cx.subscribe_in(&body, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.commit(None, window, cx);
            }
        }),
        cx.subscribe_in(
            &prompt_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.answer_prompt(true, window, cx);
                }
            },
        ),
        cx.subscribe_in(
            &secret_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.answer_prompt(true, window, cx);
                }
            },
        ),
    ];
    let git = Git {
        status: None,
        error: None,
        note: None,
        busy: None,
        reading: false,
        polling: false,
        generation: 0,
        writing: false,
        summary,
        body,
        amend: false,
        menu: None,
        ask: None,
        prompt: None,
        prompt_input,
        secret_input,
        user_name,
        user_email,
        merge_filled: None,
        rewrite: None,
        log: Default::default(),
    };
    (git, subscriptions)
}

impl Git {
    pub(crate) fn set_note(&mut self, text: String, failed: bool) {
        self.note = Some((text, failed));
    }
}

/// What a password prompt needs saying.
fn credential_note(prompt: &str) -> Option<&'static str> {
    let lower = prompt.to_lowercase();
    if lower.contains("passphrase") {
        Some("The passphrase of your ssh key, which stays on this machine.")
    } else if ["github", "gitlab", "bitbucket"]
        .iter()
        .any(|s| lower.contains(s))
    {
        Some("Not your account password: a personal access token, made in the site's settings.")
    } else {
        None
    }
}

fn last_line(output: &str) -> Option<String> {
    output
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(str::to_string)
}

impl Workbench {
    pub(crate) fn git_shown(&self) -> bool {
        self.show_files && self.bottom_tab == BottomTab::Git
    }

    /// Reads the status when the pane first shows, and keeps reading it
    /// while it shows. Called while drawing.
    pub(crate) fn sync_git(&mut self, cx: &mut Context<Self>) {
        let wanted = self.git_shown() || self.diff_wants_reading();
        if wanted && !self.git.polling && self.project.is_some() {
            self.git.polling = true;
            self.read_git_status(cx);
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(POLL).await;
                    let go_on = this
                        .update(cx, |this, cx| {
                            let go_on = (this.git_shown() || this.diff_wants_reading())
                                && this.project.is_some();
                            if !go_on {
                                this.git.polling = false;
                                return false;
                            }
                            if this.git_shown() && !this.git.reading && this.git.busy.is_none() {
                                this.read_git_status(cx);
                            }
                            this.read_diff(cx);
                            true
                        })
                        .unwrap_or(false);
                    if !go_on {
                        break;
                    }
                }
            })
            .detach();
        }
    }

    /// The project changed: its Git state goes.
    pub(crate) fn forget_git(&mut self) {
        self.git.status = None;
        self.git.error = None;
        self.git.note = None;
        self.git.ask = None;
        self.git.menu = None;
        self.git.generation += 1;
        self.git.log = Default::default();
        self.diff = None;
        self.diff_shown = false;
    }

    pub(crate) fn read_git_status(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        // A repository's own configuration can run programs.
        if self.restricted() {
            return;
        }
        self.git.generation += 1;
        let mine = self.git.generation;
        self.git.reading = true;
        let read = daemon.call::<GitStatus>(Call::GitStatus { path: project });
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let read = read.await;
            cx.update_window(handle, |_, window, cx| {
                this.update(cx, |this, cx| this.apply_git_status(read, mine, window, cx))
            })
            .ok();
        })
        .detach();
    }

    fn apply_git_status(
        &mut self,
        read: anyhow::Result<GitStatus>,
        mine: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.git.generation != mine {
            return;
        }
        self.git.reading = false;
        match read {
            Ok(next) => {
                let merge = next.merge_message.clone();
                let refs = next.refs.clone();
                let repo = !next.repo.is_empty();
                // Polling mostly reads the same again; only a change redraws.
                if self.git.status.as_ref() != Some(&next) {
                    self.git.status = Some(next);
                    cx.notify();
                }
                self.fill_merge_message(merge, window, cx);
                if self.git.error.take().is_some() {
                    cx.notify();
                }
                if repo && self.git.log.refs.as_deref() != Some(refs.as_str()) {
                    self.read_git_log(Some(refs), false, cx);
                }
                if !repo {
                    self.git.log.commits.clear();
                }
            }
            Err(err) => {
                self.git.error = Some(format!("{err:#}"));
                cx.notify();
            }
        }
    }

    /// Runs a git action; reads the status again after, whatever happened.
    /// `done` turns what git said into a note, or none.
    pub(crate) fn git_act(
        &mut self,
        action: GitAction,
        op: &'static str,
        failed: impl Into<String>,
        done: Option<DoneNote>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let failed = failed.into();
        self.git.busy = Some(op);
        self.git.note = None;
        let ran = daemon.call::<GitDone>(Call::GitRun {
            path: project,
            action,
        });
        cx.spawn(async move |this, cx| {
            let ran = ran.await;
            this.update(cx, |this, cx| {
                this.git.busy = None;
                match ran {
                    Ok(result) => {
                        if let Some(said) = done.and_then(|done| done(result.output.trim())) {
                            this.git.note = Some((said, false));
                        }
                    }
                    Err(err) => this.git.note = Some((format!("{failed}: {err:#}"), true)),
                }
                this.read_git_status(cx);
                this.read_diff(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn git_repo(&self) -> String {
        self.git
            .status
            .as_ref()
            .map(|s| s.repo.clone())
            .unwrap_or_default()
    }

    /// Stages files, asking first about one too big for GitHub.
    fn stage(&mut self, files: Vec<GitFile>, all: bool, cx: &mut Context<Self>) {
        if let Some(big) = files.iter().find(|f| f.size.unwrap_or(0) >= BIG_FILE) {
            let rest: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
            self.git.ask = Some(GitAsk::Big(big.clone(), rest, all));
            cx.notify();
            return;
        }
        let paths = if all {
            Vec::new()
        } else {
            files.iter().map(|f| f.path.clone()).collect()
        };
        self.git_act(
            GitAction::Stage { files: paths },
            "stage",
            "Could not stage",
            None,
            cx,
        );
    }

    fn commit(&mut self, then: Option<&'static str>, window: &mut Window, cx: &mut Context<Self>) {
        let summary = self.git.summary.read(cx).value().to_string();
        let body = self.git.body.read(cx).value().to_string();
        let text = commit_message(&summary, &body);
        let Some(status) = self.git.status.clone() else {
            return;
        };
        if text.is_empty() || self.needs_user() {
            return;
        }
        let amend = self.git.amend;
        let staged = !staged_files(&status).is_empty();
        let unstaged = !unstaged_files(&status).is_empty();
        let merging = status.state.as_deref() == Some("merging");
        let all = then == Some("all");
        if !(amend || staged || merging || (all && unstaged)) {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.git.busy = Some("commit");
        self.git.note = None;
        let stage = all.then(|| {
            daemon.call_in_order::<GitDone>(Call::GitRun {
                path: project.clone(),
                action: GitAction::Stage { files: Vec::new() },
            })
        });
        let committed = daemon.call_in_order::<GitDone>(Call::GitRun {
            path: project.clone(),
            action: GitAction::Commit {
                message: text,
                amend,
            },
        });
        let push = then == Some("push");
        cx.spawn_in(window, async move |this, cx| {
            if let Some(stage) = stage {
                let _ = stage.await;
            }
            let committed = committed.await;
            this.update_in(cx, |this, window, cx| {
                this.git.busy = None;
                match committed {
                    Ok(_) => {
                        this.git
                            .summary
                            .update(cx, |i, cx| i.set_value("", window, cx));
                        this.git
                            .body
                            .update(cx, |i, cx| i.set_value("", window, cx));
                        this.git.amend = false;
                        if push {
                            this.push(cx);
                        }
                    }
                    Err(err) => {
                        let what = if amend {
                            "Could not change the last commit"
                        } else {
                            "Could not commit"
                        };
                        this.git.note = Some((format!("{what}: {err:#}"), true));
                    }
                }
                this.read_git_status(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn push(&mut self, cx: &mut Context<Self>) {
        let status = self.git.status.clone().unwrap_or_default();
        let said = format!(
            "Pushed {}{}",
            status.branch.as_deref().unwrap_or("the branch"),
            status
                .upstream
                .as_deref()
                .and_then(|u| u.split('/').next())
                .map(|r| format!(" to {r}"))
                .unwrap_or_default()
        );
        self.git_act(
            GitAction::Push,
            "push",
            "Could not push",
            Some(Box::new(move |_| Some(said.clone()))),
            cx,
        );
    }

    fn toggle_amend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git.amend = !self.git.amend;
        cx.notify();
        let empty = self.git.summary.read(cx).value().trim().is_empty();
        let (Some(daemon), Some(project), Some(head)) = (
            self.daemon(),
            self.project.clone(),
            self.git.status.as_ref().and_then(|s| s.head.clone()),
        ) else {
            return;
        };
        if !self.git.amend || !empty {
            return;
        }
        let last = daemon.call::<GitCommitInfo>(Call::GitShow {
            path: project,
            sha: head,
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(last) = last.await {
                let (summary, body) = split_message(&last.message);
                this.update_in(cx, |this, window, cx| {
                    this.git
                        .summary
                        .update(cx, |i, cx| i.set_value(summary, window, cx));
                    this.git
                        .body
                        .update(cx, |i, cx| i.set_value(body, window, cx));
                })
                .ok();
            }
        })
        .detach();
    }

    /// Asks an agent (Claude Code, or Codex) for the message.
    fn write_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        self.git.writing = true;
        let written = daemon.call::<GitMessage>(Call::GitWriteMessage {
            path: project,
            agent: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let written = written.await;
            this.update_in(cx, |this, window, cx| {
                this.git.writing = false;
                match written {
                    Ok(m) => {
                        this.git
                            .summary
                            .update(cx, |i, cx| i.set_value(m.summary, window, cx));
                        this.git
                            .body
                            .update(cx, |i, cx| i.set_value(m.body, window, cx));
                    }
                    Err(err) => {
                        this.git.note = Some((format!("Could not write a message: {err:#}"), true))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A merge's commit starts with git's message; it goes again with the
    /// merge.
    fn fill_merge_message(
        &mut self,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text == self.git.merge_filled {
            return;
        }
        let (summary, body) = split_message(
            text.as_deref()
                .or(self.git.merge_filled.as_deref())
                .unwrap_or(""),
        );
        let now_summary = self.git.summary.read(cx).value().to_string();
        let now_body = self.git.body.read(cx).value().to_string();
        if text.is_some() && now_summary.trim().is_empty() && now_body.trim().is_empty() {
            self.git
                .summary
                .update(cx, |i, cx| i.set_value(summary, window, cx));
            self.git
                .body
                .update(cx, |i, cx| i.set_value(body, window, cx));
        } else if text.is_none() && now_summary == summary && now_body == body {
            self.git
                .summary
                .update(cx, |i, cx| i.set_value("", window, cx));
            self.git
                .body
                .update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.git.merge_filled = text;
    }

    /// Opens a file of the repository in the editor.
    pub(crate) fn open_repo_file(&mut self, file: &str, cx: &mut Context<Self>) {
        let path = format!("{}/{file}", self.git_repo());
        match self
            .project
            .as_deref()
            .and_then(|p| path.strip_prefix(&format!("{p}/")))
        {
            Some(rel) => self.open_file(rel.to_string(), cx),
            None => {
                self.problem = Some(format!("{file} is outside the project folder."));
                cx.notify();
            }
        }
    }

    /// The buttons on the right of the Git tab's strip.
    pub(crate) fn git_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let Some(status) = self.git.status.as_ref().filter(|s| !s.repo.is_empty()) else {
            return Vec::new();
        };
        let busy = self.git.busy;
        let tool =
            |id: &'static str,
             name: &'static str,
             op: &'static str,
             tip: String,
             enabled: bool,
             count: u32,
             cx: &mut Context<Self>,
             run: fn(&mut Workbench, &mut Window, &mut Context<Workbench>)| {
                let on = enabled && busy.is_none();
                h_flex()
                    .id(id)
                    .on_prepaint(self.popup_triggers.track(id))
                    .h(px(22.))
                    .px(px(5.))
                    .gap(px(2.))
                    .rounded(px(RADIUS))
                    .text_size(px(look::SMALL))
                    .text_color(p.muted)
                    .when(!on, |d| d.opacity(0.35))
                    .when(on, |d| {
                        d.cursor_pointer()
                            .hover(|d| d.bg(p.ink(7.)).text_color(p.text))
                    })
                    .child(if busy == Some(op) {
                        div().text_color(p.busy).child("…").into_any_element()
                    } else {
                        icon(name).size(px(14.)).into_any_element()
                    })
                    .when(count > 0, |d| d.child(count.to_string()))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if on {
                            run(this, window, cx)
                        }
                    }))
                    .into_any_element()
            };
        let behind = status.behind;
        let ahead = status.ahead;
        let plural = |n: u32| if n == 1 { "" } else { "s" };
        vec![
            tool(
                "git-fetch",
                "refresh",
                "fetch",
                "Fetch: see what's new on the remote".into(),
                !status.remotes.is_empty(),
                0,
                cx,
                |this, _, cx| this.git_act(GitAction::Fetch, "fetch", "Could not fetch", None, cx),
            ),
            tool(
                "git-pull",
                "git.pull",
                "pull",
                if behind > 0 {
                    format!("Pull {behind} commit{}", plural(behind))
                } else {
                    "Pull".into()
                },
                status.upstream.is_some(),
                behind,
                cx,
                |this, _, cx| {
                    this.git_act(
                        GitAction::Pull { rebase: false },
                        "pull",
                        "Could not pull",
                        Some(Box::new(last_line)),
                        cx,
                    )
                },
            ),
            tool(
                "git-push",
                "git.push",
                "push",
                if ahead > 0 {
                    format!("Push {ahead} commit{}", plural(ahead))
                } else {
                    "Push".into()
                },
                status.head.is_some(),
                ahead,
                cx,
                |this, _, cx| this.push(cx),
            ),
            tool(
                "git-undo",
                "edit.undo",
                "undo_commit",
                "Undo the last commit".into(),
                status.head.is_some(),
                0,
                cx,
                |this, _, cx| {
                    let status = this.git.status.clone().unwrap_or_default();
                    let subject = this.git_head_subject().unwrap_or_default();
                    let pushed = status.upstream.is_some() && status.ahead == 0;
                    this.git.ask = Some(GitAsk::Undo(subject, pushed));
                    cx.notify();
                },
            ),
            tool(
                "git-stash",
                "git.stash",
                "stash",
                "Stash".into(),
                true,
                status.stashes,
                cx,
                |this, _, cx| this.toggle_git_menu(GitMenu::Stash, cx),
            ),
            tool(
                "git-more",
                "more",
                "",
                "More".into(),
                true,
                0,
                cx,
                |this, _, cx| this.toggle_git_menu(GitMenu::More, cx),
            ),
        ]
    }

    /// The Git tab's body.
    pub(crate) fn render_git(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note =
            |text: String, color: Hsla| div().px(px(14.)).py(px(6.)).text_color(color).child(text);
        let Some(status) = &self.git.status else {
            return v_flex()
                .size_full()
                .pt(px(look::STRIP))
                .child(match &self.git.error {
                    Some(err) => note(err.clone(), p.err),
                    None if self.project.is_none() => {
                        note("Open a project to see its Git state.".into(), p.muted)
                    }
                    None => note("Reading Git…".into(), p.muted),
                })
                .into_any_element();
        };
        if status.repo.is_empty() {
            return v_flex()
                .size_full()
                .gap(px(8.))
                .p(px(14.))
                .pt(px(look::STRIP + 14.))
                .child(
                    div()
                        .text_color(p.muted)
                        .child("This project isn't a Git repository yet."),
                )
                .child(
                    h_flex().child(
                        Button::new("git-init")
                            .small()
                            .primary()
                            .label("Start one here")
                            .disabled(self.git.busy.is_some())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_act(
                                    GitAction::Init,
                                    "init",
                                    "Could not start a repository",
                                    None,
                                    cx,
                                )
                            })),
                    ),
                )
                .into_any_element();
        }
        let mut notices: Vec<AnyElement> = Vec::new();
        if let Some(ask) = self.render_git_ask(cx) {
            notices.push(ask);
        }
        if let Some(prompt) = self.render_git_prompt(cx) {
            notices.push(prompt);
        }
        if let Some((text, failed)) = &self.git.note {
            notices.push(
                h_flex()
                    .flex_none()
                    .px(px(12.))
                    .py(px(4.))
                    .gap(px(6.))
                    .text_size(px(look::SMALL))
                    .border_b(px(0.5))
                    .border_color(p.glass_edge)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(if *failed { p.err } else { p.muted })
                            .child(text.clone()),
                    )
                    .child(
                        icon_button("git-note-close", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.git.note = None;
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        let detail = if self.git.log.picked == git::WORKTREE {
            self.render_git_stage(status, cx)
        } else {
            self.render_commit_detail(cx)
        };
        v_flex()
            .size_full()
            .relative()
            .child(self.render_git_log(notices, cx))
            .child(detail)
            .children(self.render_git_menu(cx))
            .child(look::floating_header(
                &p,
                look::STRIP,
                false,
                self.render_git_head(status, cx),
            ))
            .into_any_element()
    }

    fn render_git_head(&self, status: &GitStatus, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let branch = status.branch.clone().unwrap_or_else(|| {
            status
                .head
                .as_ref()
                .map(|h| format!("HEAD at {}", &h[..7.min(h.len())]))
                .unwrap_or_default()
        });
        let counts = [
            (status.ahead > 0).then(|| format!("↑{}", status.ahead)),
            (status.behind > 0).then(|| format!("↓{}", status.behind)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let tip = match &status.upstream {
            Some(u) => format!("{branch}, following {u}; switch branch"),
            None => format!("{branch}; switch branch"),
        };
        h_flex()
            .flex_none()
            .h(px(32.))
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(icon("git.branch").size(px(14.)).text_color(p.muted))
            .child(
                div()
                    .id("git-branch")
                    .on_prepaint(self.popup_triggers.track("git-branches"))
                    .px(px(6.))
                    .rounded(px(RADIUS))
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .hover(|d| d.bg(p.ink(7.)))
                    .child(branch)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(
                        cx.listener(|this, _, _, cx| this.toggle_git_menu(GitMenu::Branches, cx)),
                    ),
            )
            .when(!counts.is_empty(), |d| {
                d.child(
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.accent)
                        .child(counts),
                )
            })
            .child(div().flex_1())
            .into_any_element()
    }

    fn git_head_subject(&self) -> Option<String> {
        let head = self.git.status.as_ref()?.head.as_ref()?;
        self.git
            .log
            .commits
            .iter()
            .find(|c| &c.sha == head)
            .map(|c| c.subject.clone())
    }
}
