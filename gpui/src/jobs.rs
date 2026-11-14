//! The Console's Jobs tab, as the Tauri app has it (`JobsPane.tsx`,
//! `model/jobs.ts`): scripts and pipelines the daemon runs in the
//! background, each in a terminal of its own, newest first, with how long
//! each has run, its CPU and memory, and its log. A job keeps running when
//! the window closes.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, JobSpec, JobUsage, SystemStats, TERMINAL_JOB, TerminalEvent, TerminalInfo,
    TerminalSubscribed,
};

use crate::look::{self, Palette, RADIUS, icon, icon_button};
use crate::terminal::TerminalView;
use crate::workbench::{ConsoleTab, Workbench};

/// How often the list, and running jobs' CPU and memory, are read while the
/// tab shows and a job runs.
const POLL: Duration = Duration::from_secs(3);
/// How often the run times count up.
const TICK: Duration = Duration::from_secs(1);

pub(crate) struct Jobs {
    /// This project's jobs, oldest first.
    list: Vec<TerminalInfo>,
    loaded: bool,
    picked: Option<String>,
    /// Each job's log keeps its own view, so switching keeps its scrollback.
    views: HashMap<String, Entity<TerminalView>>,
    usage: HashMap<String, JobUsage>,
    polling: bool,
    command: Entity<InputState>,
    asking: bool,
    /// A job whose Stop was pressed once: pressing it again stops it.
    confirm_stop: Option<String>,
    error: Option<String>,
    pub(crate) render: crate::render_document::RenderState,
}

pub(crate) fn new_jobs(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Jobs, Vec<Subscription>) {
    let command = cx.new(|cx| {
        InputState::new(window, cx).placeholder("A command, like snakemake -j 8 or bash align.sh")
    });
    let subscriptions = vec![cx.subscribe_in(
        &command,
        window,
        |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.start_typed_job(window, cx);
            }
        },
    )];
    let jobs = Jobs {
        list: Vec::new(),
        loaded: false,
        picked: None,
        views: HashMap::new(),
        usage: HashMap::new(),
        polling: false,
        command,
        asking: false,
        confirm_stop: None,
        error: None,
        render: Default::default(),
    };
    (jobs, subscriptions)
}

