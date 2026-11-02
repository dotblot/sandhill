//! The Git pane, as the Tauri app has it (`GitPane.tsx`): the project's
//! branch with fetch, pull and push; for the working tree, staged and
//! unstaged files and a commit box whose message an agent can write. A
//! file opens its diff as a tab. Git runs on the project's machine, through
//! the daemon.

use std::time::Duration;

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
use crate::workbench::{TopTab, Workbench};

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
    pub(crate) rewrite: Option<crate::git_rewrite::Rewrite>,
    // History, read in `git_log.rs`.
    pub(crate) log: crate::git_log::Log,
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
        self.show_right && self.top_tab == TopTab::Git
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

    /// Answers the big-file question: ignore it (adds it to .gitignore),
    /// stage it anyway, or cancel.
    fn answer_big(&mut self, choice: &str, cx: &mut Context<Self>) {
        let Some(GitAsk::Big(file, paths, _all)) = self.git.ask.take() else {
            return;
        };
        match choice {
            "stage" => self.git_act(
                GitAction::Stage { files: paths },
                "stage",
                "Could not stage",
                None,
                cx,
            ),
            "ignore" => {
                let (Some(daemon), repo) = (self.daemon(), self.git_repo()) else {
                    return;
                };
                let gitignore = format!("{repo}/.gitignore");
                let line = format!("/{}", file.path);
                let mut rest: Vec<String> = paths.into_iter().filter(|p| *p != file.path).collect();
                if !rest.contains(&".gitignore".to_string()) {
                    rest.push(".gitignore".into());
                }
                let read = daemon.call::<FileText>(Call::ReadFile {
                    path: gitignore.clone(),
                });
                cx.spawn(async move |this, cx| {
                    let text = read.await.map(|f| f.text).unwrap_or_default();
                    let wrote = if text.lines().any(|l| l == line) {
                        Ok(())
                    } else {
                        let joined = format!(
                            "{text}{}{line}\n",
                            if text.is_empty() || text.ends_with('\n') {
                                ""
                            } else {
                                "\n"
                            }
                        );
                        daemon
                            .call::<serde_json::Value>(Call::WriteFile {
                                path: gitignore,
                                text: joined,
                                expected_mtime_ms: None,
                                base64: false,
                                create: true,
                            })
                            .await
                            .map(|_| ())
                    };
                    this.update(cx, |this, cx| match wrote {
                        Ok(()) => this.git_act(
                            GitAction::Stage { files: rest },
                            "stage",
                            "Could not stage",
                            None,
                            cx,
                        ),
                        Err(err) => {
                            this.git.note = Some((
                                format!("Could not add {} to .gitignore: {err:#}", file.path),
                                true,
                            ));
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
            _ => cx.notify(),
        }
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

    fn needs_user(&self) -> bool {
        self.git.status.as_ref().is_some_and(|s| {
            !s.repo.is_empty() && (s.user_name.is_empty() || s.user_email.is_empty())
        })
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

    /// The answer to a question, by the button pressed.
    fn answer_ask(&mut self, choice: &'static str, cx: &mut Context<Self>) {
        let Some(ask) = self.git.ask.clone() else {
            return;
        };
        if choice == "cancel" {
            self.git.ask = None;
            cx.notify();
            return;
        }
        if let GitAsk::Big(..) = ask {
            return self.answer_big(choice, cx);
        }
        self.git.ask = None;
        let into = self
            .git
            .status
            .as_ref()
            .and_then(|s| s.branch.clone())
            .unwrap_or_else(|| "HEAD".into());
        match ask {
            GitAsk::Discard(files, _) => self.git_act(
                GitAction::Discard { files },
                "discard",
                "Could not throw the changes away",
                None,
                cx,
            ),
            GitAsk::Undo(..) => self.git_act(
                GitAction::UndoCommit,
                "undo_commit",
                "Could not undo the commit",
                None,
                cx,
            ),
            GitAsk::Merge(branch) => {
                let b = branch.clone();
                self.git_act(
                    GitAction::Merge {
                        branch: branch.clone(),
                    },
                    "merge",
                    format!("Could not merge {branch}"),
                    Some(Box::new(move |out| {
                        if out.contains("CONFLICT") {
                            None
                        } else if out.to_lowercase().contains("up to date") {
                            Some(format!("{into} already has everything in {b}"))
                        } else {
                            Some(format!("Merged {b} into {into}"))
                        }
                    })),
                    cx,
                )
            }
            GitAsk::Rebase(branch) => {
                let b = branch.clone();
                self.git_act(
                    GitAction::Rebase {
                        branch: branch.clone(),
                    },
                    "rebase",
                    format!("Could not rebase onto {branch}"),
                    Some(Box::new(move |out| {
                        let lower = out.to_lowercase();
                        if out.contains("CONFLICT") || lower.contains("could not apply") {
                            None
                        } else if lower.contains("up to date") {
                            Some(format!("{into} is already on top of {b}"))
                        } else {
                            Some(format!("Rebased {into} onto {b}"))
                        }
                    })),
                    cx,
                )
            }
            GitAsk::Tag(name) => {
                if choice == "push" {
                    let n = name.clone();
                    self.git_act(
                        GitAction::PushTag { name: name.clone() },
                        "push_tag",
                        format!("Could not push {name}"),
                        Some(Box::new(move |_| Some(format!("Pushed {n}")))),
                        cx,
                    )
                } else {
                    self.git_act(
                        GitAction::DeleteTag { name: name.clone() },
                        "delete_tag",
                        format!("Could not delete {name}"),
                        None,
                        cx,
                    )
                }
            }
            GitAsk::Big(..) => {}
        }
    }

    pub(crate) fn ask_git_text(
        &mut self,
        prompt: GitPrompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.git.menu = None;
        let secret = matches!(prompt, GitPrompt::Credential { secret: true, .. });
        let input = if secret {
            self.git.secret_input.clone()
        } else {
            self.git.prompt_input.clone()
        };
        let placeholder = match &prompt {
            GitPrompt::NewBranch => "Branch name, like feature/qc",
            GitPrompt::NewTag => "Tag, like v1.0",
            GitPrompt::Stash => "A note for the stash (optional)",
            GitPrompt::Credential { .. } => "",
        };
        input.update(cx, |i, cx| {
            i.set_value("", window, cx);
            i.set_placeholder(placeholder, window, cx);
            i.focus(window, cx);
        });
        self.git.prompt = Some(prompt);
        cx.notify();
    }

    /// The text asked for, typed (`ok`) or cancelled.
    fn answer_prompt(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(prompt) = self.git.prompt.take() else {
            return;
        };
        let secret = matches!(prompt, GitPrompt::Credential { secret: true, .. });
        let input = if secret {
            &self.git.secret_input
        } else {
            &self.git.prompt_input
        };
        let text = input.read(cx).value().to_string();
        input.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
        if let GitPrompt::Credential { id, .. } = prompt {
            if let Some(daemon) = self.daemon() {
                daemon.fire(Call::Answer {
                    id,
                    result: ok.then(|| serde_json::Value::String(text)),
                    error: (!ok).then(|| "cancelled".to_string()),
                });
            }
            return;
        }
        if !ok {
            return;
        }
        match prompt {
            GitPrompt::NewBranch => match branch_name(&text) {
                Some(name) => self.git_act(
                    GitAction::CreateBranch { name },
                    "create_branch",
                    "Could not make the branch",
                    None,
                    cx,
                ),
                None => {
                    self.git.note = Some((
                        format!(
                            "“{text}” can't be a branch name: use letters, numbers, - _ / and no spaces."
                        ),
                        true,
                    ))
                }
            },
            GitPrompt::NewTag => {
                let name = text.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let n = name.clone();
                self.git_act(
                    GitAction::CreateTag { name, sha: None },
                    "create_tag",
                    "Could not make the tag",
                    Some(Box::new(move |_| {
                        Some(format!("Tagged the current commit {n}"))
                    })),
                    cx,
                )
            }
            GitPrompt::Stash => {
                let message = Some(text.trim().to_string()).filter(|m| !m.is_empty());
                self.git_act(
                    GitAction::Stash { message },
                    "stash",
                    "Could not stash",
                    None,
                    cx,
                )
            }
            GitPrompt::Credential { .. } => {}
        }
    }

    /// Git or ssh asks this window for a password while fetching, pulling
    /// or pushing.
    pub(crate) fn on_git_credential(
        &mut self,
        id: u64,
        params: &serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = params["prompt"]
            .as_str()
            .unwrap_or("Password")
            .trim_end()
            .trim_end_matches(':')
            .to_string();
        let secret = params["secret"].as_bool().unwrap_or(false);
        self.show_right = true;
        self.top_tab = TopTab::Git;
        self.ask_git_text(GitPrompt::Credential { id, prompt, secret }, window, cx);
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

    pub(crate) fn toggle_git_menu(&mut self, menu: GitMenu, cx: &mut Context<Self>) {
        self.git.menu = if self.git.menu == Some(menu) {
            None
        } else {
            Some(menu)
        };
        cx.notify();
    }

    /// The Git tab's body.
    pub(crate) fn render_git(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let note =
            |text: String, color: Hsla| div().px(px(14.)).py(px(6.)).text_color(color).child(text);
        let Some(status) = &self.git.status else {
            return v_flex()
                .size_full()
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
        let mut column: Vec<AnyElement> = vec![self.render_git_head(status, cx)];
        if let Some(ask) = self.render_git_ask(cx) {
            column.push(ask);
        }
        if let Some(prompt) = self.render_git_prompt(cx) {
            column.push(prompt);
        }
        if let Some((text, failed)) = &self.git.note {
            column.push(
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
        column.push(self.render_git_log(cx));
        if self.git.log.picked == git::WORKTREE {
            column.push(self.render_git_stage(status, cx));
        } else {
            column.push(self.render_commit_detail(cx));
        }
        v_flex()
            .size_full()
            .relative()
            .children(column)
            .children(self.render_git_menu(cx))
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
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(icon("git.branch").size(px(14.)).text_color(p.muted))
            .child(
                div()
                    .id("git-branch")
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

    fn render_git_ask(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let ask = self.git.ask.as_ref()?;
        let into = self
            .git
            .status
            .as_ref()
            .and_then(|s| s.branch.clone())
            .unwrap_or_else(|| "HEAD".into());
        let has_remote = self
            .git
            .status
            .as_ref()
            .is_some_and(|s| !s.remotes.is_empty());
        let (title, detail, buttons): (String, String, Vec<(&'static str, &'static str, bool)>) =
            match ask {
                GitAsk::Discard(files, fresh) => {
                    let name = if files.len() == 1 {
                        split_path(&files[0]).0
                    } else {
                        format!("{} files", files.len())
                    };
                    if *fresh {
                        (
                            format!("Delete {name}?"),
                            "It isn't in Git, so it can't be brought back.".into(),
                            vec![("delete", "Delete", true)],
                        )
                    } else {
                        (
                            format!("Throw away the changes to {name}?"),
                            "Staged changes stay. This can't be undone.".into(),
                            vec![("discard", "Throw away", true)],
                        )
                    }
                }
                GitAsk::Undo(subject, pushed) => (
                    if subject.is_empty() {
                        "Undo the last commit?".into()
                    } else {
                        format!("Undo “{subject}”?")
                    },
                    if *pushed {
                        "It's already pushed; undoing it here doesn't take it back there. Its changes stay staged.".into()
                    } else {
                        "Its changes stay staged, so you can commit them again.".into()
                    },
                    vec![("undo", "Undo", true)],
                ),
                GitAsk::Big(file, ..) => (
                    format!(
                        "Stage {}? It is {}.",
                        split_path(&file.path).0,
                        file_size(file.size.unwrap_or(0))
                    ),
                    format!(
                        "{} Data this big usually stays out of Git: Ignore it adds the file to .gitignore, so Git leaves it out.",
                        too_big(file.size.unwrap_or(0))
                    ),
                    vec![
                        ("ignore", "Ignore it", true),
                        ("stage", "Stage anyway", false),
                    ],
                ),
                GitAsk::Merge(branch) => (
                    format!("Merge {branch} into {into}?"),
                    format!(
                        "Its commits join {into}. Where both changed the same lines, the files are marked ! to settle: edit them, or keep Mine or take Theirs, then commit."
                    ),
                    vec![("merge", "Merge", true)],
                ),
                GitAsk::Rebase(branch) => (
                    format!("Rebase {into} onto {branch}?"),
                    format!(
                        "{into}'s own commits are made again on top of {branch}, for a straight history. Where both changed the same lines, it stops with the files marked ! to settle, stage, then Continue."
                    ),
                    vec![("rebase", "Rebase", true)],
                ),
                GitAsk::Tag(name) => (
                    format!("Tag {name}"),
                    if has_remote {
                        "Push sends it to the remote; Delete takes it off here only.".into()
                    } else {
                        String::new()
                    },
                    if has_remote {
                        vec![("push", "Push", true), ("delete", "Delete", false)]
                    } else {
                        vec![("delete", "Delete", false)]
                    },
                ),
            };
        let mut row = h_flex().gap(px(6.)).flex_wrap();
        for (choice, label, primary) in buttons {
            row = row.child(
                Button::new(SharedString::from(format!("git-ask-{choice}")))
                    .xsmall()
                    .label(label)
                    .when(primary, |b| b.primary())
                    .on_click(cx.listener(move |this, _, _, cx| this.answer_ask(choice, cx))),
            );
        }
        row = row.child(
            Button::new("git-ask-cancel")
                .xsmall()
                .label("Cancel")
                .on_click(cx.listener(|this, _, _, cx| this.answer_ask("cancel", cx))),
        );
        Some(
            v_flex()
                .flex_none()
                .gap(px(4.))
                .mx(px(8.))
                .my(px(6.))
                .p(px(10.))
                .rounded(px(RADIUS))
                .bg(p.accent_soft)
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .when(!detail.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.muted)
                            .child(detail),
                    )
                })
                .child(row)
                .into_any_element(),
        )
    }

    fn render_git_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let prompt = self.git.prompt.as_ref()?;
        let (title, detail, ok) = match prompt {
            GitPrompt::NewBranch => (
                "New branch".to_string(),
                Some("Made from the current commit, and switched to."),
                "Create",
            ),
            GitPrompt::NewTag => (
                "New tag".into(),
                Some("Marks the current commit: a release like v1.0, or the version a paper used."),
                "Create",
            ),
            GitPrompt::Stash => (
                "Stash changes".into(),
                Some("Puts every change aside, new files too, and leaves the working tree clean."),
                "Stash",
            ),
            GitPrompt::Credential { prompt, secret, .. } => (
                prompt.clone(),
                if *secret {
                    credential_note(prompt)
                } else {
                    None
                },
                "Sign in",
            ),
        };
        let secret = matches!(prompt, GitPrompt::Credential { secret: true, .. });
        let input = if secret {
            &self.git.secret_input
        } else {
            &self.git.prompt_input
        };
        Some(
            v_flex()
                .flex_none()
                .gap(px(4.))
                .mx(px(8.))
                .my(px(6.))
                .p(px(10.))
                .rounded(px(RADIUS))
                .bg(p.accent_soft)
                .child(div().font_weight(FontWeight::MEDIUM).child(title))
                .when_some(detail, |d, text| {
                    d.child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.muted)
                            .child(text),
                    )
                })
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(div().flex_1().child(Input::new(input).small()))
                        .child(
                            Button::new("git-prompt-ok")
                                .xsmall()
                                .primary()
                                .label(ok)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer_prompt(true, window, cx)
                                })),
                        )
                        .child(
                            Button::new("git-prompt-cancel")
                                .xsmall()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.answer_prompt(false, window, cx)
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_git_stage(&self, status: &GitStatus, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let staged: Vec<GitFile> = staged_files(status).into_iter().cloned().collect();
        let unstaged: Vec<GitFile> = unstaged_files(status).into_iter().cloned().collect();
        let busy = self.git.busy.is_some();
        let resolving = matches!(status.state.as_deref(), Some("merging") | Some("rebasing"));
        let mut files: Vec<AnyElement> = Vec::new();
        if !staged.is_empty() {
            files.push(self.file_group("Staged", p.ok, &staged, true, false, cx));
        }
        if !unstaged.is_empty() {
            files.push(self.file_group("Unstaged", p.busy, &unstaged, false, resolving, cx));
        }
        if status.more {
            files.push(
                div()
                    .px(px(14.))
                    .py(px(4.))
                    .text_color(p.muted)
                    .child("Only the first 2,000 changed files show.")
                    .into_any_element(),
            );
        }
        if staged.is_empty() && unstaged.is_empty() {
            files.push(
                div()
                    .px(px(14.))
                    .py(px(6.))
                    .text_color(p.muted)
                    .child("No changes since the last commit.")
                    .into_any_element(),
            );
        }
        let state = status.state.clone().map(|state| {
            let merging = state == "merging";
            let word = match state.as_str() {
                "merging" => "Merging",
                "rebasing" => "Rebasing",
                "cherry-picking" => "Cherry-picking",
                _ => "Reverting",
            };
            let conflicts = unstaged.iter().any(|f| f.conflict);
            h_flex()
                .flex_none()
                .gap(px(6.))
                .mx(px(8.))
                .mt(px(6.))
                .px(px(10.))
                .py(px(6.))
                .rounded(px(RADIUS))
                .bg(p.busy_soft)
                .text_size(px(look::SMALL))
                .child(div().flex_1().min_w_0().child(format!(
                    "{word}: fix each ! file in the editor and stage it, or pick Mine or Theirs; then {}.",
                    if merging { "commit" } else { "Continue" }
                )))
                .when(!merging, |d| {
                    d.child(
                        Button::new("git-continue")
                            .xsmall()
                            .primary()
                            .label("Continue")
                            .disabled(busy || conflicts)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git_act(GitAction::Continue, "continue", "Could not go on", None, cx)
                            })),
                    )
                })
                .child(
                    Button::new("git-abort")
                        .xsmall()
                        .label("Stop")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.git_act(GitAction::Abort, "abort", "Could not stop it", None, cx)
                        })),
                )
        });
        let needs_user = self.needs_user().then(|| {
            v_flex()
                .flex_none()
                .gap(px(4.))
                .px(px(10.))
                .py(px(6.))
                .border_t(px(0.5))
                .border_color(p.glass_edge)
                .text_size(px(look::SMALL))
                .child(
                    div()
                        .text_color(p.muted)
                        .child("Commits need your name and email:"),
                )
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git.user_name).small()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(Input::new(&self.git.user_email).small()),
                        )
                        .child(
                            Button::new("git-user-save")
                                .xsmall()
                                .primary()
                                .label("Save")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let name =
                                        this.git.user_name.read(cx).value().trim().to_string();
                                    let email =
                                        this.git.user_email.read(cx).value().trim().to_string();
                                    if name.is_empty() || !email.contains('@') {
                                        return;
                                    }
                                    this.git_act(
                                        GitAction::SetUser { name, email },
                                        "set_user",
                                        "Could not save your name and email",
                                        None,
                                        cx,
                                    )
                                })),
                        ),
                )
        });
        let can_commit =
            self.git.amend || !staged.is_empty() || status.state.as_deref() == Some("merging");
        let typed = !self.git.summary.read(cx).value().trim().is_empty();
        let committing = self.git.busy == Some("commit");
        let amend = self.git.amend;
        let commit_box = v_flex()
            .flex_none()
            .gap(px(6.))
            .px(px(8.))
            .py(px(8.))
            .border_t(px(0.5))
            .border_color(p.glass_edge)
            .child(Input::new(&self.git.summary).small())
            .child(Input::new(&self.git.body).small())
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        Button::new("git-write")
                            .xsmall()
                            .label(if self.git.writing { "Writing…" } else { "Write it" })
                            .disabled(self.git.writing || staged.is_empty())
                            .tooltip(if staged.is_empty() {
                                "Stage changes first"
                            } else {
                                "Ask an agent (Claude Code, or Codex) to write the message from the staged changes"
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.write_message(window, cx))),
                    )
                    .child(
                        Button::new("git-amend")
                            .xsmall()
                            .label("Amend")
                            .selected(amend)
                            .disabled(status.head.is_none())
                            .tooltip("Change the last commit instead of making a new one")
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_amend(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("git-commit")
                            .xsmall()
                            .primary()
                            .label(match (committing, amend) {
                                (true, _) => "Committing…",
                                (false, true) => "Amend",
                                (false, false) => "Commit",
                            })
                            .disabled(!typed || !can_commit || busy || self.needs_user())
                            .on_click(cx.listener(|this, _, window, cx| this.commit(None, window, cx))),
                    )
                    .child(
                        icon_button("git-commit-more", "chevron.down", &p, false)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_git_menu(GitMenu::Commit, cx))),
                    ),
            );
        v_flex()
            .flex_1()
            .min_h(px(140.))
            .children(state)
            .child(
                div()
                    .id("git-files")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(px(4.))
                    .children(files),
            )
            .children(needs_user)
            .child(commit_box)
            .into_any_element()
    }

    fn file_group(
        &self,
        title: &'static str,
        color: Hsla,
        files: &[GitFile],
        staged: bool,
        resolving: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let busy = self.git.busy.is_some();
        let all_files: Vec<GitFile> = files.to_vec();
        let head = h_flex()
            .px(px(12.))
            .pt(px(6.))
            .pb(px(2.))
            .gap(px(6.))
            .text_size(px(look::SMALL))
            .child(look::dot(color))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(p.muted)
                    .child(title),
            )
            .child(div().text_color(p.faint).child(files.len().to_string()))
            .child(div().flex_1())
            .child(
                icon_button(
                    SharedString::from(format!("git-all-{title}")),
                    if staged { "close" } else { "add" },
                    &p,
                    false,
                )
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if staged {
                        "Unstage all"
                    } else {
                        "Stage all"
                    })
                    .build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.git.busy.is_some() {
                        return;
                    }
                    if staged {
                        this.git_act(
                            GitAction::Unstage { files: Vec::new() },
                            "unstage",
                            "Could not unstage",
                            None,
                            cx,
                        )
                    } else {
                        this.stage(all_files.clone(), true, cx)
                    }
                })),
            );
        let hover = p.ink(7.);
        let rows = files.iter().enumerate().map(|(ix, file)| {
            let (letter, kind, meaning) = file_mark(file, staged);
            let (name, dir) = split_path(&file.path);
            let lines = if staged {
                file.staged_lines.as_ref()
            } else {
                file.lines.as_ref()
            };
            let big = file.size.unwrap_or(0) >= BIG_FILE
                && (!staged || file.staged.as_deref() == Some("A"));
            let mark_color = match kind {
                MarkKind::Added => p.ok,
                MarkKind::Deleted | MarkKind::Conflict => p.err,
                MarkKind::Renamed => p.accent,
                MarkKind::Modified => p.busy,
            };
            let (diff_path, open_path, toggle, discard, ours, theirs) = (
                file.path.clone(),
                file.path.clone(),
                file.clone(),
                file.clone(),
                file.path.clone(),
                file.path.clone(),
            );
            let tip = format!("{}\n{meaning}", file.path);
            let fresh = file.unstaged.as_deref() == Some("?");
            let tag = if staged { "s" } else { "u" };
            let side_button = |id: String,
                               label: &'static str,
                               side: sandhill_proto::GitSide,
                               path: String,
                               cx: &mut Context<Self>| {
                Button::new(SharedString::from(id))
                    .xsmall()
                    .label(label)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.git_act(
                            GitAction::Resolve {
                                file: path.clone(),
                                side,
                            },
                            "resolve",
                            format!("Could not settle {path}"),
                            None,
                            cx,
                        )
                    }))
            };
            h_flex()
                .id(SharedString::from(format!("git-file-{tag}-{ix}")))
                .mx(px(4.))
                .px(px(8.))
                .h(px(24.))
                .gap(px(6.))
                .rounded(px(RADIUS))
                .whitespace_nowrap()
                .overflow_hidden()
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_diff(diff_path.clone(), staged, None, cx)
                }))
                .child(
                    div()
                        .w(px(12.))
                        .flex_none()
                        .text_size(px(look::SMALL))
                        .font_weight(FontWeight::BOLD)
                        .text_color(mark_color)
                        .child(letter),
                )
                .child(
                    picture(&if file.path.ends_with('/') {
                        look::folder_icon(
                            file.path
                                .trim_end_matches('/')
                                .rsplit('/')
                                .next()
                                .unwrap_or(""),
                        )
                    } else {
                        look::file_icon(&file.path)
                    })
                    .size(px(14.)),
                )
                .child(div().flex_none().text_color(p.text).child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child(dir),
                )
                .when(big, |d| {
                    d.child(
                        div()
                            .text_size(px(look::SMALL))
                            .text_color(p.err)
                            .child(file_size(file.size.unwrap_or(0))),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child(line_counts(lines)),
                )
                .when(!file.path.ends_with('/') && letter != "D", |d| {
                    d.child(
                        icon_button(
                            SharedString::from(format!("git-open-{tag}-{ix}")),
                            "files",
                            &p,
                            false,
                        )
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Open the file")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.open_repo_file(&open_path, cx)
                        })),
                    )
                })
                .when(resolving && file.conflict, |d| {
                    d.child(side_button(
                        format!("git-ours-{ix}"),
                        "Mine",
                        sandhill_proto::GitSide::Ours,
                        ours,
                        cx,
                    ))
                    .child(side_button(
                        format!("git-theirs-{ix}"),
                        "Theirs",
                        sandhill_proto::GitSide::Theirs,
                        theirs,
                        cx,
                    ))
                })
                .when(!staged && !file.conflict, |d| {
                    d.child(
                        icon_button(
                            SharedString::from(format!("git-discard-{ix}")),
                            "delete",
                            &p,
                            false,
                        )
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(if fresh {
                                "Delete"
                            } else {
                                "Throw away the changes"
                            })
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.git.ask = Some(GitAsk::Discard(vec![discard.path.clone()], fresh));
                            cx.notify();
                        })),
                    )
                })
                .child(
                    icon_button(
                        SharedString::from(format!("git-toggle-{tag}-{ix}")),
                        if staged { "close" } else { "add" },
                        &p,
                        false,
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if staged {
                            "Unstage"
                        } else {
                            "Stage"
                        })
                        .build(window, cx)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if this.git.busy.is_some() {
                            return;
                        }
                        if staged {
                            this.git_act(
                                GitAction::Unstage {
                                    files: vec![toggle.path.clone()],
                                },
                                "unstage",
                                "Could not unstage",
                                None,
                                cx,
                            )
                        } else {
                            this.stage(vec![toggle.clone()], false, cx)
                        }
                    })),
                )
                .into_any_element()
        });
        v_flex().child(head).children(rows).into_any_element()
    }

    fn render_git_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let menu = self.git.menu?;
        let status = self.git.status.clone()?;
        let mut rows: Vec<AnyElement> = Vec::new();
        let heading = |text: String| {
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(2.))
                .text_size(px(look::SMALL - 1.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.faint)
                .child(text)
                .into_any_element()
        };
        let sep = || {
            div()
                .my(px(3.))
                .h(px(0.5))
                .bg(p.glass_edge)
                .into_any_element()
        };
        let mut n = 0usize;
        let mut item = |icon_name: &'static str,
                        label: String,
                        enabled: bool,
                        cx: &mut Context<Self>,
                        run: MenuRun| {
            n += 1;
            let hover = p.ink(8.);
            h_flex()
                .id(("git-menu-item", n))
                .gap(px(8.))
                .px(px(8.))
                .h(px(26.))
                .rounded(px(RADIUS - 4.))
                .whitespace_nowrap()
                .when(!enabled, |d| d.opacity(0.4))
                .when(enabled, |d| d.cursor_pointer().hover(move |d| d.bg(hover)))
                .child(icon(icon_name).size(px(14.)).text_color(p.muted))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if enabled {
                        this.git.menu = None;
                        run(this, window, cx);
                        cx.notify();
                    }
                }))
                .into_any_element()
        };
        let branch = status.branch.clone();
        let into = branch.clone().unwrap_or_else(|| "HEAD".into());
        let unstaged: Vec<GitFile> = unstaged_files(&status).into_iter().cloned().collect();
        let staged_any = !staged_files(&status).is_empty();
        let busy_state = status.state.is_some();
        match menu {
            GitMenu::Branches => {
                rows.push(heading("Branches".into()));
                for b in &status.branches {
                    let current = Some(b) == branch.as_ref();
                    let name = b.clone();
                    rows.push(item(
                        if current { "check" } else { "git.branch" },
                        b.clone(),
                        !current,
                        cx,
                        Box::new(move |this, _, cx| {
                            this.git_act(
                                GitAction::Switch {
                                    branch: name.clone(),
                                },
                                "switch",
                                format!("Could not switch to {name}"),
                                None,
                                cx,
                            )
                        }),
                    ));
                }
                let remote = git::remote_only(&status);
                if !remote.is_empty() {
                    rows.push(sep());
                    rows.push(heading("On the remote".into()));
                    for b in remote {
                        let name = b.clone();
                        rows.push(item(
                            "globe",
                            b,
                            true,
                            cx,
                            Box::new(move |this, _, cx| {
                                this.git_act(
                                    GitAction::Switch {
                                        branch: name.clone(),
                                    },
                                    "switch",
                                    format!("Could not switch to {name}"),
                                    None,
                                    cx,
                                )
                            }),
                        ));
                    }
                }
                rows.push(sep());
                rows.push(item(
                    "add",
                    "New branch…".into(),
                    true,
                    cx,
                    Box::new(|this, window, cx| {
                        this.ask_git_text(GitPrompt::NewBranch, window, cx)
                    }),
                ));
                let can_merge =
                    branch.is_some() && !git::mergeable(&status).is_empty() && !busy_state;
                rows.push(item(
                    "git.branch",
                    format!("Merge into {into}…"),
                    can_merge,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git.menu = Some(GitMenu::Merge);
                        cx.notify();
                    }),
                ));
                rows.push(item(
                    "git.branch",
                    format!("Rebase {into} onto…"),
                    can_merge,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git.menu = Some(GitMenu::Rebase);
                        cx.notify();
                    }),
                ));
                rows.push(sep());
                rows.push(heading("Tags".into()));
                for t in status.tags.iter().take(12) {
                    let name = t.clone();
                    rows.push(item(
                        "git.tag",
                        t.clone(),
                        true,
                        cx,
                        Box::new(move |this, _, cx| {
                            this.git.ask = Some(GitAsk::Tag(name.clone()));
                            cx.notify();
                        }),
                    ));
                }
                rows.push(item(
                    "add",
                    "New tag…".into(),
                    status.head.is_some(),
                    cx,
                    Box::new(|this, window, cx| this.ask_git_text(GitPrompt::NewTag, window, cx)),
                ));
            }
            GitMenu::Merge | GitMenu::Rebase => {
                let merge = menu == GitMenu::Merge;
                rows.push(heading(if merge {
                    format!("Merge into {into}")
                } else {
                    format!("Rebase {into} onto")
                }));
                for b in git::mergeable(&status) {
                    let remote = status.remote_branches.contains(&b);
                    let name = b.clone();
                    rows.push(item(
                        if remote { "globe" } else { "git.branch" },
                        b,
                        true,
                        cx,
                        Box::new(move |this, _, cx| {
                            this.git.ask = Some(if merge {
                                GitAsk::Merge(name.clone())
                            } else {
                                GitAsk::Rebase(name.clone())
                            });
                            cx.notify();
                        }),
                    ));
                }
            }
            GitMenu::Stash => {
                rows.push(item(
                    "git.stash",
                    "Stash changes…".into(),
                    !status.files.is_empty(),
                    cx,
                    Box::new(|this, window, cx| this.ask_git_text(GitPrompt::Stash, window, cx)),
                ));
                rows.push(item(
                    "edit.undo",
                    if status.stashes > 0 {
                        format!("Bring back the last stash ({} kept)", status.stashes)
                    } else {
                        "Bring back the last stash".into()
                    },
                    status.stashes > 0,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git_act(
                            GitAction::StashPop,
                            "stash_pop",
                            "Could not bring the stash back",
                            None,
                            cx,
                        )
                    }),
                ));
            }
            GitMenu::More => {
                rows.push(item(
                    "git.pull",
                    "Pull, with my commits on top".into(),
                    status.upstream.is_some() && !busy_state,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git_act(
                            GitAction::Pull { rebase: true },
                            "pull",
                            "Could not pull",
                            Some(Box::new(last_line)),
                            cx,
                        )
                    }),
                ));
                rows.push(item(
                    "sort",
                    "Squash or reorder commits…".into(),
                    status.head.is_some() && branch.is_some() && !busy_state,
                    cx,
                    Box::new(|this, _, cx| this.open_rewrite(cx)),
                ));
                rows.push(sep());
                let to_stage = unstaged.clone();
                rows.push(item(
                    "add",
                    "Stage all".into(),
                    !unstaged.is_empty(),
                    cx,
                    Box::new(move |this, _, cx| this.stage(to_stage.clone(), true, cx)),
                ));
                rows.push(item(
                    "close",
                    "Unstage all".into(),
                    staged_any,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git_act(
                            GitAction::Unstage { files: Vec::new() },
                            "unstage",
                            "Could not unstage",
                            None,
                            cx,
                        )
                    }),
                ));
                let to_discard: Vec<String> = unstaged
                    .iter()
                    .filter(|f| !f.conflict)
                    .map(|f| f.path.clone())
                    .collect();
                let fresh = unstaged.iter().all(|f| f.unstaged.as_deref() == Some("?"));
                rows.push(item(
                    "delete",
                    "Throw away all unstaged changes…".into(),
                    !to_discard.is_empty(),
                    cx,
                    Box::new(move |this, _, cx| {
                        this.git.ask = Some(GitAsk::Discard(to_discard.clone(), fresh));
                        cx.notify();
                    }),
                ));
                rows.push(sep());
                rows.push(item(
                    "refresh",
                    "Read again".into(),
                    true,
                    cx,
                    Box::new(|this, _, cx| {
                        this.git.log.refs = None;
                        this.read_git_status(cx)
                    }),
                ));
            }
            GitMenu::Commit => {
                let typed = !self.git.summary.read(cx).value().trim().is_empty();
                let can =
                    self.git.amend || staged_any || status.state.as_deref() == Some("merging");
                rows.push(item(
                    "git.push",
                    if self.git.amend {
                        "Amend and push".into()
                    } else {
                        "Commit and push".into()
                    },
                    typed && can,
                    cx,
                    Box::new(|this, window, cx| this.commit(Some("push"), window, cx)),
                ));
                rows.push(item(
                    "add",
                    "Stage all and commit".into(),
                    typed && (staged_any || !unstaged.is_empty()),
                    cx,
                    Box::new(|this, window, cx| this.commit(Some("all"), window, cx)),
                ));
            }
        }
        let bottom = menu == GitMenu::Commit;
        let left = matches!(menu, GitMenu::Branches | GitMenu::Merge | GitMenu::Rebase);
        let body = crate::composer_view::popup(&p)
            .id("git-menu")
            .occlude()
            .min_w(px(220.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.git.menu = None;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .when(left, |d| d.left(px(8.)))
                .when(!left, |d| d.right(px(8.)))
                .when(bottom, |d| d.bottom(px(44.)))
                .when(!bottom, |d| d.top(px(34.)))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }
}
