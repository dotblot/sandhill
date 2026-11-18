//! A file's diff in the editor area, as the Tauri app has it (`DiffView.tsx`):
//! old and new line numbers, removed lines red and added ones green. A
//! working tree diff is read again while it shows, so it follows edits, and
//! can be staged or unstaged from its bar: the whole file, a hunk from its
//! header, or the lines picked by their numbers (click, Shift-click), as
//! `git add -p` would.

use std::collections::BTreeSet;

use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, GitAction, GitDiff};

use crate::git::{DiffKind, DiffLine, diff_lines, hunk_label, split_path};
use crate::look::{self, ButtonSize::Row, Palette, bar_button, icon, pane, picture};
use crate::workbench::Workbench;

/// Lines drawn before "Show all".
const SHOWN_LINES: usize = 4000;

pub(crate) struct DiffTab {
    pub repo: String,
    pub file: String,
    pub staged: bool,
    pub sha: Option<String>,
    diff: Option<GitDiff>,
    lines: Vec<DiffLine>,
    error: Option<String>,
    /// Changed lines picked, by their places in the diff's text.
    picked: BTreeSet<usize>,
    anchor: Option<usize>,
    all: bool,
    /// Throw away the picked lines, once more to do it.
    confirm: bool,
    generation: u64,
}

impl DiffTab {
    pub fn title(&self) -> String {
        let name = split_path(&self.file).0;
        match (&self.sha, self.staged) {
            (Some(sha), _) => format!("{name} @ {}", &sha[..7.min(sha.len())]),
            (None, true) => format!("{name} (staged)"),
            (None, false) => format!("{name} (changes)"),
        }
    }

    fn pickable(&self) -> bool {
        self.sha.is_none() && self.diff.as_ref().is_some_and(|d| d.lines)
    }

    /// The changed lines of the hunk whose header is at `at`.
    fn hunk_lines(&self, at: usize) -> Vec<usize> {
        let start = self.lines.iter().position(|l| l.at == at).unwrap_or(0);
        self.lines[start + 1..]
            .iter()
            .take_while(|l| l.kind != DiffKind::Hunk)
            .filter(|l| changed(l))
            .map(|l| l.at)
            .collect()
    }
}