/// Files that run as jobs with the program for their kind.
pub(crate) fn is_job_script(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    [".r", ".py", ".sh", ".bash", ".nf", ".smk"]
        .iter()
        .any(|ext| lower.ends_with(ext))
        || name == "Snakefile"
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum JobState {
    Running,
    Done,
    Failed,
    Stopped,
}

pub(crate) fn job_state(job: &TerminalInfo) -> JobState {
    if job.running {
        return JobState::Running;
    }
    match &job.exit {
        Some(exit) if exit.signal.is_some() => JobState::Stopped,
        Some(exit) if exit.code == Some(0) => JobState::Done,
        _ => JobState::Failed,
    }
}

/// Ignore historical exits from before a restart; keep daemon-produced output
/// paths with the same completed run as its terminal state.
fn apply_job_exit(job: &mut TerminalInfo, event: &TerminalEvent) -> bool {
    let Some(exit) = &event.exit else {
        return false;
    };
    if event.terminal != job.id || exit.ended_ms < job.started_ms {
        return false;
    }
    job.running = false;
    job.exit = Some(exit.clone());
    job.output_paths.clone_from(&event.output_paths);
    true
}

/// "45s", "12m", "2h 13m", "3d 4h".
pub(crate) fn format_duration(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let h = m / 60;
    if h < 24 {
        return if m % 60 > 0 {
            format!("{h}h {}m", m % 60)
        } else {
            format!("{h}h")
        };
    }
    let d = h / 24;
    if h % 24 > 0 {
        format!("{d}d {}h", h % 24)
    } else {
        format!("{d}d")
    }
}

/// How long a job has run, or ran.
pub(crate) fn job_duration(job: &TerminalInfo, now: i64) -> String {
    let start = if job.started_ms > 0 {
        job.started_ms
    } else {
        job.created_ms
    };
    let end = if job.running {
        now
    } else {
        job.exit.as_ref().map_or(now, |e| e.ended_ms)
    };
    format_duration(end - start)
}

fn format_bytes(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

/// A path inside the job's project, from the project's folder.
fn in_project(job: &TerminalInfo, path: &str) -> String {
    match &job.project {
        Some(project) => path
            .strip_prefix(&format!("{project}/"))
            .unwrap_or(path)
            .to_string(),
        None => path.to_string(),
    }
}

/// The script and its arguments, or the command line.
pub(crate) fn what_runs(job: &TerminalInfo) -> String {
    let spec = job.job.clone().unwrap_or(JobSpec {
        path: None,
        command: None,
    });
    match (spec.path, spec.command) {
        (Some(path), Some(command)) => format!("{} {command}", in_project(job, &path)),
        (Some(path), None) => in_project(job, &path),
        (None, Some(command)) => command,
        (None, None) => job.title.clone(),
    }
}

fn state_label(job: &TerminalInfo) -> String {
    match job_state(job) {
        JobState::Running => "Running".into(),
        JobState::Done => "Finished".into(),
        JobState::Stopped => "Stopped".into(),
        JobState::Failed => format!(
            "Failed, exit {}",
            job.exit
                .as_ref()
                .and_then(|e| e.code)
                .map_or("?".to_string(), |c| c.to_string())
        ),
    }
}

impl Workbench {
    fn jobs_shown(&self) -> bool {
        self.show_console && self.console_tab == ConsoleTab::Jobs
    }

    /// Reads the list when the tab first shows. Called while drawing.
    pub(crate) fn sync_jobs(&mut self, cx: &mut Context<Self>) {
        if self.jobs_shown() && !self.jobs.loaded {
            self.jobs.loaded = true;
            self.refresh_jobs(cx);
        }
    }

    /// The project changed: its jobs are read again when the tab shows.
    pub(crate) fn forget_jobs(&mut self) {
        self.jobs.list.clear();
        self.jobs.views.clear();
        self.jobs.usage.clear();
        self.jobs.picked = None;
        self.jobs.loaded = false;
        self.jobs.render = Default::default();
    }

    pub(crate) fn refresh_jobs(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let project = self.project.clone();
        let listed = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals);
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                if this.project != project {
                    return;
                }
                match listed {
                    Ok(all) => {
                        for job in all
                            .iter()
                            .filter(|t| t.kind == TERMINAL_JOB && t.project == project)
                        {
                            if let Some(notice) = this.jobs.render.observe(job) {
                                this.notice = Some(notice);
                            }
                            if !job.running {
                                this.irkernel_job_ended(&job.id, cx);
                            }
                        }
                        this.jobs.list = all
                            .into_iter()
                            .filter(|t| t.kind == TERMINAL_JOB && t.project == project)
                            .collect();
                        this.jobs.error = None;
                        let ids: Vec<String> =
                            this.jobs.list.iter().map(|j| j.id.clone()).collect();
                        this.jobs.views.retain(|id, _| ids.contains(id));
                    }
                    Err(err) => this.jobs.error = Some(format!("{err:#}")),
                }
                this.follow_picked_job(cx);
                this.poll_jobs(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The job whose log shows: the one picked, else the newest.
    fn shown_job(&self) -> Option<&TerminalInfo> {
        self.jobs
            .picked
            .as_ref()
            .and_then(|id| self.jobs.list.iter().find(|j| &j.id == id))
            .or_else(|| self.jobs.list.last())
    }

    /// Makes a view for the shown job's log and follows its output.
    fn follow_picked_job(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.shown_job().map(|j| j.id.clone()) else {
            return;
        };
        if self.jobs.views.contains_key(&id) {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        let input = daemon.ordered({
            let id = id.clone();
            move |data| Call::TerminalInput {
                terminal: id.clone(),
                data,
            }
        });
        let resize = {
            let daemon = daemon.clone();
            let id = id.clone();
            Box::new(move |cols, rows| {
                daemon.fire(Call::ResizeTerminal {
                    terminal: id.clone(),
                    cols,
                    rows,
                })
            })
        };
        let window = self.window_handle;
        let view = cx.new(|cx| TerminalView::new(id.clone(), input, resize, window, cx));
        self.jobs.views.insert(id.clone(), view);
        let subscribed = daemon.call::<TerminalSubscribed>(Call::SubscribeTerminal {
            terminal: id,
            from: None,
            chat_from: None,
        });
        cx.spawn(async move |this, cx| {
            if let Err(err) = subscribed.await {
                this.update(cx, |this, cx| {
                    this.jobs.error = Some(format!("Could not read the job's log: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Output or an exit for a job's terminal. False when it isn't a job's.
    pub(crate) fn on_job_event(&mut self, event: &TerminalEvent, cx: &mut Context<Self>) -> bool {
        let known = self.jobs.list.iter().any(|j| j.id == event.terminal);
        let view = self.jobs.views.get(&event.terminal).cloned();
        if !known && view.is_none() {
            return false;
        }
        if let Some(view) = view {
            view.update(cx, |view, cx| {
                if let Some(data) = &event.data {
                    view.feed(data, event.replay, cx);
                }
            });
        }
        if let Some(job) = self.jobs.list.iter_mut().find(|j| j.id == event.terminal)
            && apply_job_exit(job, event)
        {
            if let Some(notice) = self.jobs.render.observe(job) {
                self.notice = Some(notice);
            }
            self.irkernel_job_ended(&event.terminal, cx);
            cx.notify();
        }
        true
    }

    /// While the tab shows and a job runs: the run times count up, and the
    /// list and CPU and memory are read again now and then.
    fn poll_jobs(&mut self, cx: &mut Context<Self>) {
        if self.jobs.polling || !self.jobs_shown() || !self.jobs.list.iter().any(|j| j.running) {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        self.jobs.polling = true;
        cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                let ask = ticks.is_multiple_of(POLL.as_secs() as u32);
                ticks += 1;
                let stats =
                    ask.then(|| daemon.call::<SystemStats>(Call::SystemStats { path: None }));
                let stats = match stats {
                    Some(stats) => stats.await.ok(),
                    None => None,
                };
                let go_on = this
                    .update(cx, |this, cx| {
                        if let Some(stats) = stats {
                            this.jobs.usage = stats
                                .jobs
                                .into_iter()
                                .map(|j| (j.terminal.clone(), j))
                                .collect();
                        }
                        cx.notify();
                        let go_on = this.jobs_shown() && this.jobs.list.iter().any(|j| j.running);
                        if !go_on {
                            this.jobs.polling = false;
                        }
                        go_on
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
                cx.background_executor().timer(TICK).await;
            }
        })
        .detach();
    }

    /// Runs a script or command line as a job, in the project's folder.
    pub(crate) fn start_job(&mut self, job: JobSpec, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        if self.held_back("run jobs", cx) {
            return;
        }
        let project = self.project.clone();
        let generation = self.generation;
        self.console_tab = ConsoleTab::Jobs;
        self.show_console = true;
        let started = daemon.call::<TerminalInfo>(Call::StartJob {
            job,
            cwd: project.clone(),
            project: project.clone(),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project != project {
                    return;
                }
                match started {
                    Ok(info) => this.jobs_started(info, cx),
                    Err(err) => this.jobs.error = Some(format!("Could not start the job: {err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A job just started: picked in the list, its log followed.
    pub(crate) fn jobs_started(&mut self, info: TerminalInfo, cx: &mut Context<Self>) {
        self.jobs.error = None;
        self.jobs.picked = Some(info.id.clone());
        self.jobs.loaded = true;
        if !self.jobs.list.iter().any(|j| j.id == info.id) {
            self.jobs.list.push(info);
        }
        self.follow_picked_job(cx);
        self.poll_jobs(cx);
    }

    /// Run as job, above a script: runs the file as saved.
    pub(crate) fn run_open_as_job(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if open.dirty {
            self.problem = Some("Save the file first: a job runs the file as saved.".into());
            cx.notify();
            return;
        }
        let path = open.notebook.path.clone();
        self.start_job(
            JobSpec {
                path: Some(path),
                command: None,
            },
            cx,
        );
    }

    fn start_typed_job(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.jobs.command.read(cx).value().trim().to_string();
        if command.is_empty() {
            return;
        }
        self.jobs
            .command
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.jobs.asking = false;
        self.start_job(
            JobSpec {
                path: None,
                command: Some(command),
            },
            cx,
        );
    }

    fn stop_job(&mut self, id: String, cx: &mut Context<Self>) {
        if self.jobs.confirm_stop.as_deref() != Some(&id) {
            self.jobs.confirm_stop = Some(id);
            cx.notify();
            return;
        }
        self.jobs.confirm_stop = None;
        self.job_call(Call::StopTerminal { terminal: id }, cx);
    }

    fn run_job_again(&mut self, id: String, cx: &mut Context<Self>) {
        self.jobs.picked = Some(id.clone());
        // Its log is read again, with the new run's output, once it starts.
        if self.jobs.views.remove(&id).is_some()
            && let Some(daemon) = self.daemon()
        {
            daemon.fire(Call::UnsubscribeTerminal {
                terminal: id.clone(),
            });
        }
        self.job_call(
            Call::RestartTerminal {
                terminal: id,
                options: None,
                cols: None,
                rows: None,
                prompt: None,
            },
            cx,
        );
    }

    fn remove_job(&mut self, id: String, cx: &mut Context<Self>) {
        self.jobs.list.retain(|j| j.id != id);
        self.jobs.views.remove(&id);
        if self.jobs.picked.as_deref() == Some(&id) {
            self.jobs.picked = None;
        }
        self.job_call(Call::CloseTerminal { terminal: id }, cx);
    }

    /// Sends a call about a job, then reads the list again.
    fn job_call(&mut self, call: Call, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let done = daemon.call::<serde_json::Value>(call);
        cx.spawn(async move |this, cx| {
            let done = done.await;
            this.update(cx, |this, cx| {
                if let Err(err) = done {
                    this.jobs.error = Some(format!("{err:#}"));
                }
                this.refresh_jobs(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The Jobs tab's body: the list on the left, the shown job's log on
    /// the right.
    pub(crate) fn render_jobs(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let now = crate::plots::now_ms();
        let shown = self.shown_job().map(|j| j.id.clone());
        let hover = p.ink(7.);
        let rows: Vec<AnyElement> = self
            .jobs
            .list
            .iter()
            .rev()
            .map(|job| {
                let id = job.id.clone();
                let state = job_state(job);
                let on = shown.as_deref() == Some(job.id.as_str());
                let mut detail = vec![job_duration(job, now)];
                if job.running
                    && let Some(usage) = self.jobs.usage.get(&job.id)
                {
                    if let Some(cpu) = usage.cpu {
                        detail.push(format!("{}% CPU", cpu.round()));
                    }
                    detail.push(format_bytes(usage.rss));
                }
                let mark = match state {
                    JobState::Running => div()
                        .size(px(8.))
                        .rounded_full()
                        .bg(p.busy)
                        .into_any_element(),
                    JobState::Done => icon("check")
                        .size(px(13.))
                        .text_color(p.ok)
                        .into_any_element(),
                    _ => icon("close")
                        .size(px(13.))
                        .text_color(p.err)
                        .into_any_element(),
                };
                let (pick, stop, again, remove) = (id.clone(), id.clone(), id.clone(), id.clone());
                let confirming = self.jobs.confirm_stop.as_deref() == Some(job.id.as_str());
                h_flex()
                    .id(SharedString::from(format!("job-{}", job.id)))
                    .mx(px(4.))
                    .px(px(8.))
                    .gap(px(8.))
                    .py(px(3.))
                    .rounded(px(RADIUS))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .when(on, |d| d.bg(p.ink(9.)))
                    .hover(move |d| d.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.jobs.picked = Some(pick.clone());
                        this.follow_picked_job(cx);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(mark),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(p.text)
                                    .child(job.name.clone().unwrap_or_else(|| job.title.clone())),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(detail.join(" · ")),
                            ),
                    )
                    .map(|d| {
                        if job.running {
                            d.child(if confirming {
                                div()
                                    .id(SharedString::from(format!("job-stop-{}", job.id)))
                                    .px(px(6.))
                                    .rounded_full()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.err)
                                    .hover(|d| d.bg(p.ink(10.)))
                                    .child("Stop?")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.stop_job(stop.clone(), cx)
                                    }))
                                    .into_any_element()
                            } else {
                                icon_button(
                                    SharedString::from(format!("job-stop-{}", job.id)),
                                    "stop",
                                    &p,
                                    false,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.stop_job(stop.clone(), cx)
                                }))
                                .into_any_element()
                            })
                        } else {
                            d.child(
                                icon_button(
                                    SharedString::from(format!("job-again-{}", job.id)),
                                    "refresh",
                                    &p,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.run_job_again(again.clone(), cx)
                                    },
                                )),
                            )
                            .child(
                                icon_button(
                                    SharedString::from(format!("job-remove-{}", job.id)),
                                    "delete",
                                    &p,
                                    false,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.remove_job(remove.clone(), cx)
                                    },
                                )),
                            )
                        }
                    })
                    .into_any_element()
            })
            .collect();
        let new_job = h_flex()
            .flex_none()
            .h(px(32.))
            .px(px(8.))
            .py(px(6.))
            .gap(px(6.))
            .child(
                Button::new("new-job")
                    .xsmall()
                    .label("New job")
                    .icon(gpui_kit::component::Icon::default().path("sandhill/system/add.svg"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.jobs.asking = !this.jobs.asking;
                        if this.jobs.asking {
                            this.jobs
                                .command
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    })),
            );
        let ask = self.jobs.asking.then(|| {
            v_flex()
                .flex_none()
                .px(px(8.))
                .pb(px(6.))
                .gap(px(4.))
                .child(Input::new(&self.jobs.command).small())
                .child(
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child("Runs in this project's folder with your shell's setup, and keeps running when Sandhill closes. Enter starts it."),
                )
        });
        let list = v_flex()
            .relative()
            .w(relative(0.4))
            .min_w(px(180.))
            .max_w(px(320.))
            .flex_none()
            .h_full()
            .border_r(px(0.5))
            .border_color(p.glass_edge)
            .child(
                look::PaneScroll::new(
                    "jobs-scroll",
                    look::STRIP + 32.,
                    0.,
                div()
                    .id("jobs-list")
                    .children(ask)
                    .when_some(self.jobs.error.clone(), |d, err| {
                        d.child(
                            div()
                                .px(px(12.))
                                .py(px(4.))
                                .text_size(px(look::SMALL))
                                .text_color(p.err)
                                .child(err),
                        )
                    })
                    .when(self.jobs.list.is_empty() && self.jobs.loaded, |d| {
                        d.child(
                            div()
                                .px(px(12.))
                                .py(px(6.))
                                .text_color(p.muted)
                                .child("No jobs yet. Run a script in the background with Run as job above a script, or start a command with New job. Jobs keep running when Sandhill closes."),
                        )
                    })
                    .children(rows),
                ),
            )
            .child(look::floating_header(&p, look::STRIP, false, new_job));
        let log = self.shown_job().map(|job| {
            let place = if job.project.as_deref() == Some(job.cwd.as_str()) {
                job.cwd.rsplit('/').next().unwrap_or(&job.cwd).to_string()
            } else {
                in_project(job, &job.cwd)
            };
            let state_color = match job_state(job) {
                JobState::Running => p.busy,
                JobState::Done => p.ok,
                _ => p.err,
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .pt(px(look::STRIP))
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(5.))
                        .border_b(px(0.5))
                        .border_color(p.glass_edge)
                        .text_size(px(look::SMALL))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(div().text_color(state_color).child(state_label(job)))
                        .child(
                            div()
                                .font_family(look::MONO)
                                .text_color(p.text)
                                .child(what_runs(job)),
                        )
                        .child(div().text_color(p.faint).child(format!("in {place}"))),
                )
                .child(self.render_output_actions(
                    &job.output_paths,
                    &format!("job-{}", job.id),
                    cx,
                ))
                .child(
                    // Absolute, so the log's height is the pane's, not its
                    // text's.
                    div().flex_1().min_h_0().relative().child(
                        div()
                            .absolute()
                            .inset_0()
                            .bg(p.term)
                            .children(self.jobs.views.get(&job.id).cloned()),
                    ),
                )
        });
        h_flex()
            .size_full()
            .child(list)
            .children(log)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{JobState, format_duration, is_job_script, job_state, what_runs};
    use sandhill_proto::TerminalInfo;

    fn job(value: serde_json::Value) -> TerminalInfo {
        let mut base = serde_json::json!({
            "id": "t1", "kind": "job", "title": "align.sh", "cwd": "/p",
            "project": "/p", "created_ms": 0, "running": false, "end": 0
        });
        for (k, v) in value.as_object().unwrap() {
            base[k] = v.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn scripts_and_durations() {
        assert!(is_job_script("/p/scripts/qc.R"));
        assert!(is_job_script("/p/Snakefile"));
        assert!(is_job_script("main.nf"));
        assert!(!is_job_script("notes.txt"));
        assert_eq!(format_duration(45_000), "45s");
        assert_eq!(format_duration(12 * 60_000), "12m");
        assert_eq!(format_duration((2 * 60 + 13) * 60_000), "2h 13m");
        assert_eq!(format_duration(27 * 3_600_000), "1d 3h");
    }

    #[test]
    fn states_and_what_runs() {
        assert_eq!(
            job_state(&job(serde_json::json!({"running": true}))),
            JobState::Running
        );
        assert_eq!(
            job_state(&job(
                serde_json::json!({"exit": {"code": 0, "ended_ms": 5}})
            )),
            JobState::Done
        );
        assert_eq!(
            job_state(&job(
                serde_json::json!({"exit": {"signal": 15, "ended_ms": 5}})
            )),
            JobState::Stopped
        );
        assert_eq!(
            job_state(&job(
                serde_json::json!({"exit": {"code": 2, "ended_ms": 5}})
            )),
            JobState::Failed
        );
        let j = job(serde_json::json!({"job": {"path": "/p/scripts/align.sh", "command": "-t 8"}}));
        assert_eq!(what_runs(&j), "scripts/align.sh -t 8");
    }

    #[test]
    fn completed_render_keeps_outputs_and_ignores_pre_restart_exits() {
        let mut info = job(serde_json::json!({"running": true, "started_ms": 20}));
        let mut event = sandhill_proto::TerminalEvent {
            terminal: info.id.clone(),
            exit: Some(sandhill_proto::TerminalExit {
                code: Some(0),
                signal: None,
                ended_ms: 19,
                idle: false,
            }),
            output_paths: vec!["/p/report.html".into()],
            ..Default::default()
        };
        assert!(!super::apply_job_exit(&mut info, &event));
        assert!(info.running);
        assert!(info.output_paths.is_empty());
        event.exit.as_mut().unwrap().ended_ms = 21;
        assert!(super::apply_job_exit(&mut info, &event));
        assert!(!info.running);
        assert_eq!(info.output_paths, ["/p/report.html"]);
    }
}
