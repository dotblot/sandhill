//! The Git pane's history, as the Tauri app has it: the commit graph of
//! every branch, newest first, with the working tree on top and author
//! avatars (people, Claude Code, Codex); a picked commit shows its message
//! and the files it changed, each opening its diff.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, GitCommit, GitCommitInfo, GitLog};

use crate::git::{
    self, MarkKind, WORKTREE, avatar, badges, coauthors_of, file_mark, layout_graph, line_counts,
    split_path, staged_files, time_ago, unstaged_files,
};
use crate::look::{self, Palette, icon, icon_button, picture};
use crate::workbench::Workbench;

/// Commits read at a time.
const PAGE: u32 = 200;
fn detail_head_height() -> f32 {
    look::control_row() + 2. * look::PANE_PAD_Y
}
/// The graph's lane spacing.
const LANE: f32 = 16.;
const MAX_LANES: usize = 8;
/// The Branch / Tag column.
const REFS_WIDTH: f32 = 84.;

pub(crate) struct Log {
    pub commits: Vec<GitCommit>,
    pub more: bool,
    /// The refs the log was read for.
    pub refs: Option<String>,
    pub picked: String,
    pub info: Option<GitCommitInfo>,
    generation: u64,
    /// How tall the history is; the files below get the rest.
    pub height: f32,
}

impl Default for Log {
    fn default() -> Self {
        Log {
            commits: Vec::new(),
            more: false,
            refs: None,
            picked: WORKTREE.into(),
            info: None,
            generation: 0,
            height: 200.,
        }
    }
}

fn lane_color(p: &Palette, lane: usize) -> Hsla {
    p.chart[lane % p.chart.len()]
}

/// Choose Kit's neutral foreground with the best contrast across the gradient.
fn avatar_text(p: &Palette, colors: (Hsla, Hsla)) -> Hsla {
    let luminance = |color: Hsla| {
        let c = color.to_rgb();
        let linear = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
    };
    let backgrounds = [luminance(colors.0), luminance(colors.1)];
    let contrast = |text| {
        let text = luminance(text);
        backgrounds
            .into_iter()
            .map(|bg| (text.max(bg) + 0.05) / (text.min(bg) + 0.05))
            .fold(f32::INFINITY, f32::min)
    };
    if contrast(p.text) >= contrast(p.primary_text) {
        p.text
    } else {
        p.primary_text
    }
}