fn changed(line: &DiffLine) -> bool {
    matches!(line.kind, DiffKind::Add | DiffKind::Del)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinesOp {
    Stage,
    Unstage,
    Discard,
}

impl Workbench {
    /// Opens a file's diff as a tab, in place of the one open.
    pub(crate) fn open_diff(
        &mut self,
        file: String,
        staged: bool,
        sha: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.reveal_middle();
        self.hide_object_explorer();
        let repo = self
            .git
            .status
            .as_ref()
            .map(|s| s.repo.clone())
            .unwrap_or_default();
        let same = self.diff.as_ref().is_some_and(|d| {
            d.file == file && d.staged == staged && d.sha == sha && d.repo == repo
        });
        if !same {
            self.diff = Some(DiffTab {
                repo,
                file,
                staged,
                sha,
                diff: None,
                lines: Vec::new(),
                error: None,
                picked: BTreeSet::new(),
                anchor: None,
                all: false,
                confirm: false,
                generation: 0,
            });
        }
        self.diff_shown = true;
        self.explorer_shown = false;
        self.read_diff(cx);
        cx.notify();
    }

    pub(crate) fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.diff = None;
        self.diff_shown = false;
        cx.notify();
    }

    /// Whether a working tree diff shows, which is read again now and then.
    pub(crate) fn diff_wants_reading(&self) -> bool {
        self.diff_shown && self.diff.as_ref().is_some_and(|d| d.sha.is_none())
    }

    pub(crate) fn read_diff(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(tab) = self.diff.as_mut() else {
            return;
        };
        // A commit's diff never changes.
        if tab.sha.is_some() && tab.diff.is_some() {
            return;
        }
        tab.generation += 1;
        let mine = tab.generation;
        let read = daemon.call::<GitDiff>(Call::GitDiff {
            path: tab.repo.clone(),
            file: tab.file.clone(),
            staged: tab.staged,
            sha: tab.sha.clone(),
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                let Some(tab) = this.diff.as_mut().filter(|t| t.generation == mine) else {
                    return;
                };
                match read {
                    Ok(next) => {
                        if tab.diff.as_ref() != Some(&next) {
                            // Picks name places in the old text.
                            tab.picked.clear();
                            tab.confirm = false;
                            tab.lines = diff_lines(&next.text);
                            tab.diff = Some(next);
                            cx.notify();
                        }
                        if tab.error.take().is_some() {
                            cx.notify();
                        }
                    }
                    Err(err) => {
                        tab.error = Some(format!("{err:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn diff_toggle_file(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = &self.diff else { return };
        let files = vec![tab.file.clone()];
        let staged = tab.staged;
        let action = if staged {
            GitAction::Unstage { files }
        } else {
            GitAction::Stage { files }
        };
        let failed = format!(
            "Could not {} {}",
            if staged { "unstage" } else { "stage" },
            tab.file
        );
        self.git_act(
            action,
            if staged { "unstage" } else { "stage" },
            failed,
            None,
            cx,
        );
    }

    fn diff_apply_lines(&mut self, op: LinesOp, ats: Vec<usize>, cx: &mut Context<Self>) {
        let Some(tab) = self.diff.as_mut() else {
            return;
        };
        let Some(diff) = &tab.diff else { return };
        if ats.is_empty() {
            return;
        }
        if op == LinesOp::Discard && !tab.confirm {
            tab.confirm = true;
            cx.notify();
            return;
        }
        tab.confirm = false;
        let (file, text) = (tab.file.clone(), diff.text.clone());
        let mut lines = ats;
        lines.sort_unstable();
        tab.picked.clear();
        let (action, verb, name) = match op {
            LinesOp::Stage => (
                GitAction::StageLines {
                    file: file.clone(),
                    diff: text,
                    lines,
                },
                "stage",
                "stage_lines",
            ),
            LinesOp::Unstage => (
                GitAction::UnstageLines {
                    file: file.clone(),
                    diff: text,
                    lines,
                },
                "unstage",
                "unstage_lines",
            ),
            LinesOp::Discard => (
                GitAction::DiscardLines {
                    file: file.clone(),
                    diff: text,
                    lines,
                },
                "throw away",
                "discard_lines",
            ),
        };
        self.git_act(
            action,
            name,
            format!("Could not {verb} the lines of {file}"),
            None,
            cx,
        );
    }

    /// A click on a changed line's number picks or unpicks it; Shift-click
    /// picks the range from the last one clicked.
    fn diff_pick(&mut self, at: usize, shift: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.diff.as_mut() else {
            return;
        };
        tab.confirm = false;
        match (shift, tab.anchor) {
            (true, Some(anchor)) => {
                let (low, high) = if anchor < at {
                    (anchor, at)
                } else {
                    (at, anchor)
                };
                let range: Vec<usize> = tab
                    .lines
                    .iter()
                    .filter(|l| changed(l) && l.at >= low && l.at <= high)
                    .map(|l| l.at)
                    .collect();
                tab.picked.extend(range);
            }
            _ => {
                tab.anchor = Some(at);
                if !tab.picked.remove(&at) {
                    tab.picked.insert(at);
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn render_diff(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(tab) = &self.diff else {
            return div().into_any_element();
        };
        let pickable = tab.pickable();
        let picked = tab.picked.len();
        let (added, removed) = tab.lines.iter().fold((0, 0), |(a, r), l| match l.kind {
            DiffKind::Add => (a + 1, r),
            DiffKind::Del => (a, r + 1),
            _ => (a, r),
        });
        let where_ = match (&tab.sha, tab.staged) {
            (Some(sha), _) => format!("in commit {}", &sha[..7.min(sha.len())]),
            (None, true) => "staged".into(),
            (None, false) => "not staged".into(),
        };
        let words = format!("{picked} line{}", if picked == 1 { "" } else { "s" });
        let staged = tab.staged;
        let file = tab.file.clone();
        let mut bar = h_flex()
            .flex_none()
            .h_full()
            .px(px(look::GAP_8))
            .gap(px(look::GAP_4))
            .whitespace_nowrap()
            .child(picture(&look::file_icon(&tab.file)).size(px(look::ICON)))
            .child(
                div()
                    .min_w(px(40.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .font_family(look::mono())
                    .text_size(px(look::small()))
                    .child(tab.file.clone()),
            )
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.faint)
                    .child(where_),
            )
            .when(added > 0, |d| {
                d.child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.ok)
                        .child(format!("+{added}")),
                )
            })
            .when(removed > 0, |d| {
                d.child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.err)
                        .child(format!("−{removed}")),
                )
            })
            .when(pickable && picked == 0, |d| {
                d.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child("Click line numbers to pick lines"),
                )
            })
            .child(div().flex_1());
        if pickable && picked > 0 {
            let ats: Vec<usize> = tab.picked.iter().copied().collect();
            if staged {
                let ats = ats.clone();
                bar = bar.child(
                    bar_button(
                        "diff-unstage-lines",
                        format!("Unstage {words}"),
                        Some("close"),
                        &p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.diff_apply_lines(LinesOp::Unstage, ats.clone(), cx)
                    })),
                );
            } else {
                let stage = ats.clone();
                let discard = ats.clone();
                bar = bar
                    .child(
                        bar_button(
                            "diff-stage-lines",
                            format!("Stage {words}"),
                            Some("add"),
                            &p,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.diff_apply_lines(LinesOp::Stage, stage.clone(), cx)
                        })),
                    )
                    .child(
                        bar_button(
                            "diff-discard-lines",
                            if tab.confirm {
                                "Throw away? Click again"
                            } else {
                                "Throw away"
                            },
                            Some("delete"),
                            &p,
                        )
                        .when(tab.confirm, |d| d.text_color(p.err))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.diff_apply_lines(LinesOp::Discard, discard.clone(), cx)
                        })),
                    );
            }
            bar = bar.child(
                bar_button("diff-clear", "", Some("close"), &p).on_click(cx.listener(
                    |this, _, _, cx| {
                        if let Some(tab) = this.diff.as_mut() {
                            tab.picked.clear();
                            tab.confirm = false;
                        }
                        cx.notify();
                    },
                )),
            );
        }
        if tab.sha.is_none() {
            bar = bar.child(
                bar_button(
                    "diff-toggle",
                    if staged { "Unstage" } else { "Stage" },
                    Some(if staged { "close" } else { "add" }),
                    &p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.diff_toggle_file(cx))),
            );
        }
        bar = bar.child(
            bar_button("diff-open", "Open", Some("files"), &p)
                .on_click(cx.listener(move |this, _, _, cx| this.open_repo_file(&file, cx))),
        );
        let mut body: Vec<AnyElement> = Vec::new();
        let note = |text: String, color: Hsla| {
            div()
                .px(px(14.))
                .py(px(8.))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        if let Some(err) = &tab.error {
            body.push(note(err.clone(), p.err));
        }
        if tab.diff.as_ref().is_some_and(|d| d.binary) {
            body.push(note(
                "A binary file: there are no lines to compare.".into(),
                p.muted,
            ));
        } else if tab.diff.is_some() && tab.lines.is_empty() {
            body.push(note(
                if staged {
                    "Nothing of this file is staged now.".into()
                } else {
                    "No changes left in this file.".into()
                },
                p.muted,
            ));
        }
        let shown = if tab.all {
            tab.lines.len()
        } else {
            SHOWN_LINES.min(tab.lines.len())
        };
        let add_bg = p.ok.opacity(0.12);
        let del_bg = p.err.opacity(0.12);
        let pick_bg = p.accent.opacity(0.28);
        for line in &tab.lines[..shown] {
            if line.kind == DiffKind::Hunk {
                let (lines, context) = hunk_label(&line.text);
                let at = line.at;
                let hunk = tab.hunk_lines(at);
                let mut row = h_flex()
                    .h(px(24.))
                    .px(px(10.))
                    .gap(px(8.))
                    .bg(p.chunk)
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child(div().font_weight(FontWeight::MEDIUM).child(lines))
                    .child(
                        div()
                            .font_family(look::mono())
                            .text_color(p.faint)
                            .child(context),
                    );
                if pickable {
                    if staged {
                        let h = hunk.clone();
                        row = row.child(
                            look::button(("hunk-unstage", at), "Unstage", None, Row).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.diff_apply_lines(LinesOp::Unstage, h.clone(), cx)
                                }),
                            ),
                        );
                    } else {
                        let (s, d) = (hunk.clone(), hunk.clone());
                        row = row
                            .child(
                                look::button(("hunk-stage", at), "Stage", None, Row).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.diff_apply_lines(LinesOp::Stage, s.clone(), cx)
                                    }),
                                ),
                            )
                            .child(
                                look::button(("hunk-discard", at), "Throw away", None, Row)
                                    .danger()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.diff_apply_lines(LinesOp::Discard, d.clone(), cx)
                                    })),
                            );
                    }
                }
                body.push(row.into_any_element());
                continue;
            }
            let at = line.at;
            let can_pick = pickable && changed(line);
            let is_picked = tab.picked.contains(&at);
            let (bg, mark) = match line.kind {
                DiffKind::Add => (Some(add_bg), "+"),
                DiffKind::Del => (Some(del_bg), "−"),
                _ => (None, ""),
            };
            let number = |n: Option<u32>| {
                div()
                    .w(px(44.))
                    .flex_none()
                    .pr(px(6.))
                    .text_right()
                    .text_color(p.faint)
                    .child(n.map(|n| n.to_string()).unwrap_or_default())
            };
            let numbers = h_flex()
                .id(("diff-n", at))
                .flex_none()
                .h_full()
                .when(is_picked, |d| d.bg(pick_bg))
                .when(can_pick, |d| {
                    d.cursor_pointer()
                        .hover(|d| d.bg(p.accent.opacity(0.18)))
                        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                            this.diff_pick(at, ev.modifiers().shift, cx)
                        }))
                })
                .child(number(line.old))
                .child(number(line.new));
            body.push(
                h_flex()
                    .min_h(px(19.))
                    .font_family(look::mono())
                    .text_size(px(look::code()))
                    .when_some(bg, |d, bg| d.bg(bg))
                    .child(numbers)
                    .child(div().w(px(16.)).flex_none().text_color(p.faint).child(mark))
                    .child(
                        div()
                            .flex_1()
                            .whitespace_nowrap()
                            .text_color(if line.kind == DiffKind::Note {
                                p.faint
                            } else {
                                p.text
                            })
                            .child(line.text.replace('\t', "    ")),
                    )
                    .into_any_element(),
            );
        }
        if !tab.all && tab.lines.len() > SHOWN_LINES {
            body.push(
                div()
                    .id("diff-all")
                    .px(px(14.))
                    .py(px(6.))
                    .text_color(p.accent)
                    .cursor_pointer()
                    .child(format!("Show all {} lines", tab.lines.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(tab) = this.diff.as_mut() {
                            tab.all = true;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        if tab.diff.as_ref().is_some_and(|d| d.truncated) {
            body.push(note("The diff is cut short at 2 MB.".into(), p.muted));
        }
        pane(&p)
            .flex_1()
            .h_full()
            .child(
                Self::strip(&p)
                    .child(
                        h_flex()
                            .id("diff-actions")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_x_scroll()
                            .restrict_scroll_to_axis()
                            .child(bar),
                    )
                    .child(self.panel_buttons(crate::shell::pane_sizes::Panel::Editor, cx)),
            )
            .child(
                div()
                    .id("diff-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_scroll()
                    .restrict_scroll_to_axis()
                    .child(v_flex().min_w_full().children(body)),
            )
            .into_any_element()
    }

    /// The diff's document tab in its project capsule.
    pub(crate) fn diff_tab_element(
        &self,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let tab = self.diff.as_ref()?;
        let picked = expanded && self.diff_shown;
        let lead = icon("git").size(px(14.)).into_any_element();
        let (lead, trailing) = if expanded {
            crate::look::place_close(
                lead,
                crate::look::tab_close("diff-close", picked, &p).on_click(cx.listener(
                    |this, _, _, cx| {
                        cx.stop_propagation();
                        this.close_diff(cx)
                    },
                )),
                picked,
            )
        } else {
            (lead, None)
        };
        Some(
            Self::tab(
                "diff-tab",
                Some(lead),
                if expanded { tab.title() } else { String::new() },
                picked,
                &p,
            )
            .flex_none()
            .when(!expanded, |d| d.px(px(5.)).gap_0())
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new("Git diff").build(w, cx))
            .when_some(trailing, |d, close| d.pr(px(3.)).child(close))
            .on_click(cx.listener(|this, _, _, cx| {
                this.reveal_middle();
                this.hide_object_explorer();
                this.diff_shown = true;
                this.explorer_shown = false;
                this.read_diff(cx);
                cx.notify();
            }))
            .into_any_element(),
        )
    }
}
