//! The Git pane's questions and prompts: asking, answering, and their dialogs.

use super::*;

impl Workbench {
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

    pub(super) fn needs_user(&self) -> bool {
        self.git.status.as_ref().is_some_and(|s| {
            !s.repo.is_empty() && (s.user_name.is_empty() || s.user_email.is_empty())
        })
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
    pub(super) fn answer_prompt(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.show_files = true;
        self.bottom_tab = BottomTab::Git;
        self.ask_git_text(GitPrompt::Credential { id, prompt, secret }, window, cx);
    }

    pub(super) fn render_git_ask(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    pub(super) fn render_git_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
}