impl Workbench {
    pub(crate) fn read_git_log(
        &mut self,
        refs: Option<String>,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        if self.restricted() {
            return;
        }
        self.git.log.generation += 1;
        let mine = self.git.log.generation;
        let skip = if append {
            self.git.log.commits.len() as u32
        } else {
            0
        };
        let read = daemon.call::<GitLog>(Call::GitLog {
            path: project,
            skip,
            limit: Some(PAGE),
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.git.log.generation != mine {
                    return;
                }
                match read {
                    Ok(log) => {
                        this.git.log.refs = refs;
                        if append {
                            this.git.log.commits.extend(log.commits);
                        } else {
                            this.git.log.commits = log.commits;
                            let picked = this.git.log.picked.clone();
                            if picked != WORKTREE
                                && !this.git.log.commits.iter().any(|c| c.sha == picked)
                            {
                                this.pick_commit(WORKTREE.into(), cx);
                            }
                        }
                        this.git.log.more = log.more;
                    }
                    Err(err) => this.git.status_error(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows a commit's message and files, or the working tree.
    pub(crate) fn pick_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        self.git.log.picked = sha.clone();
        self.git.log.info = None;
        cx.notify();
        if sha == WORKTREE {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let shown = daemon.call::<GitCommitInfo>(Call::GitShow {
            path: project,
            sha: sha.clone(),
        });
        cx.spawn(async move |this, cx| {
            let shown = shown.await;
            this.update(cx, |this, cx| {
                if this.git.log.picked != sha {
                    return;
                }
                match shown {
                    Ok(info) => this.git.log.info = Some(info),
                    Err(err) => this
                        .git
                        .status_error(format!("Could not read the commit: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The history: a row for the working tree, then the commits.
    pub(crate) fn render_git_log(
        &self,
        notices: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(status) = &self.git.status else {
            return div().into_any_element();
        };
        let mut shas: Vec<(String, Vec<String>)> = vec![(
            WORKTREE.to_string(),
            status.head.clone().into_iter().collect(),
        )];
        shas.extend(
            self.git
                .log
                .commits
                .iter()
                .map(|c| (c.sha.clone(), c.parents.clone())),
        );
        let (graph, lanes) = layout_graph(&shas);
        let graph_width = (6. + lanes.min(MAX_LANES) as f32 * LANE).max(52.);
        let x = |lane: usize| 12. + lane as f32 * LANE;
        let now = crate::panes::plots::now_ms();
        let staged = staged_files(status).len();
        let unstaged = unstaged_files(status).len();
        let head = status.head.clone();
        let edges: Vec<(usize, usize, usize, usize)> = graph
            .iter()
            .enumerate()
            .flat_map(|(row, r)| r.edges.iter().map(move |e| (row, e.from, e.to, e.color)))
            .collect();
        let canvas_height = shas.len() as f32 * look::row();
        let lines = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let origin = bounds.origin;
                for (row, from, to, color) in &edges {
                    let (x1, x2) = (x(*from), x(*to));
                    let y1 = *row as f32 * look::row() + look::row() / 2.;
                    let y2 = y1 + look::row();
                    let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
                    let mut path = PathBuilder::stroke(px(2.));
                    if *row == 0 {
                        path = path.dash_array(&[px(2.), px(2.)]);
                    }
                    path.move_to(at(x1, y1));
                    if x1 == x2 {
                        path.line_to(at(x2, y2));
                    } else {
                        path.cubic_bezier_to(
                            at(x2, y2),
                            at(x1, y1 + look::row() / 2.),
                            at(x2, y2 - look::row() / 2.),
                        );
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, lane_color(&p, *color));
                    }
                }
            },
        )
        .absolute()
        .top_0()
        .left(px(look::PANE_PAD_X + REFS_WIDTH))
        .w(px(graph_width))
        .h(px(canvas_height));
        let hover = p.ink(look::ROW_HOVER);
        let rows = shas.iter().enumerate().map(|(i, (sha, _))| {
            let lane = graph.get(i).map_or(0, |r| r.lane);
            let worktree = sha == WORKTREE;
            let commit = (!worktree).then(|| &self.git.log.commits[i - 1]);
            let on = &self.git.log.picked == sha;
            let pick = sha.clone();
            let refs_col = div()
                .w(px(REFS_WIDTH))
                .flex_none()
                .overflow_hidden()
                .children(commit.map(|c| {
                    let all = badges(c);
                    let extra = all.len().saturating_sub(1);
                    h_flex()
                        .gap(px(look::GAP_2))
                        .children(all.into_iter().next().map(|b| {
                            let (color, name) = match b.kind.as_str() {
                                "head" => (p.accent, "check"),
                                "tag" => (p.busy, "git.tag"),
                                "remote" => (p.muted, "globe"),
                                _ => (p.ok, "git.branch"),
                            };
                            h_flex()
                                .min_w_0()
                                .gap(px(look::GAP_4))
                                .px(px(look::GAP_4))
                                .h(px(17.))
                                .rounded(px(look::CONTROL_RADIUS))
                                .bg(color.opacity(0.14))
                                .text_color(color)
                                .text_size(px(look::small()))
                                .child(icon(name).size(px(look::ICON_SMALL)))
                                .child(
                                    div()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(b.name),
                                )
                                .when(b.remote, |d| {
                                    d.child(icon("globe").size(px(look::ICON_SMALL)))
                                })
                        }))
                        .when(extra > 0, |d| {
                            d.child(
                                div()
                                    .text_size(px(look::small()))
                                    .text_color(p.faint)
                                    .child(format!("+{extra}")),
                            )
                        })
                }));
            let dot: AnyElement = match commit {
                None => div()
                    .absolute()
                    .left(px(x(lane) - 5.))
                    .top(px(look::row() / 2. - 5.))
                    .size(px(10.))
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .border_color(lane_color(&p, lane))
                    .bg(p.bg)
                    .into_any_element(),
                Some(c) => {
                    let face = avatar(&c.author, &c.email, &c.coauthors);
                    let is_head = head.as_deref() == Some(c.sha.as_str());
                    let colors = (p.chart[face.chart_indices.0], p.chart[face.chart_indices.1]);
                    div()
                        .absolute()
                        .left(px(x(lane) - 9.))
                        .top(px(look::row() / 2. - 9.))
                        .size(px(18.))
                        .rounded_full()
                        .border(px(1.5))
                        .border_color(lane_color(&p, lane))
                        .bg(linear_gradient(
                            135.,
                            linear_color_stop(colors.0, 0.),
                            linear_color_stop(colors.1, 1.),
                        ))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(look::tiny()))
                        .font_weight(FontWeight::BOLD)
                        .text_color(avatar_text(&p, colors))
                        .when(is_head, |d| {
                            d.shadow(vec![BoxShadow {
                                color: lane_color(&p, lane).opacity(0.6),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(2.),
                                inset: false,
                            }])
                        })
                        .child(face.initials)
                        .into_any_element()
                }
            };
            let message: AnyElement = match commit {
                None => h_flex()
                    .gap(px(look::GAP_6))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Working tree"))
                    .when(staged > 0, |d| {
                        d.child(badge(format!("{staged} staged"), p.ok))
                    })
                    .when(unstaged > 0, |d| {
                        d.child(badge(format!("{unstaged} unstaged"), p.busy))
                    })
                    .when(staged == 0 && unstaged == 0, |d| {
                        d.child(badge("clean".into(), p.muted))
                    })
                    .into_any_element(),
                Some(c) => h_flex()
                    .gap(px(look::GAP_8))
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(c.subject.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(look::small()))
                            .text_color(p.faint)
                            .child(time_ago(c.time_ms, now)),
                    )
                    .into_any_element(),
            };
            let tip = match commit {
                None => "The files as they are now".to_string(),
                Some(c) => format!(
                    "{}\n{}, {}\n{}",
                    c.subject,
                    avatar(&c.author, &c.email, &c.coauthors).who,
                    git::when(c.time_ms),
                    &c.sha[..10.min(c.sha.len())]
                ),
            };
            h_flex()
                .id(SharedString::from(format!("git-row-{sha}")))
                .h(px(look::row()))
                .px(px(look::PANE_PAD_X))
                .rounded(px(look::CONTROL_RADIUS))
                .whitespace_nowrap()
                .cursor_pointer()
                .when(on, |d| d.bg(p.sel))
                .when(!on, |d| d.hover(move |d| d.bg(hover)))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.pick_commit(pick.clone(), cx)))
                .child(refs_col)
                .child(
                    div()
                        .w(px(graph_width))
                        .flex_none()
                        .h_full()
                        .relative()
                        .child(dot),
                )
                .child(div().flex_1().min_w_0().child(message))
                .into_any_element()
        });
        let older = self.git.log.more.then(|| {
            h_flex()
                .id("git-older")
                .mx(px(look::GAP_4))
                .px(px(look::GAP_8))
                .h(px(look::row()))
                .rounded(px(look::CONTROL_RADIUS))
                .text_size(px(look::small()))
                .text_color(p.accent)
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .child("Older commits")
                .on_click(cx.listener(|this, _, _, cx| {
                    let refs = this.git.log.refs.clone();
                    this.read_git_log(refs, true, cx)
                }))
        });
        let columns = h_flex()
            .flex_none()
            .px(px(look::PANE_PAD_X))
            .h(px(look::dense_row()))
            .text_size(px(look::small()))
            .text_color(p.faint)
            .child(div().w(px(REFS_WIDTH)).child("Branch / Tag"))
            .child(div().w(px(graph_width)).flex_none().child("Graph"))
            .child(div().child("Message"));
        v_flex()
            .relative()
            .flex_none()
            .h(px(self.git.log.height
                + look::strip()
                + git::pane::head_height()))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(look::PaneScroll::new(
                "git-log-scroll",
                look::strip() + git::pane::head_height() + look::dense_row(),
                0.,
                div()
                    .id("git-log")
                    .children(notices)
                    .child(div().relative().child(lines).children(rows))
                    .children(older),
            ))
            .child(look::floating_header(
                &p,
                look::strip() + git::pane::head_height(),
                false,
                columns,
            ))
            .into_any_element()
    }

    pub(crate) fn render_commit_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(info) = &self.git.log.info else {
            return div()
                .flex_1()
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .text_color(p.muted)
                .child("Reading the commit…")
                .into_any_element();
        };
        let face = avatar(&info.author, &info.email, &coauthors_of(&info.message));
        let hover = p.ink(look::ROW_HOVER);
        let sha = info.sha.clone();
        let files = info.files.iter().enumerate().map(|(ix, change)| {
            let (name, dir) = split_path(&change.path);
            let file = sandhill_proto::GitFile {
                path: change.path.clone(),
                from: change.from.clone(),
                staged: Some(change.status.clone()),
                ..Default::default()
            };
            let (letter, kind, meaning) = file_mark(&file, true);
            let color = match kind {
                MarkKind::Added => p.ok,
                MarkKind::Deleted | MarkKind::Conflict => p.err,
                MarkKind::Renamed => p.accent,
                MarkKind::Modified => p.busy,
            };
            let path = change.path.clone();
            let sha = sha.clone();
            let tip = format!("{}\n{meaning}", change.path);
            h_flex()
                .id(("git-change", ix))
                .mx(px(look::GAP_4))
                .px(px(look::GAP_8))
                .h(px(look::row()))
                .gap(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(move |d| d.bg(hover))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.open_diff(path.clone(), false, Some(sha.clone()), cx)
                }))
                .child(
                    div()
                        .w(px(look::ICON_SMALL))
                        .text_size(px(look::small()))
                        .font_weight(FontWeight::BOLD)
                        .text_color(color)
                        .child(letter),
                )
                .child(picture(&look::file_icon(&change.path)).size(px(look::ICON)))
                .child(div().flex_none().child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child(dir),
                )
                .child(
                    div()
                        .text_size(px(look::small()))
                        .text_color(p.faint)
                        .child(line_counts(change.lines.as_ref())),
                )
        });
        v_flex()
            .relative()
            .flex_1()
            .min_h(px(140.))
            .child(look::PaneScroll::new(
                "git-detail-scroll",
                detail_head_height(),
                0.,
                div()
                    .id("git-detail")
                    .child(
                        div()
                            .px(px(look::PANE_PAD_X))
                            .pb(px(look::PANE_PAD_Y))
                            .whitespace_normal()
                            .children(
                                info.message
                                    .lines()
                                    .map(|l| div().min_h(px(look::GAP_8)).child(l.to_string()))
                                    .collect::<Vec<_>>(),
                            ),
                    )
                    .child(
                        h_flex()
                            .px(px(look::PANE_PAD_X))
                            .pt(px(look::GAP_4))
                            .gap(px(look::GAP_4))
                            .text_size(px(look::small()))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.muted)
                                    .child("Changed"),
                            )
                            .child(
                                div()
                                    .text_color(p.faint)
                                    .child(info.files.len().to_string()),
                            ),
                    )
                    .children(files),
            ))
            .child(
                h_flex()
                    .h(px(detail_head_height()))
                    .gap(px(look::GAP_8))
                    .px(px(look::PANE_PAD_X))
                    .text_size(px(look::small()))
                    .child(
                        div()
                            .font_family(look::mono())
                            .text_color(p.accent)
                            .child(info.sha[..7.min(info.sha.len())].to_string()),
                    )
                    .child(div().text_color(p.text).child(face.who))
                    .child(div().text_color(p.faint).child(git::when(info.time_ms)))
                    .child(div().flex_1())
                    .child(
                        icon_button("git-detail-close", "close", &p, false).on_click(
                            cx.listener(|this, _, _, cx| this.pick_commit(WORKTREE.into(), cx)),
                        ),
                    )
                    .map(|row| look::floating_header(&p, 0., false, row)),
            )
            .into_any_element()
    }
}

fn badge(text: String, color: Hsla) -> Div {
    div()
        .px(px(look::GAP_6))
        .rounded(px(look::CONTROL_RADIUS))
        .bg(color.opacity(0.14))
        .text_color(color)
        .text_size(px(look::small()))
        .child(text)
}

impl crate::git::pane::Git {
    pub(crate) fn status_error(&mut self, err: String) {
        self.set_note(err, true);
    }
}
