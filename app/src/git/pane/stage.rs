//! The Git pane's staging area: the changed files and the commit box.

use super::*;

impl Workbench {
    pub(super) fn render_git_stage(
        &self,
        status: &GitStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::GAP_4))
                    .text_color(p.muted)
                    .child("Only the first 2,000 changed files show.")
                    .into_any_element(),
            );
        }
        if staged.is_empty() && unstaged.is_empty() {
            files.push(
                div()
                    .px(px(look::PANE_PAD_X))
                    .py(px(look::PANE_PAD_Y))
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
                .gap(px(look::GAP_8))
                .mx(px(look::GAP_4))
                .mt(px(look::PANE_PAD_Y))
                .px(px(look::GAP_8))
                .py(px(look::PANE_PAD_Y))
                .rounded(px(look::RADIUS))
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
                .gap(px(look::GAP_4))
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
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
                        .gap(px(look::GAP_8))
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
                                .small()
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
            .gap(px(look::GAP_8))
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .border_t(px(0.5))
            .border_color(p.glass_edge)
            .child(Input::new(&self.git.summary).small())
            .child(Input::new(&self.git.body).small())
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
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
                            .on_prepaint(self.popup_triggers.track("git-commit"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_git_menu(GitMenu::Commit, cx))),
                    ),
            );
        v_flex()
            .flex_1()
            .min_h(px(140.))
            .children(state)
            .child(look::PaneScroll::new(
                "git-files-scroll",
                0.,
                look::GAP_4,
                div()
                    .id("git-files")
                    .pb(px(look::PANE_PAD_Y))
                    .children(files),
            ))
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
            .px(px(look::PANE_PAD_X))
            .pt(px(look::GAP_6))
            .pb(px(look::GAP_2))
            .gap(px(look::GAP_4))
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
        let hover = p.ink(look::ROW_HOVER);
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
                .mx(px(look::GAP_4))
                .px(px(look::GAP_8))
                .h(px(look::ROW))
                .gap(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
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
                        .w(px(look::ICON_SMALL))
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
                    .size(px(look::ICON)),
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
}
