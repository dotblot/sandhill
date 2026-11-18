//! Git's Squash and reorder dialog, as the Tauri app's `RewriteDialog.tsx`
//! has it: the branch's own commits (those not pushed yet), newest at the
//! top as in the graph, to drag into another order or fold into the commit
//! below. A commit others fold into gets their messages under it, to edit.
//! The daemon makes the commits again (`GitAction::Rewrite`, an interactive
//! rebase).

use std::collections::{HashMap, HashSet};

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, GitAction, GitOwnCommits, GitRewrite};

use crate::look::{self, Palette, icon_button};
use crate::workbench::Workbench;

/// The dialog's state.
pub(crate) struct Rewrite {
    own: GitOwnCommits,
    /// Newest first, as shown.
    order: Vec<String>,
    squashed: HashSet<String>,
    /// The message boxes of commits others fold into, by commit, with the
    /// text last put in them, so one not typed in follows the folding.
    messages: HashMap<String, (Entity<TextareaState>, String)>,
}

/// A commit being dragged to another place.
#[derive(Clone)]
struct Dragged {
    sha: String,
    subject: String,
}

impl Render for Dragged {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        div()
            .px(px(look::GAP_8))
            .py(px(look::GAP_4))
            .rounded(px(look::CONTROL_RADIUS))
            .bg(p.bg)
            .border_1()
            .border_color(p.accent)
            .text_size(px(look::CHROME))
            .child(self.subject.clone())
    }
}

/// Each commit kept, oldest first, with those folded into it: a commit
/// marked squashed folds into the one below it, except the bottom one.
pub(crate) fn groups(order: &[String], squashed: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut list: Vec<(String, Vec<String>)> = Vec::new();
    for (i, sha) in order.iter().rev().enumerate() {
        let at = order.len() - 1 - i;
        let folds = squashed.contains(sha) && at + 1 < order.len();
        match list.last_mut() {
            Some((_, folded)) if folds => folded.push(sha.clone()),
            _ => list.push((sha.clone(), Vec::new())),
        }
    }
    list
}

/// What the daemon is asked for: the commits in their new order, oldest
/// first, each folded into the one before or with its new message.
pub(crate) fn plan(
    groups: &[(String, Vec<String>)],
    message: impl Fn(&str) -> String,
) -> Vec<GitRewrite> {
    groups
        .iter()
        .flat_map(|(sha, folded)| {
            let head = GitRewrite {
                sha: sha.clone(),
                squash: false,
                message: (!folded.is_empty()).then(|| message(sha)),
            };
            std::iter::once(head).chain(folded.iter().map(|sha| GitRewrite {
                sha: sha.clone(),
                squash: true,
                message: None,
            }))
        })
        .collect()
}

/// The button's words: "Reorder", or what squashing leaves.
pub(crate) fn action_label(kept: usize, all: usize) -> String {
    if kept == all {
        "Reorder".into()
    } else {
        format!(
            "Squash into {kept} {}",
            if kept == 1 { "commit" } else { "commits" }
        )
    }
}

impl Rewrite {
    fn message_of(&self, sha: &str) -> &str {
        self.own
            .commits
            .iter()
            .find(|c| c.sha == sha)
            .map(|c| c.message.as_str())
            .unwrap_or_default()
    }

