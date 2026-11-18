//! The Git pane's menus.

use super::*;

impl Workbench {
    pub(crate) fn toggle_git_menu(&mut self, menu: GitMenu, cx: &mut Context<Self>) {
        let same_family = self.git.menu == Some(menu)
            || (menu == GitMenu::Branches
                && matches!(self.git.menu, Some(GitMenu::Merge | GitMenu::Rebase)));
        self.git.menu = if same_family { None } else { Some(menu) };
        cx.notify();
    }

    pub(super) fn render_git_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let menu = self.git.menu?;
        let status = self.git.status.clone()?;
        let mut rows: Vec<AnyElement> = Vec::new();
        let heading = |text: String| {
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(2.))
                .text_size(px(look::SMALL))
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
        let body = crate::agents::composer_view::popup(&p)
            .id("git-menu")
            .occlude()
            .min_w(px(220.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                let trigger = match menu {
                    GitMenu::Branches | GitMenu::Merge | GitMenu::Rebase => "git-branches",
                    GitMenu::Stash => "git-stash",
                    GitMenu::More => "git-more",
                    GitMenu::Commit => "git-commit",
                };
                if this.popup_triggers.contains(trigger, event.position) {
                    return;
                }
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