    fn subject(&self, sha: &str) -> String {
        self.message_of(sha)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The messages of a commit and those folded into it, one after another.
    fn joined(&self, sha: &str, folded: &[String]) -> String {
        std::iter::once(sha)
            .chain(folded.iter().map(String::as_str))
            .map(|s| self.message_of(s).trim())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn changed(&self) -> bool {
        let start: Vec<&String> = self.own.commits.iter().rev().map(|c| &c.sha).collect();
        self.order.iter().zip(&start).any(|(a, b)| a != *b)
            || groups(&self.order, &self.squashed)
                .iter()
                .any(|(_, f)| !f.is_empty())
    }
}

impl Workbench {
    /// Opens Squash and reorder with the branch's own commits, if there
    /// are two to change.
    pub(crate) fn open_rewrite(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let found = daemon.call::<GitOwnCommits>(Call::GitOwnCommits { path: project });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                match found {
                    Err(err) => this
                        .git
                        .set_note(format!("Could not read the commits: {err:#}"), true),
                    Ok(found) if found.commits.len() < 2 => {
                        let note = found.note.clone().unwrap_or_default();
                        let said = if found.commits.is_empty() {
                            if note.is_empty() {
                                "There are no commits to change.".to_string()
                            } else {
                                note
                            }
                        } else {
                            format!("Only the last commit isn't pushed yet. {note}")
                                .trim()
                                .to_string()
                        };
                        this.git.set_note(said, false);
                    }
                    Ok(found) => {
                        let order = found.commits.iter().rev().map(|c| c.sha.clone()).collect();
                        this.git.rewrite = Some(Rewrite {
                            own: found,
                            order,
                            squashed: HashSet::new(),
                            messages: HashMap::new(),
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Puts message boxes under the commits others fold into, filled with
    /// the messages joined, unless one was typed in.
    fn refresh_rewrite_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rw) = self.git.rewrite.as_mut() else {
            return;
        };
        let groups = groups(&rw.order, &rw.squashed);
        for (sha, folded) in &groups {
            if folded.is_empty() {
                continue;
            }
            let joined = rw.joined(sha, folded);
            match rw.messages.get_mut(sha) {
                Some((input, put)) => {
                    if input.read(cx).value() == put.as_str() && *put != joined {
                        input.update(cx, |i, cx| i.set_value(joined.clone(), window, cx));
                        *put = joined;
                    }
                }
                None => {
                    let text = joined.clone();
                    let input = cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .auto_grow(3, 10)
                            .default_value(text)
                    });
                    rw.messages.insert(sha.clone(), (input, joined));
                }
            }
        }
        cx.notify();
    }

    fn move_commit(&mut self, sha: &str, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rw) = self.git.rewrite.as_mut() else {
            return;
        };
        rw.order.retain(|s| s != sha);
        let to = to.min(rw.order.len());
        rw.order.insert(to, sha.to_string());
        self.refresh_rewrite_messages(window, cx);
    }

    fn toggle_squash(&mut self, sha: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rw) = self.git.rewrite.as_mut() else {
            return;
        };
        if !rw.squashed.remove(sha) {
            rw.squashed.insert(sha.to_string());
        }
        self.refresh_rewrite_messages(window, cx);
    }

    fn finish_rewrite(&mut self, cx: &mut Context<Self>) {
        let Some(rw) = self.git.rewrite.take() else {
            return;
        };
        let groups = groups(&rw.order, &rw.squashed);
        let commits = plan(&groups, |sha| {
            rw.messages
                .get(sha)
                .map(|(input, _)| input.read(cx).value().to_string())
                .unwrap_or_default()
        });
        let left = groups.len();
        let all = commits.len();
        self.git_act(
            GitAction::Rewrite {
                base: rw.own.base.clone(),
                commits,
            },
            "rewrite",
            "Could not change the commits",
            Some(Box::new(move |output: &str| {
                let lower = output.to_lowercase();
                if lower.contains("conflict") || lower.contains("could not apply") {
                    None
                } else if left < all {
                    Some(format!(
                        "Squashed into {left} {}",
                        if left == 1 { "commit" } else { "commits" }
                    ))
                } else {
                    Some("Reordered the commits".into())
                }
            })),
            cx,
        );
        cx.notify();
    }

    /// The dialog over the window, when open.
    pub(crate) fn render_rewrite(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rw = self.git.rewrite.as_ref()?;
        let p = *Palette::get(cx);
        let groups = groups(&rw.order, &rw.squashed);
        let count = rw.order.len();
        let empty_message = groups.iter().any(|(sha, folded)| {
            !folded.is_empty()
                && rw
                    .messages
                    .get(sha)
                    .is_none_or(|(i, _)| i.read(cx).value().trim().is_empty())
        });
        let can = rw.changed() && !empty_message;
        let rows = rw.order.iter().enumerate().map(|(i, sha)| {
            let folds = rw.squashed.contains(sha) && i + 1 < count;
            let subject = rw.subject(sha);
            let dragged = Dragged {
                sha: sha.clone(),
                subject: subject.clone(),
            };
            let (up, down, flip, drop_at) = (sha.clone(), sha.clone(), sha.clone(), i);
            let message = groups
                .iter()
                .find(|(g, f)| g == sha && !f.is_empty())
                .and_then(|(g, _)| rw.messages.get(g))
                .map(|(input, _)| input.clone());
            let hover = p.ink(look::ROW_HOVER);
            v_flex()
                .id(SharedString::from(format!("rw-{sha}")))
                .px(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .when(folds, |d| d.pl(px(22.)).text_color(p.muted))
                .on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<Dragged>(move |d, _, _, _| d.bg(hover))
                .on_drop(cx.listener(move |this, d: &Dragged, window, cx| {
                    this.move_commit(&d.sha, drop_at, window, cx)
                }))
                .child(
                    h_flex()
                        .h(px(look::ROW))
                        .gap(px(look::GAP_8))
                        .child(
                            div()
                                .flex_none()
                                .text_color(p.faint)
                                .cursor(CursorStyle::OpenHand)
                                .child("⋮⋮"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(subject),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family(look::MONO)
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(sha.chars().take(7).collect::<String>()),
                        )
                        .child(
                            icon_button(
                                SharedString::from(format!("rw-up-{sha}")),
                                "arrow.up",
                                &p,
                                false,
                            )
                            .when(i == 0, |d| d.opacity(0.35))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if i > 0 {
                                        this.move_commit(&up, i - 1, window, cx)
                                    }
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                SharedString::from(format!("rw-down-{sha}")),
                                "arrow.down",
                                &p,
                                false,
                            )
                            .when(i + 1 == count, |d| d.opacity(0.35))
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    if i + 1 < count {
                                        this.move_commit(&down, i + 1, window, cx)
                                    }
                                },
                            )),
                        )
                        .child(
                            h_flex()
                                .id(SharedString::from(format!("rw-squash-{sha}")))
                                .flex_none()
                                .h(px(look::CONTROL_ROW))
                                .gap(px(look::GAP_4))
                                .px(px(look::GAP_4))
                                .rounded(px(look::CONTROL_RADIUS))
                                .text_color(p.muted)
                                .when(i + 1 == count, |d| d.opacity(0.35))
                                .when(i + 1 < count, |d| {
                                    d.cursor_pointer()
                                        .hover(|d| d.bg(p.ink(look::BUTTON_HOVER)))
                                })
                                .child(
                                    div()
                                        .size(px(look::ICON))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(look::CONTROL_RADIUS))
                                        .border_1()
                                        .border_color(if folds { p.accent } else { p.line })
                                        .when(folds, |d| {
                                            d.bg(p.accent).text_color(p.primary_text).child(
                                                look::icon("check").size(px(look::ICON_SMALL)),
                                            )
                                        }),
                                )
                                .child("Squash")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if i + 1 < count {
                                        this.toggle_squash(&flip, window, cx)
                                    }
                                })),
                        ),
                )
                .when_some(message, |d, input| {
                    d.child(
                        div()
                            .pt(px(look::GAP_4))
                            .pb(px(look::GAP_6))
                            .font_family(look::MONO)
                            .child(Textarea::new(&input)),
                    )
                })
        });
        let card = v_flex()
            .id("rewrite-card")
            .w(px(580.))
            .max_w(relative(0.9))
            .p(px(16.))
            .gap(px(look::GAP_12))
            .bg(p.bg)
            .border_1()
            .border_color(p.glass_edge)
            .rounded(px(look::RADIUS))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Squash and reorder commits"),
            )
            .child(
                div().text_color(p.muted).child(format!(
                    "Your commits that aren't pushed yet, newest at the top. Drag one to move it, or squash it into the commit below. Changes not committed are put aside and brought back.{}",
                    rw.own
                        .note
                        .as_ref()
                        .map(|n| format!(" {n}"))
                        .unwrap_or_default()
                )),
            )
            .child(
                v_flex()
                    .id("rewrite-list")
                    .max_h(px(380.))
                    .overflow_y_scroll()
                    .p(px(look::GAP_4))
                    .border_1()
                    .border_color(p.line)
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.chunk)
                    .children(rows),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .child(
                        Button::new("rewrite-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.git.rewrite = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("rewrite-go")
                            .small()
                            .primary()
                            .disabled(!can)
                            .label(action_label(groups.len(), count))
                            .on_click(cx.listener(|this, _, _, cx| this.finish_rewrite(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("rewrite-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{action_label, groups, plan};
    use std::collections::HashSet;

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn squashes_into_the_commit_below() {
        // Newest first: d on top, a at the bottom.
        let order = s(&["d", "c", "b", "a"]);
        let squashed: HashSet<String> = s(&["c", "b", "a"]).into_iter().collect();
        // a is at the bottom and can't fold; b and c fold into a.
        let g = groups(&order, &squashed);
        assert_eq!(
            g,
            vec![
                ("a".to_string(), s(&["b", "c"])),
                ("d".to_string(), Vec::new())
            ]
        );
        let commits = plan(&g, |sha| format!("msg {sha}"));
        let shown: Vec<(String, bool, Option<String>)> = commits
            .into_iter()
            .map(|c| (c.sha, c.squash, c.message))
            .collect();
        assert_eq!(
            shown,
            vec![
                ("a".into(), false, Some("msg a".into())),
                ("b".into(), true, None),
                ("c".into(), true, None),
                ("d".into(), false, None),
            ]
        );
        assert_eq!(action_label(2, 4), "Squash into 2 commits");
        assert_eq!(action_label(1, 2), "Squash into 1 commit");
        assert_eq!(action_label(3, 3), "Reorder");
    }
}
