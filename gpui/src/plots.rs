//! Opt-in Console plot forwarding and the project's notebook plot history.
//! Images remain journaled by the daemon regardless of forwarding.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, EVENT_RUN_STARTED, FileStat, HistoryPage, PlotRedraw, SessionPlots};
use serde_json::Value;

use crate::look::{self, Palette, RADIUS, icon_button};
use crate::prefs::{PlotSizing, PlotUnit};
use crate::workbench::{BottomTab, Plot, Workbench, is_script, plot_key};

/// The image types shown, best first: an SVG can be saved at any size.
const MIMES: [&str; 4] = ["image/svg+xml", "image/png", "image/jpeg", "image/gif"];
/// Plots read from each session's journal when it is first seen.
const LOAD: u32 = 100;
/// How many plots the pane keeps, all sessions together.
const MAX_PLOTS: usize = 300;
/// Where plots are saved unless the user typed another folder last.
const FOLDER: &str = "plots";
/// Pixels in an inch for saved PNGs.
const DPI: u32 = 300;
/// How long the pane's size holds still before the plot in front is drawn
/// again for it.
const FIT_DELAY: Duration = Duration::from_millis(400);
/// Points in an inch, which sizes are given to the session in.
const POINTS: f32 = 96.;
/// Rooms smaller than this aren't worth drawing for.
const MIN_FIT: f32 = 80.;
/// Redrawn plots kept to show again without drawing.
const FITS_KEPT: usize = 12;

/// A plot in the history.
#[derive(Clone)]
pub(crate) struct PlotItem {
    /// The run, then the image: the same plot read twice has the same key.
    pub key: String,
    pub run: String,
    pub session: String,
    pub ts_ms: i64,
    /// The time is the journal's; plots seen as a run goes have its start.
    pub exact: bool,
    pub cell: Option<(String, String)>,
    pub by: Option<String>,
    pub mime: String,
    pub value: Value,
}

/// The large view of one plot, to save it.
pub(crate) struct Zoom {
    /// Whether its session can draw it again at any size; `None` while asked.
    pub redraw: Option<PlotRedraw>,
    /// Keep the preview on this plot when new live output arrives.
    key: String,
    /// The pane drops ended sessions; an already-open preview keeps its image.
    retained: Option<PlotItem>,
    preview: Option<Arc<Image>>,
    preview_note: Option<String>,
    room: Rc<Cell<Option<PreviewSize>>>,
    fitting: Option<Task<()>>,
    size: Option<Size<Pixels>>,
    resizing: Option<(Point<Pixels>, Size<Pixels>)>,
    offset: Point<Pixels>,
    moving: Option<(Point<Pixels>, Point<Pixels>)>,
    initial_picture: Option<Size<Pixels>>,
    follow: bool,
    redraw_preview: bool,
    unit: PlotUnit,
    units_open: bool,
    physical: (f64, f64),
    folder: String,
    folder_dialog: Option<Entity<crate::folder_dialog::FolderDialog>>,
    _subscriptions: Vec<Subscription>,
    pub format: &'static str,
    pub width: Entity<InputState>,
    pub height: Entity<InputState>,
    pub name: Entity<InputState>,
    pub dpi: u32,
    dpi_input: Entity<InputState>,
    /// What happened last: saved, or why not.
    pub note: Option<String>,
    /// A path that is there already; saving again replaces it.
    pub replace: Option<String>,
    pub working: bool,
    /// Opened from under a notebook cell: that cell's plots, and the one
    /// shown; otherwise the view steps through the Plots pane's.
    pub only: Option<(Vec<PlotItem>, usize)>,
}

/// Logical canvas size and backing resolution, not export dimensions.
#[derive(Clone, Copy, PartialEq)]
struct PreviewSize {
    width: f64,
    height: f64,
    dpi: u32,
}

/// Keep the accepted size as the tolerance's anchor, so small real resizes
/// accumulate. Fixed physical dimensions and backing DPI are always exact.
fn preview_size_changed(
    previous: Option<PreviewSize>,
    next: PreviewSize,
    sizing: PlotSizing,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    match sizing {
        PlotSizing::Fixed { .. } => previous != next,
        PlotSizing::Fit => {
            previous.dpi != next.dpi
                || (previous.width - next.width).abs() > 1.
                || (previous.height - next.height).abs() > 1.
        }
    }
}

#[derive(Default)]
pub(crate) struct History {
    pub items: Vec<PlotItem>,
    /// The plot in front, by key; none for the newest.
    pub picked: Option<String>,
    /// Sessions whose journal was read.
    loaded: HashSet<String>,
    /// Runs drawing now; read again from the journal once they finish.
    drawing: HashSet<String>,
    /// The newest plot cleared away.
    cleared: Option<i64>,
    pub zoom: Option<Zoom>,
    /// The picture's actual on-screen bounds, before opening its window.
    pub(crate) image_bounds: Rc<RefCell<HashMap<String, Size<Pixels>>>>,
    /// The plot in front and the room it has, as last drawn
    /// (`key@WxH`): a change asks for it to be drawn again.
    room: Rc<RefCell<String>>,
    /// Plots drawn again for the pane, by `key@WxH`, newest last.
    fits: Vec<(String, Arc<Image>)>,
    /// Plots their session can't draw again.
    cannot: HashSet<String>,
    /// Waits for the room to hold still, then asks.
    fitting: Option<Task<()>>,
    sizing: PlotSizing,
    sizing_project: Option<(String, String)>,
    size_menu: bool,
    size_inputs: Option<(Entity<InputState>, Entity<InputState>, Entity<InputState>)>,
    size_error: Option<String>,
    notes: HashMap<String, String>,
    pub console_auto: bool,
    sent: crate::prefs::SentPlots,
}

/// A plot's key with the room it is drawn for.
fn fit_key(plot: &str, width: u32, height: u32) -> String {
    format!("{plot}@{width}x{height}")
}

fn redraw_size(sizing: PlotSizing, width: f32, height: f32, scale: f32) -> (f64, f64, u32) {
    match sizing {
        PlotSizing::Fixed { width, height, dpi } => (width, height, dpi),
        PlotSizing::Fit => (
            f64::from(width / POINTS),
            f64::from(height / POINTS),
            (POINTS * scale.clamp(1., 4.)).round() as u32,
        ),
    }
}

fn size_key(plot: &str, sizing: PlotSizing, width: f32, height: f32) -> String {
    match sizing {
        PlotSizing::Fit => fit_key(plot, width.round() as u32, height.round() as u32),
        PlotSizing::Fixed { width, height, dpi } => format!("{plot}@fixed-{width}x{height}@{dpi}"),
    }
}

/// The export rule uses logical screen points, not the display's backing scale.
fn window_inches(width: f64, height: f64) -> (f64, f64) {
    (width / f64::from(POINTS), height / f64::from(POINTS))
}

fn dimension_text(inches: f64, unit: PlotUnit, dpi: u32) -> String {
    let value = unit.display_value(inches, dpi);
    if unit == PlotUnit::Pixels {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

/// Keep at least 96 points of the title horizontally and all of it vertically.
fn clamp_zoom_offset(
    offset: Point<Pixels>,
    card: Size<Pixels>,
    viewport: Size<Pixels>,
) -> Point<Pixels> {
    let origin = point(
        (viewport.width - card.width) / 2.,
        (viewport.height - card.height) / 2.,
    );
    point(
        offset.x.clamp(
            px(96.).min(card.width).min(viewport.width / 2.) - card.width - origin.x,
            viewport.width - px(96.).min(card.width).min(viewport.width / 2.) - origin.x,
        ),
        offset.y.clamp(
            -origin.y,
            (viewport.height - px(36.) - origin.y).max(-origin.y),
        ),
    )
}
/// A plot as the clipboard takes it: as drawn, except an SVG, which is
/// drawn on white at twice its size so it stays sharp.
pub(crate) fn clipboard_image(image: &Image) -> anyhow::Result<Image> {
    if image.format() != ImageFormat::Svg {
        return Ok(Image::from_bytes(image.format(), image.bytes().to_vec()));
    }
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(image.bytes(), &options)?;
    let size = tree
        .size()
        .to_int_size()
        .scale_by(2.)
        .unwrap_or(tree.size().to_int_size());
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| anyhow::anyhow!("the plot has no size"))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(2., 2.),
        &mut pixmap.as_mut(),
    );
    Ok(Image::from_bytes(ImageFormat::Png, pixmap.encode_png()?))
}

impl History {
    /// Recorded plots remain available after their session ends.
    fn shown(&self) -> Vec<&PlotItem> {
        self.items
            .iter()
            .filter(|p| self.cleared.is_none_or(|c| p.ts_ms > c))
            .collect()
    }

    /// Adds plots, or updates ones it has, oldest first, keeping the newest
    /// [`MAX_PLOTS`]. Whether one was new.
    fn add(&mut self, add: Vec<PlotItem>) -> bool {
        let mut new = false;
        for item in add {
            match self.items.iter_mut().find(|p| p.key == item.key) {
                Some(known) if !known.exact || item.exact => *known = item,
                Some(_) => {}
                None => {
                    self.items.push(item);
                    new = true;
                }
            }
        }
        // Stable: plots of one run keep the order they came in.
        self.items.sort_by_key(|p| p.ts_ms);
        if self.items.len() > MAX_PLOTS {
            self.items.drain(..self.items.len() - MAX_PLOTS);
        }
        new
    }
}

/// A plot from an output's MIME bundle; nothing if it holds no image.
fn plot_item(
    run: &str,
    session: &str,
    ts_ms: i64,
    exact: bool,
    cell: Option<(String, String)>,
    by: Option<String>,
    data: &Value,
) -> Option<PlotItem> {
    let mime = MIMES.into_iter().find(|m| data.get(*m).is_some())?;
    let value = data[mime].clone();
    Some(PlotItem {
        key: format!("{run}:{}", plot_key(&value)),
        run: run.to_string(),
        session: session.to_string(),
        ts_ms,
        exact,
        cell,
        by,
        mime: mime.to_string(),
        value,
    })
}

/// Plots of notebooks stay under their cells; scripts' come here.
pub(crate) fn from_notebook(cell: &Option<(String, String)>) -> bool {
    cell.as_ref().is_some_and(|(file, _)| !is_script(file))
}

/// Where a plot came from: "Console", an agent, or a file's name.
fn source(item: &PlotItem) -> String {
    match (&item.cell, &item.by) {
        (Some((file, _)), _) => file.rsplit('/').next().unwrap_or(file).to_string(),
        (None, Some(by)) => by.clone(),
        _ => "Console".into(),
    }
}

/// "just now", "5 min ago", "2 h ago", "3 d ago".
fn ago(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A file name for a plot, without its extension: its file's name, else
/// "plot"; numbered when its run drew several.
fn plot_name(item: &PlotItem, all: &[&PlotItem]) -> String {
    let base = match &item.cell {
        Some((file, _)) => {
            let name = file.rsplit('/').next().unwrap_or(file);
            name.rsplit_once('.')
                .map_or(name, |(stem, _)| stem)
                .to_string()
        }
        None => "plot".to_string(),
    };
    let siblings: Vec<_> = all.iter().filter(|p| p.run == item.run).collect();
    let name = if siblings.len() > 1 {
        let at = siblings.iter().position(|p| p.key == item.key).unwrap_or(0);
        format!("{base}-{}", at + 1)
    } else {
        base
    };
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_matches(|c| c == '-' || c == '.').to_string();
    if clean.is_empty() {
        "plot".into()
    } else {
        clean
    }
}

/// What a plot is to its session, to draw it again: the blob of its image,
/// and its place among its run's plots.
fn redraw_target(item: &PlotItem, all: &[&PlotItem]) -> (Option<String>, u32) {
    let image = item.value[sandhill_proto::BLOB_KEY]
        .as_str()
        .map(str::to_string);
    let index = all
        .iter()
        .filter(|p| p.run == item.run)
        .position(|p| p.key == item.key)
        .unwrap_or(0);
    (image, index as u32)
}

/// The formats a plot can be saved in without its session drawing it again.
fn plain_formats(mime: &str) -> &'static [&'static str] {
    match mime {
        "image/svg+xml" => &["svg"],
        "image/jpeg" => &["jpeg"],
        "image/gif" => &["gif"],
        _ => &["png"],
    }
}

fn extension(format: &str) -> &str {
    if format == "jpeg" { "jpg" } else { format }
}

impl Workbench {
    pub(crate) fn track_plot_bounds(
        &self,
        key: String,
        image: Arc<Image>,
    ) -> impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + 'static {
        let tracked = self.plot_history.image_bounds.clone();
        move |bounds, window, cx| {
            let mut shown = bounds.size;
            if let Some(rendered) = image.get_render_image(window, cx) {
                let pixels = rendered.size(0);
                let width = pixels.width.0 as f32;
                let height = pixels.height.0 as f32;
                if width > 0. && height > 0. {
                    let scale =
                        (f32::from(shown.width) / width).min(f32::from(shown.height) / height);
                    shown = size(px(width * scale), px(height * scale));
                }
            }
            tracked.borrow_mut().insert(key, shown);
        }
    }
    /// The sessions whose plots show: every kernel open in this window.
    fn live_sessions(&self) -> Vec<String> {
        self.open
            .iter()
            .chain(&self.others)
            .filter_map(|o| o.session.id())
            .chain(self.loose.id())
            .chain(self.console_sessions.retained.iter().map(String::as_str))
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }

    /// Reads the plots of sessions not read yet. Called as the window draws.
    pub(crate) fn sync_plots(&mut self, cx: &mut Context<Self>) {
        let project = (self.host(), self.project.clone().unwrap_or_default());
        if self.plot_history.sizing_project.as_ref() != Some(&project) {
            self.plot_history.sizing = crate::prefs::plot_sizing(&project.0, &project.1);
            self.plot_history.console_auto = crate::prefs::console_plots(&project.0);
            self.plot_history.sent = crate::prefs::sent_plots();
            self.plot_history.sizing_project = Some(project);
            self.plot_history.room.borrow_mut().clear();
            self.plot_history.size_menu = false;
        }
        for session in self.live_sessions() {
            if self.plot_history.loaded.insert(session.clone()) {
                self.load_plots(session, LOAD, cx);
            }
        }
    }

    /// Reads a session's latest plots from the daemon's journal.
    fn load_plots(&mut self, session: String, limit: u32, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let read = daemon.call::<SessionPlots>(Call::SessionPlots {
            session: session.clone(),
            limit: Some(limit),
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok(read) => {
                        let items: Vec<PlotItem> = read
                            .plots
                            .iter()
                            .filter_map(|p| {
                                let cell = p.cell.clone().map(|c| (c.file, c.cell_id));
                                let item = plot_item(
                                    &p.run,
                                    &session,
                                    p.ts_ms,
                                    true,
                                    cell,
                                    p.by.clone(),
                                    &p.data,
                                )?;
                                this.plot_allowed(&item).then_some(item)
                            })
                            .collect();
                        if this.plot_history.console_auto {
                            this.remember_sent_plots(&items);
                        }
                        this.plot_history.add(items);
                        cx.notify();
                    }
                    Err(_) => {
                        this.plot_history.loaded.remove(&session);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// A run outside notebooks showed something: its plots come to the front.
    pub(crate) fn plots_from_run(&mut self, session: &str, run_id: &str, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get(run_id) else {
            return;
        };
        if run.quiet {
            return;
        }
        let items: Vec<PlotItem> = run
            .outputs
            .iter()
            .filter(|o| {
                matches!(
                    o["output_type"].as_str(),
                    Some("display_data" | "execute_result")
                )
            })
            .filter_map(|o| {
                plot_item(
                    run_id,
                    session,
                    now_ms(),
                    false,
                    run.cell.clone(),
                    run.by.clone(),
                    &o["data"],
                )
            })
            .filter(|item| self.plot_allowed(item))
            .collect();
        if items.is_empty() {
            return;
        }
        if self.plot_history.console_auto {
            self.remember_sent_plots(&items);
        }
        self.plot_history.drawing.insert(run_id.to_string());
        if self.plot_history.add(items) {
            self.plot_history.picked = None;
            self.bottom_tab = BottomTab::Plots;
        }
        cx.notify();
    }

    fn plot_allowed(&self, item: &PlotItem) -> bool {
        if from_notebook(&item.cell) {
            self.notebook_plots
        } else {
            self.plot_history.console_auto
                || self.plot_history.sent.iter().any(|entry| {
                    entry[0] == self.host() && entry[1] == item.session && entry[2] == item.key
                })
        }
    }

    pub(crate) fn toggle_console_plots(&mut self, cx: &mut Context<Self>) {
        self.plot_history.console_auto = !self.plot_history.console_auto;
        crate::prefs::save_console_plots(&self.host(), self.plot_history.console_auto);
        self.plot_history.loaded.clear();
        cx.notify();
    }

    fn run_plots(&self, run_id: &str) -> Vec<PlotItem> {
        let Some(run) = self.runs.get(run_id).filter(|run| !run.quiet) else {
            return Vec::new();
        };
        run.outputs
            .iter()
            .filter_map(|output| {
                plot_item(
                    run_id,
                    &run.session,
                    run.queued_ms.unwrap_or_else(now_ms),
                    false,
                    run.cell.clone(),
                    run.by.clone(),
                    &output["data"],
                )
            })
            .collect()
    }

    fn remember_sent_plots(&mut self, items: &[PlotItem]) {
        let host = self.host();
        let mut sent = crate::prefs::sent_plots();
        let mut changed = false;
        for item in items.iter().filter(|item| !from_notebook(&item.cell)) {
            changed |= crate::prefs::remember_plot(&mut sent, &host, &item.session, &item.key);
        }
        if changed {
            crate::prefs::save_sent_plots(&sent);
        }
        self.plot_history.sent = sent;
    }

    fn send_plot_items(&mut self, items: Vec<PlotItem>, cx: &mut Context<Self>) {
        if items.is_empty() {
            return;
        }
        self.remember_sent_plots(&items);
        self.plot_history.picked = items.last().map(|item| item.key.clone());
        self.plot_history.add(items);
        self.bottom_tab = BottomTab::Plots;
        cx.notify();
    }

    pub(crate) fn send_console_plot(&mut self, run: &str, key: &str, cx: &mut Context<Self>) {
        let items = self
            .run_plots(run)
            .into_iter()
            .filter(|item| plot_key(&item.value) == key)
            .collect();
        self.send_plot_items(items, cx);
    }

    pub(crate) fn send_all_console_plots(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.loose.id().map(str::to_string) else {
            return;
        };
        let host = self.host();
        let Some(daemon) = self.daemon() else { return };
        let generation = self.generation;
        // SessionPlots is capped by the daemon. Walk the journal so Send all
        // also includes plots older than its latest-image window.
        cx.spawn(async move |this, cx| {
            let read: anyhow::Result<Vec<PlotItem>> = async {
                let mut since_seq = 0;
                let mut runs = HashMap::new();
                let mut items = Vec::new();
                loop {
                    let page = daemon
                        .call::<HistoryPage>(Call::History {
                            session: session.clone(),
                            since_seq,
                            limit: Some(5000),
                        })
                        .await?;
                    for event in page.events {
                        let Some(run) = event.run else { continue };
                        if event.msg_type == EVENT_RUN_STARTED {
                            let cell = event.content["cell"]["file"]
                                .as_str()
                                .zip(event.content["cell"]["cell_id"].as_str())
                                .map(|(file, id)| (file.to_string(), id.to_string()));
                            runs.insert(
                                run,
                                (
                                    cell,
                                    event.content["by"].as_str().map(str::to_string),
                                    event.content["quiet"].as_bool().unwrap_or(false),
                                ),
                            );
                        } else if matches!(
                            event.msg_type.as_str(),
                            "display_data" | "execute_result" | "update_display_data"
                        ) && let Some((cell, by, false)) = runs.get(&run)
                            && !from_notebook(cell)
                            && let Some(item) = plot_item(
                                &run,
                                &session,
                                event.ts_ms,
                                true,
                                cell.clone(),
                                by.clone(),
                                &event.content["data"],
                            )
                        {
                            items.push(item);
                        }
                    }
                    if page.next_seq <= since_seq {
                        break;
                    }
                    since_seq = page.next_seq;
                }
                Ok(items)
            }
            .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match read {
                    Ok(items) => {
                        let items = items
                            .into_iter()
                            .filter(|item| {
                                !this.plot_history.sent.iter().any(|entry| {
                                    entry[0] == host && entry[1] == session && entry[2] == item.key
                                })
                            })
                            .collect();
                        this.send_plot_items(items, cx);
                    }
                    Err(err) => {
                        this.problem = Some(format!("Loading Console plots failed: {err:#}"));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_console_plot(
        &mut self,
        run: &str,
        key: &str,
        bounds_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.run_plots(run);
        let Some(at) = items.iter().position(|item| plot_key(&item.value) == key) else {
            return;
        };
        let picture = self
            .plot_history
            .image_bounds
            .borrow()
            .get(bounds_key)
            .copied();
        self.open_zoom_on(Some((items, at)), picture, window, cx);
    }

    /// A run finished: plots it drew are read again, for the journal's times.
    pub(crate) fn plots_run_finished(
        &mut self,
        session: &str,
        run_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.plot_history.drawing.remove(run_id) {
            return;
        }
        let count = self
            .plot_history
            .items
            .iter()
            .filter(|p| p.run == run_id)
            .count()
            .max(1);
        self.load_plots(session.to_string(), count as u32, cx);
    }

    /// The plots shown, and the one in front.
    fn plots_now(&self) -> (Vec<&PlotItem>, Option<usize>) {
        let list = self.plot_history.shown();
        let at = self
            .plot_history
            .picked
            .as_ref()
            .and_then(|k| list.iter().position(|p| &p.key == k))
            .or_else(|| list.len().checked_sub(1));
        (list, at)
    }

    fn step_plot(&mut self, by: isize, cx: &mut Context<Self>) {
        let (list, at) = self.plots_now();
        let Some(at) = at else {
            return;
        };
        let to = (at as isize + by).clamp(0, list.len() as isize - 1) as usize;
        let key = (to + 1 < list.len()).then(|| list[to].key.clone());
        self.plot_history.picked = key;
        cx.notify();
    }

    fn pick_plot(&mut self, ix: usize, cx: &mut Context<Self>) {
        let (list, _) = self.plots_now();
        let key = (ix + 1 < list.len()).then(|| list[ix].key.clone());
        self.plot_history.picked = key;
        cx.notify();
    }

    /// Copies the plot in front, as drawn, to the clipboard.
    fn copy_plot(&mut self, cx: &mut Context<Self>) {
        let (list, at) = self.plots_now();
        let Some(image) = at.and_then(|at| self.plot_image(list[at])) else {
            return;
        };
        self.copy_image(&image, cx);
    }

    /// Copies a plot, as drawn, to the clipboard.
    pub(crate) fn copy_image(&mut self, image: &Image, cx: &mut Context<Self>) {
        match clipboard_image(image) {
            Ok(image) => {
                cx.write_to_clipboard(ClipboardItem::new_image(&image));
                self.problem = None;
                self.notice = Some("Copied the plot.".into());
            }
            Err(err) => self.problem = Some(format!("Could not copy the plot: {err:#}")),
        }
        cx.notify();
    }

    /// The pane's room or the plot in front changed: once the room holds
    /// still, the plot's session draws it again at that size, as RStudio
    /// does, so text and lines stay sharp instead of stretching. Plain
    /// images, plots the session no longer keeps and sessions busy with
    /// other code keep the image as drawn.
    fn fit_plot(
        &mut self,
        key: String,
        width: f32,
        height: f32,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        self.plot_history.fitting = None;
        let (list, at) = self.plots_now();
        let Some(item) = at.map(|at| list[at]) else {
            return;
        };
        let sizing = self.plot_history.sizing;
        if item.key != key
            || (matches!(sizing, PlotSizing::Fit)
                && (width < MIN_FIT || height < MIN_FIT || item.mime == "image/svg+xml"))
            || self.plot_history.cannot.contains(&item.key)
        {
            return;
        }
        let wanted = size_key(&item.key, sizing, width, height);
        if self.plot_history.fits.iter().any(|(k, _)| *k == wanted) {
            cx.notify();
            return;
        }
        let Some(daemon) = self.daemon() else {
            self.plot_history.notes.insert(
                key,
                "Recorded image: reconnect to redraw at the selected size.".into(),
            );
            cx.notify();
            return;
        };
        let (image, index) = redraw_target(item, &list);
        let (width, height, dpi) = redraw_size(sizing, width, height, scale);
        let generation = self.generation;
        let call = Call::RedrawPlot {
            session: item.session.clone(),
            run: item.run.clone(),
            image,
            index,
            file: None,
            format: String::new(),
            width,
            height,
            dpi,
            show: true,
        };
        self.plot_history.fitting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIT_DELAY).await;
            let asked = daemon.call::<PlotRedraw>(call).await;
            this.update(cx, |this, cx| {
                if this.generation != generation || *this.plot_history.room.borrow() != wanted {
                    return;
                }
                match asked {
                    Ok(PlotRedraw {
                        can: true,
                        png: Some(png),
                        ..
                    }) => {
                        let Ok(bytes) = sandhill_proto::decode_base64(png.trim()) else {
                            return;
                        };
                        let fits = &mut this.plot_history.fits;
                        fits.push((wanted, Arc::new(Image::from_bytes(ImageFormat::Png, bytes))));
                        if fits.len() > FITS_KEPT {
                            fits.remove(0);
                        }
                        this.plot_history.notes.remove(&key);
                    }
                    Ok(reply) => {
                        this.plot_history.cannot.insert(key.clone());
                        this.plot_history.notes.insert(
                            key.clone(),
                            format!(
                                "Recorded image: {}",
                                reply
                                    .reason
                                    .as_deref()
                                    .unwrap_or("this plot cannot be redrawn.")
                            ),
                        );
                    }
                    Err(err) => {
                        this.plot_history
                            .notes
                            .insert(key.clone(), format!("Recorded image: {err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The plot in front drawn again for the room it has, if it was.
    fn fitted_image(&self, item: &PlotItem) -> Option<Arc<Image>> {
        if self.plot_history.cannot.contains(&item.key)
            || self.plot_history.notes.contains_key(&item.key)
        {
            return None;
        }
        let room = self.plot_history.room.borrow();
        let prefix = format!("{}@", item.key);
        // The size asked for, or the newest one while the room changes.
        self.plot_history
            .fits
            .iter()
            .rev()
            .find(|(k, _)| *k == *room)
            .or_else(|| {
                self.plot_history
                    .fits
                    .iter()
                    .rev()
                    .find(|(k, _)| k.starts_with(&prefix))
            })
            .map(|(_, image)| image.clone())
    }

    fn clear_plots(&mut self, cx: &mut Context<Self>) {
        let (list, _) = self.plots_now();
        if let Some(last) = list.iter().map(|p| p.ts_ms).max() {
            self.plot_history.cleared = Some(last);
            self.plot_history.picked = None;
            self.plot_history.zoom = None;
        }
        cx.notify();
    }

    pub(crate) fn dismiss_plot_size_menu(&mut self) -> bool {
        std::mem::take(&mut self.plot_history.size_menu)
    }

    fn toggle_plot_size_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_plot_size_menu() {
            cx.notify();
            return;
        }
        let (width, height, dpi) = match self.plot_history.sizing {
            PlotSizing::Fixed { width, height, dpi } => (width, height, dpi),
            PlotSizing::Fit => (7., 5., DPI),
        };
        self.plot_history.size_inputs = Some((
            cx.new(|cx| InputState::new(window, cx).default_value(width.to_string())),
            cx.new(|cx| InputState::new(window, cx).default_value(height.to_string())),
            cx.new(|cx| InputState::new(window, cx).default_value(dpi.to_string())),
        ));
        self.plot_history.size_error = None;
        self.plot_history.size_menu = true;
        cx.notify();
    }

    fn set_plot_sizing(&mut self, sizing: PlotSizing, cx: &mut Context<Self>) {
        self.plot_history.sizing = sizing;
        self.plot_history.size_menu = false;
        self.plot_history.room.borrow_mut().clear();
        self.plot_history.fitting = None;
        self.plot_history
            .notes
            .retain(|key, _| self.plot_history.cannot.contains(key));
        if let Some(project) = &self.project {
            crate::prefs::save_plot_sizing(&self.host(), project, sizing);
        }
        cx.notify();
    }

    fn apply_custom_plot_size(&mut self, cx: &mut Context<Self>) {
        let Some((width, height, dpi)) = &self.plot_history.size_inputs else {
            return;
        };
        let parsed = width
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .zip(height.read(cx).value().trim().parse::<f64>().ok())
            .zip(dpi.read(cx).value().trim().parse::<u32>().ok())
            .map(|((width, height), dpi)| PlotSizing::Fixed { width, height, dpi });
        if let Some(sizing) = parsed.filter(|s| s.pixels().is_some()) {
            self.set_plot_sizing(sizing, cx);
        } else {
            self.plot_history.size_error = Some(
                "Enter positive inches and a whole-number DPI, at most 30,000 pixels per side."
                    .into(),
            );
            cx.notify();
        }
    }

    fn render_plot_size_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.plot_history.size_menu {
            return None;
        }
        let (width, height, dpi) = self.plot_history.size_inputs.as_ref()?;
        let p = *Palette::get(cx);
        let mut rows = v_flex().gap(px(4.));
        for (i, (label, sizing)) in [
            ("Fit to pane", PlotSizing::Fit),
            (
                "3.5 × 3 in",
                PlotSizing::Fixed {
                    width: 3.5,
                    height: 3.,
                    dpi: DPI,
                },
            ),
            (
                "7 × 5 in",
                PlotSizing::Fixed {
                    width: 7.,
                    height: 5.,
                    dpi: DPI,
                },
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if i == 1 {
                rows = rows.child(
                    div()
                        .px(px(8.))
                        .py(px(3.))
                        .text_color(p.faint)
                        .child("Fixed size"),
                );
            }
            rows = rows.child(
                h_flex()
                    .id(("plot-size-choice", i))
                    .h(px(27.))
                    .px(px(8.))
                    .gap(px(8.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(p.hover))
                    .child(
                        div()
                            .w(px(12.))
                            .child(if self.plot_history.sizing == sizing {
                                "✓"
                            } else {
                                ""
                            }),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_plot_sizing(sizing, cx))),
            );
        }
        let body = crate::composer_view::popup(&p)
            .id("plot-size-menu")
            .occlude()
            .w(px(310.))
            .text_size(px(13.))
            .child(rows)
            .child(
                div()
                    .px(px(8.))
                    .pt(px(8.))
                    .pb(px(4.))
                    .text_color(p.faint)
                    .child("Custom: width × height (in), DPI"),
            )
            .child(
                h_flex()
                    .px(px(8.))
                    .gap(px(6.))
                    .child(div().w(px(64.)).child(Input::new(width).small()))
                    .child("×")
                    .child(div().w(px(64.)).child(Input::new(height).small()))
                    .child(div().w(px(64.)).child(Input::new(dpi).small())),
            )
            .child(
                look::bar_button("plot-size-apply", "Apply custom size", None, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.apply_custom_plot_size(cx))),
            )
            .when_some(self.plot_history.size_error.clone(), |d, error| {
                d.child(div().px(px(8.)).text_color(p.err).child(error))
            })
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("plot-size", event.position) {
                    return;
                }
                this.dismiss_plot_size_menu();
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// The buttons over the Plots pane.
    pub(crate) fn plot_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let (list, at) = self.plots_now();
        let has = at.is_some();
        let back = at.is_some_and(|a| a > 0);
        let on = at.is_some_and(|a| a + 1 < list.len());
        let sizing = self.plot_history.sizing;
        // The tools share a narrow strip with the pane's tabs: keep it short.
        let size_label = match sizing {
            PlotSizing::Fit => "Fit".to_string(),
            PlotSizing::Fixed { width, height, .. } => format!("{width}×{height}"),
        };
        vec![
            look::bar_button("plot-size", size_label, Some("chevron.down"), &p)
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Plot size").build(w, cx)
                })
                .on_prepaint(self.popup_triggers.track("plot-size"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_plot_size_menu(window, cx)))
                .into_any_element(),
            icon_button("plot-back", "previous", &p, false)
                .when(!back, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.step_plot(-1, cx)))
                .into_any_element(),
            icon_button("plot-next", "next", &p, false)
                .when(!on, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.step_plot(1, cx)))
                .into_any_element(),
            icon_button("plot-zoom", "zoom", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| this.open_zoom(window, cx)))
                .into_any_element(),
            icon_button("plot-copy", "edit.copy", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Copy as PNG").build(w, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.copy_plot(cx)))
                .into_any_element(),
            icon_button("plot-save", "doc.save", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| this.open_zoom(window, cx)))
                .into_any_element(),
            icon_button("plot-clear", "delete", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.clear_plots(cx)))
                .into_any_element(),
        ]
    }

    /// The image of a plot, as cached; asks for it the first time.
    fn plot_image(&self, item: &PlotItem) -> Option<std::sync::Arc<Image>> {
        match self.plots.get(&plot_key(&item.value)) {
            Some(Plot::Ready(image)) => Some(image.clone()),
            _ => None,
        }
    }

    /// Asks for the images of the plots shown that aren't read yet.
    pub(crate) fn fetch_plot_images(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, Value)> = {
            let (list, _) = self.plots_now();
            list.iter()
                .filter(|p| !self.plots.contains_key(&plot_key(&p.value)))
                .map(|p| (p.mime.clone(), p.value.clone()))
                .collect()
        };
        for (mime, value) in wanted {
            self.fetch_image(&mime, &value, cx);
        }
    }

    /// The Plots pane's body: the plot in front, where it came from, and a
    /// strip of thumbnails.
    pub(crate) fn render_plots(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let (list, at) = self.plots_now();
        let Some(at) = at else {
            return v_flex().size_full().relative()
                .child(div().px(px(14.)).py(px(8.)).text_color(p.muted)
                    .child("Plots that R and Python draw show here: from the Console, scripts and agents. Notebooks' plots stay under their cells."))
                .children(self.render_plot_size_menu(cx))
                .into_any_element();
        };
        let item = list[at];
        let shown = self.fitted_image(item).or_else(|| self.plot_image(item));
        let main: AnyElement = match shown.clone() {
            Some(image) => img(image)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .text_color(p.faint)
                .child("Loading the plot…")
                .into_any_element(),
        };
        let thumbs = (list.len() > 1).then(|| {
            let hover = p.ink(8.);
            h_flex()
                .id("plot-thumbs")
                .flex_none()
                .h(px(62.))
                .gap(px(6.))
                .px(px(8.))
                .py(px(6.))
                .border_t(px(0.5))
                .border_color(p.glass_edge)
                .overflow_x_scroll()
                .children(list.iter().enumerate().map(|(ix, t)| {
                    let on = ix == at;
                    div()
                        .id(SharedString::from(format!("thumb-{ix}")))
                        .flex_none()
                        .w(px(64.))
                        .h(px(48.))
                        .p(px(2.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if on { p.accent } else { p.line })
                        .bg(p.bg)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .children(
                            self.plot_image(t)
                                .map(|i| img(i).size_full().object_fit(ObjectFit::Contain)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_plot(ix, cx)))
                }))
        });
        let when = ago(now_ms() - item.ts_ms);
        v_flex()
            .size_full()
            .relative()
            .child(
                div()
                    .id("plot-stage")
                    .flex_1()
                    .min_h_0()
                    .m(px(8.))
                    .relative()
                    .overflow_hidden()
                    .when_some(shown, |d, image| {
                        d.on_prepaint(self.track_plot_bounds(plot_key(&item.value), image))
                    })
                    .child(main)
                    .child(self.room_watcher(item.key.clone(), cx))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        if event.click_count() == 2 {
                            this.open_zoom(window, cx);
                        }
                    })),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(10.))
                    .pb(px(4.))
                    .gap(px(8.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(p.muted)
                            .child(source(item)),
                    )
                    .child(format!("{} of {} · {when}", at + 1, list.len())),
            )
            .when_some(self.plot_history.notes.get(&item.key), |d, note| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .pb(px(4.))
                        .text_size(px(13.))
                        .text_color(p.muted)
                        .child(note.clone()),
                )
            })
            .children(thumbs)
            .children(self.render_plot_size_menu(cx))
            .into_any_element()
    }

    /// Draws nothing, but sees the room the plot has each frame and, when
    /// it or the plot in front changed, asks for the plot to fit it.
    fn room_watcher(&self, plot: String, cx: &mut Context<Self>) -> impl IntoElement {
        let room = self.plot_history.room.clone();
        let sizing = self.plot_history.sizing;
        let this = cx.weak_entity();
        canvas(
            move |bounds, window, cx| {
                let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let key = size_key(&plot, sizing, width, height);
                if *room.borrow() == key {
                    return;
                }
                *room.borrow_mut() = key;
                let scale = window.scale_factor();
                let plot = plot.clone();
                let this = this.clone();
                cx.defer(move |cx| {
                    this.update(cx, |this, cx| this.fit_plot(plot, width, height, scale, cx))
                        .ok();
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The plots the large view steps through, and the one it shows.
    fn zoom_plots(&self) -> (Vec<&PlotItem>, Option<usize>) {
        match self
            .plot_history
            .zoom
            .as_ref()
            .and_then(|z| z.only.as_ref())
        {
            Some((items, at)) => (items.iter().collect(), Some(*at)),
            None => {
                let (mut items, _) = self.plots_now();
                let at = self.plot_history.zoom.as_ref().and_then(|zoom| {
                    items
                        .iter()
                        .position(|item| item.key == zoom.key)
                        .or_else(|| {
                            let item = zoom.retained.as_ref()?;
                            items.push(item);
                            Some(items.len() - 1)
                        })
                });
                (items, at)
            }
        }
    }

    /// The plots under notebook cell `ix`, as `PlotItem`s: of its latest
    /// run, else as saved in the file.
    fn cell_plots(&self, ix: usize) -> Vec<PlotItem> {
        let Some(open) = self.open.as_ref() else {
            return Vec::new();
        };
        let Some(cell) = open.notebook.cells.get(ix) else {
            return Vec::new();
        };
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id).map(|run| (id, run)));
        let (outputs, run_id, session) = match run {
            Some((id, run)) => (
                &run.outputs,
                id.clone(),
                open.session.id().unwrap_or_default().to_string(),
            ),
            None => (&cell.outputs, format!("saved-{}", cell.id), String::new()),
        };
        let place = Some((open.notebook.path.clone(), cell.id.clone()));
        outputs
            .iter()
            .filter(|o| {
                matches!(
                    o["output_type"].as_str(),
                    Some("display_data" | "execute_result")
                )
            })
            .filter_map(|o| {
                plot_item(
                    &run_id,
                    &session,
                    now_ms(),
                    false,
                    place.clone(),
                    None,
                    &o["data"],
                )
            })
            .collect()
    }

    /// Opens a plot under notebook cell `ix` large, to save it.
    pub(crate) fn open_cell_plot(
        &mut self,
        ix: usize,
        key: &str,
        bounds_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.cell_plots(ix);
        let Some(at) = items.iter().position(|p| plot_key(&p.value) == key) else {
            return;
        };
        let picture = self
            .plot_history
            .image_bounds
            .borrow()
            .get(bounds_key)
            .copied();
        self.open_zoom_on(Some((items, at)), picture, window, cx);
    }

    /// The large view's previous or next plot.
    fn step_zoom(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        match self.plot_history.zoom.as_mut().and_then(|z| z.only.take()) {
            Some((items, at)) => {
                let to = (at as isize + by).clamp(0, items.len() as isize - 1) as usize;
                self.open_zoom_on(Some((items, to)), None, window, cx);
            }
            None => {
                let (items, at) = self.zoom_plots();
                let Some(at) = at else { return };
                let to = (at as isize + by).clamp(0, items.len() as isize - 1) as usize;
                self.plot_history.picked = Some(items[to].key.clone());
                self.open_zoom(window, cx);
            }
        }
    }

    /// Opens the plot in front large, asking its session whether it can
    /// draw it again at any size.
    pub(crate) fn open_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_zoom_on(None, None, window, cx);
    }

    fn open_zoom_on(
        &mut self,
        only: Option<(Vec<PlotItem>, usize)>,
        source_picture: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (item, image, index, name) = {
            let (list, at) = match &only {
                Some((items, at)) => (items.iter().collect::<Vec<_>>(), Some(*at)),
                None => self.plots_now(),
            };
            let Some(at) = at else {
                return;
            };
            let item = list[at].clone();
            let (image, index) = redraw_target(&item, &list);
            let name = plot_name(&item, &list);
            (item, image, index, name)
        };
        let project = self.project.as_deref().unwrap_or_default();
        let folder = crate::prefs::plot_folder(&self.host(), project)
            .unwrap_or_else(|| format!("{}/{FOLDER}", project.trim_end_matches('/')));
        let format = plain_formats(&item.mime)[0];
        let (unit, dpi) = crate::prefs::plot_export(&self.host(), project);
        let picture = source_picture
            .or_else(|| {
                self.plot_history
                    .image_bounds
                    .borrow()
                    .get(&plot_key(&item.value))
                    .copied()
            })
            .unwrap_or(size(px(614.), px(461.)));
        let previous = self.plot_history.zoom.as_ref();
        let card_size = previous.and_then(|zoom| zoom.size).or(Some(size(
            (picture.width + px(28.)).max(px(720.)),
            picture.height + px(256.),
        )));
        let offset = previous.map_or(point(px(0.), px(0.)), |zoom| zoom.offset);
        let physical = match self.plot_history.sizing {
            PlotSizing::Fixed { width, height, .. } => (width, height),
            PlotSizing::Fit => window_inches(
                f64::from(f32::from(picture.width)),
                f64::from(f32::from(picture.height)),
            ),
        };
        let width = cx.new(|cx| {
            InputState::new(window, cx).default_value(dimension_text(physical.0, unit, dpi))
        });
        let height = cx.new(|cx| {
            InputState::new(window, cx).default_value(dimension_text(physical.1, unit, dpi))
        });
        let dpi_input = cx.new(|cx| InputState::new(window, cx).default_value(dpi.to_string()));
        let name = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{name}.{}", extension(format)))
        });
        let mut subscriptions = Vec::new();
        for input in [&width, &height] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.zoom_dimensions_changed(window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &dpi_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.zoom_dpi_changed(window, cx);
                }
            },
        ));
        self.plot_history.zoom = Some(Zoom {
            redraw: None,
            key: item.key.clone(),
            retained: only.is_none().then(|| item.clone()),
            preview: only.is_none().then(|| self.fitted_image(&item)).flatten(),
            preview_note: None,
            room: Rc::new(Cell::new(None)),
            fitting: None,
            size: card_size,
            resizing: None,
            offset,
            moving: None,
            initial_picture: Some(picture),
            follow: matches!(self.plot_history.sizing, PlotSizing::Fit),
            redraw_preview: true,
            unit,
            units_open: false,
            physical,
            folder,
            folder_dialog: None,
            _subscriptions: subscriptions,
            format,
            width,
            height,
            name,
            dpi,
            dpi_input,
            note: None,
            replace: None,
            working: false,
            only,
        });
        let Some(daemon) = self.daemon() else {
            if let Some(zoom) = &mut self.plot_history.zoom {
                zoom.redraw = Some(PlotRedraw {
                    can: false,
                    reason: Some("reconnect to redraw this plot".into()),
                    png: None,
                });
                zoom.redraw_preview = false;
            }
            cx.notify();
            return;
        };
        let asked = daemon.call::<PlotRedraw>(Call::RedrawPlot {
            session: item.session.clone(),
            run: item.run.clone(),
            image,
            index,
            file: None,
            format: String::new(),
            width: 0.,
            height: 0.,
            dpi: 0,
            show: false,
        });
        let room = self.plot_history.zoom.as_ref().unwrap().room.clone();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await.unwrap_or_else(|err| PlotRedraw {
                can: false,
                reason: Some(format!("{err:#}")),
                png: None,
            });
            this.update_in(cx, |this, window, cx| {
                let Some(zoom) = this.plot_history.zoom.as_mut() else {
                    return;
                };
                if !Rc::ptr_eq(&zoom.room, &room) {
                    return;
                }
                let can_redraw = answer.can;
                zoom.redraw_preview &= answer.can;
                zoom.redraw = Some(answer);
                if can_redraw {
                    this.set_zoom_format("png", window, cx);
                }
                this.fit_zoom(room, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn dismiss_zoom_units(&mut self) -> bool {
        self.plot_history
            .zoom
            .as_mut()
            .is_some_and(|zoom| std::mem::take(&mut zoom.units_open))
    }

    fn sync_zoom_dimensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let width = dimension_text(zoom.physical.0, zoom.unit, zoom.dpi);
        let height = dimension_text(zoom.physical.1, zoom.unit, zoom.dpi);
        if zoom.width.read(cx).value().as_str() != width {
            zoom.width
                .update(cx, |input, cx| input.set_value(width, window, cx));
        }
        if zoom.height.read(cx).value().as_str() != height {
            zoom.height
                .update(cx, |input, cx| input.set_value(height, window, cx));
        }
        cx.notify();
    }

    fn remember_zoom_sizing(&mut self, cx: &mut Context<Self>) {
        let Some(zoom) = &self.plot_history.zoom else {
            return;
        };
        let sizing = if zoom.follow {
            PlotSizing::Fit
        } else {
            PlotSizing::Fixed {
                width: zoom.physical.0,
                height: zoom.physical.1,
                dpi: zoom.dpi,
            }
        };
        if let Some(project) = &self.project {
            crate::prefs::save_plot_sizing(&self.host(), project, sizing);
            crate::prefs::save_plot_export(&self.host(), project, zoom.unit, zoom.dpi);
        }
        self.plot_history.sizing = sizing;
        cx.notify();
    }

    fn zoom_dimensions_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let width = zoom.width.read(cx).value().to_string();
        let height = zoom.height.read(cx).value().to_string();
        // Programmatic conversions and follow-window updates emit Change too.
        if width == dimension_text(zoom.physical.0, zoom.unit, zoom.dpi)
            && height == dimension_text(zoom.physical.1, zoom.unit, zoom.dpi)
        {
            return;
        }
        zoom.follow = false;
        if let Some((width, height)) = width
            .trim()
            .parse::<f64>()
            .ok()
            .zip(height.trim().parse::<f64>().ok())
        {
            let physical = (
                zoom.unit.inches(width, zoom.dpi),
                zoom.unit.inches(height, zoom.dpi),
            );
            if (PlotSizing::Fixed {
                width: physical.0,
                height: physical.1,
                dpi: zoom.dpi,
            })
            .pixels()
            .is_some()
            {
                zoom.physical = physical;
                self.remember_zoom_sizing(cx);
            }
        }
        cx.notify();
    }

    fn zoom_dpi_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let Ok(dpi) = zoom.dpi_input.read(cx).value().trim().parse::<u32>() else {
            return;
        };
        if dpi == 0 || dpi > 30000 || dpi == zoom.dpi {
            return;
        }
        zoom.dpi = dpi;
        self.sync_zoom_dimensions(window, cx);
        self.remember_zoom_sizing(cx);
    }

    fn choose_plot_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(zoom) = &self.plot_history.zoom else {
            return;
        };
        if self.host() == sandhill_link::LOCAL {
            // This Mac's folders: the system's own window.
            let chosen = crate::folder_dialog::choose_local_folder("Choose", cx);
            cx.spawn(async move |this, cx| {
                if let Some(folder) = chosen.await {
                    this.update(cx, |this, cx| this.set_plot_folder(folder, cx))
                        .ok();
                }
            })
            .detach();
            return;
        }
        let start = Some(zoom.folder.clone());
        let places = self
            .project
            .iter()
            .map(|project| crate::folder_dialog::Place {
                label: project
                    .rsplit('/')
                    .next()
                    .unwrap_or(project)
                    .to_string()
                    .into(),
                path: project.clone(),
            })
            .collect();
        let dialog = cx.new(|cx| {
            crate::folder_dialog::FolderDialog::new(
                daemon,
                "Choose a Folder for Plots".into(),
                "Choose".into(),
                start,
                places,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &crate::folder_dialog::FolderDialogDone, _, cx| {
                if let Some(zoom) = &mut this.plot_history.zoom {
                    zoom.folder_dialog = None;
                }
                match &event.0 {
                    Some(folder) => this.set_plot_folder(folder.clone(), cx),
                    None => cx.notify(),
                }
            },
        );
        if let Some(zoom) = &mut self.plot_history.zoom {
            zoom.folder_dialog = Some(dialog);
            zoom._subscriptions.push(subscription);
        }
        cx.notify();
    }

    fn set_plot_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        let host = self.host();
        if let Some(zoom) = &mut self.plot_history.zoom {
            zoom.folder = folder.clone();
            zoom.replace = None;
            if let Some(project) = &self.project {
                crate::prefs::save_plot_folder(&host, project, &folder);
            }
        }
        cx.notify();
    }

    /// One request at a time. Resizes during a render are coalesced, and an
    /// obsolete reply never replaces the plot in the current preview.
    fn fit_zoom(&mut self, room: Rc<Cell<Option<PreviewSize>>>, cx: &mut Context<Self>) {
        let (list, at) = self.zoom_plots();
        let Some(item) = at.map(|at| list[at]) else {
            return;
        };
        let (image, index) = redraw_target(item, &list);
        let (session, run) = (item.session.clone(), item.run.clone());
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        if !Rc::ptr_eq(&zoom.room, &room)
            || room.get().is_none()
            || zoom.fitting.is_some()
            || !zoom.redraw_preview
            // Until the window is resized the picture keeps its original
            // pixels, so it looks exactly as it did before zooming.
            || zoom.initial_picture.is_some()
            || !zoom.redraw.as_ref().is_some_and(|r| r.can)
        {
            return;
        }
        zoom.preview_note = Some("Waiting for the preview size to settle…".into());
        zoom.fitting = Some(cx.spawn(async move |this, cx| {
            loop {
                let Some(size) = room.get() else {
                    return;
                };
                cx.background_executor().timer(FIT_DELAY).await;
                if room.get() != Some(size) {
                    continue;
                }
                let current = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    let Some(zoom) = this.plot_history.zoom.as_mut() else {
                        return false;
                    };
                    if !Rc::ptr_eq(&zoom.room, &room) || !zoom.redraw_preview {
                        return false;
                    }
                    zoom.preview_note = Some("Redrawing preview…".into());
                    cx.notify();
                    true
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
                let answer = daemon
                    .call::<PlotRedraw>(Call::RedrawPlot {
                        session: session.clone(),
                        run: run.clone(),
                        image: image.clone(),
                        index,
                        file: None,
                        format: String::new(),
                        width: size.width / f64::from(POINTS),
                        height: size.height / f64::from(POINTS),
                        dpi: size.dpi,
                        show: true,
                    })
                    .await;
                let again = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        let Some(zoom) = this.plot_history.zoom.as_mut() else {
                            return false;
                        };
                        if !Rc::ptr_eq(&zoom.room, &room) || !zoom.redraw_preview {
                            return false;
                        }
                        if room.get() != Some(size) {
                            return true;
                        }
                        let preview = answer.and_then(|reply| {
                            if !reply.can {
                                zoom.redraw_preview = false;
                                zoom.redraw = Some(reply.clone());
                            }
                            let png = reply.png.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "{}",
                                    reply
                                        .reason
                                        .as_deref()
                                        .unwrap_or("no redrawn image was returned")
                                )
                            })?;
                            let bytes = sandhill_proto::decode_base64(png.trim())?;
                            Ok(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
                        });
                        match preview {
                            Ok(image) => {
                                zoom.preview = Some(image);
                                zoom.preview_note = None;
                            }
                            Err(err) => {
                                zoom.preview_note = Some(format!("Preview unchanged: {err:#}"))
                            }
                        }
                        zoom.fitting = None;
                        cx.notify();
                        false
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
        }));
    }

    fn zoom_room_watcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let room = self.plot_history.zoom.as_ref().unwrap().room.clone();
        let this = cx.weak_entity();
        let sizing = PlotSizing::Fit;
        let resize_this = this.clone();
        canvas(
            move |bounds, window, cx| {
                let size = PreviewSize {
                    width: f64::from(f32::from(bounds.size.width)),
                    height: f64::from(f32::from(bounds.size.height)),
                    dpi: (POINTS * window.scale_factor().clamp(1., 4.)).round() as u32,
                };
                if (matches!(sizing, PlotSizing::Fit)
                    && (size.width < f64::from(MIN_FIT) || size.height < f64::from(MIN_FIT)))
                    || !preview_size_changed(room.get(), size, sizing)
                {
                    return;
                }
                room.set(Some(size));
                let (this, room) = (this.clone(), room.clone());
                window.defer(cx, move |window, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(zoom) = &mut this.plot_history.zoom
                            && Rc::ptr_eq(&zoom.room, &room)
                            && zoom.follow
                        {
                            zoom.physical = window_inches(size.width, size.height);
                            this.sync_zoom_dimensions(window, cx);
                        }
                        this.fit_zoom(room, cx);
                        cx.notify();
                    })
                    .ok();
                });
            },
            move |_, _, window, _| {
                let this = resize_this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    this.update(cx, |this, cx| {
                        let Some(zoom) = this.plot_history.zoom.as_mut() else {
                            return;
                        };
                        let viewport = window.viewport_size();
                        if let Some((start, original)) = zoom.moving {
                            if !event.dragging() {
                                zoom.moving = None;
                                return;
                            }
                            let card = zoom.size.unwrap_or(size(px(720.), px(600.)));
                            zoom.offset = clamp_zoom_offset(
                                original + (event.position - start),
                                card,
                                viewport,
                            );
                        } else if let Some((start, original)) = zoom.resizing {
                            if !event.dragging() {
                                zoom.resizing = None;
                                return;
                            }
                            let max = size(viewport.width * 0.96, viewport.height * 0.96);
                            let delta = event.position - start;
                            // The card remains centred on its translated centre. Each
                            // edge moves half the size delta, hence the factor of two.
                            let resized = size(
                                (original.width + delta.x * 2.)
                                    .clamp(px(720.).min(max.width), max.width),
                                (original.height + delta.y * 2.)
                                    .clamp(px(340.).min(max.height), max.height),
                            );
                            zoom.size = Some(resized);
                            zoom.initial_picture = None;
                            zoom.offset = clamp_zoom_offset(zoom.offset, resized, viewport);
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })
                    .ok();
                });
                let this = resize_this.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        this.update(cx, |this, cx| {
                            if let Some(zoom) = this.plot_history.zoom.as_mut() {
                                let resized = zoom.resizing.take().is_some();
                                let moved = zoom.moving.take().is_some();
                                if resized || moved {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    fn set_zoom_format(
        &mut self,
        format: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        zoom.format = format;
        zoom.replace = None;
        // The typed name takes the new extension.
        let typed = zoom.name.read(cx).value().to_string();
        let stem = match typed.rsplit_once('.') {
            Some((stem, ext)) if !ext.contains('/') => stem.to_string(),
            _ => typed,
        };
        let renamed = format!("{stem}.{}", extension(format));
        zoom.name
            .update(cx, |i, cx| i.set_value(renamed, window, cx));
        cx.notify();
    }

    /// Saves the plot in the project's folder: drawn again by its session
    /// at the size asked when it can, else as it was drawn.
    fn save_plot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (list, at) = self.zoom_plots();
        let (Some(at), Some(daemon), Some(project)) = (at, self.daemon(), self.project.clone())
        else {
            return;
        };
        let item = list[at].clone();
        let (image, index) = redraw_target(&item, &list);
        let host = self.host();
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        let typed = zoom.name.read(cx).value().trim().to_string();
        if typed.is_empty() || typed.contains('/') {
            zoom.note = Some("Enter a file name, and choose its folder separately.".into());
            cx.notify();
            return;
        }
        let folder = zoom.folder.clone();
        let mut path = format!("{}/{typed}", folder.trim_end_matches('/'));
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let original_format = plain_formats(&item.mime)[0];
        let format = zoom.format;
        if !redrawn {
            path = std::path::Path::new(&path)
                .with_extension(extension(original_format))
                .to_string_lossy()
                .into_owned();
        }
        let parsed = zoom
            .width
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .zip(zoom.height.read(cx).value().trim().parse::<f64>().ok())
            .zip(zoom.dpi_input.read(cx).value().trim().parse::<u32>().ok());
        let Some(((width, height), dpi)) = parsed else {
            zoom.note = Some("Enter positive dimensions and a whole-number dpi.".into());
            cx.notify();
            return;
        };
        let physical = (zoom.unit.inches(width, dpi), zoom.unit.inches(height, dpi));
        if (PlotSizing::Fixed {
            width: physical.0,
            height: physical.1,
            dpi,
        })
        .pixels()
        .is_none()
        {
            zoom.note = Some("Enter positive dimensions, at most 30,000 pixels per side.".into());
            cx.notify();
            return;
        }
        // Display rounding (especially after switching units) must never alter
        // the canonical physical size used for export.
        let physical = zoom.physical;
        let (width, height) = (physical.0 * f64::from(dpi), physical.1 * f64::from(dpi));
        let room = zoom.room.clone();
        let replace = zoom.replace.clone();
        zoom.working = true;
        zoom.note = None;
        crate::prefs::save_plot_folder(&host, &project, &folder);
        let stat = daemon.call::<Vec<FileStat>>(Call::StatFiles {
            paths: vec![path.clone()],
        });
        let blob = item.value[sandhill_proto::BLOB_KEY]
            .as_str()
            .map(str::to_string);
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let mut unavailable = None;
            let outcome: anyhow::Result<Option<String>> = async {
                let exists = stat.await?.first().is_some_and(|s| s.mtime_ms.is_some());
                if exists && replace.as_deref() != Some(path.as_str()) {
                    return Ok(None);
                }
                let folder_stat = daemon
                    .call::<Vec<FileStat>>(Call::StatFiles {
                        paths: vec![folder.clone()],
                    })
                    .await?;
                if !folder_stat
                    .first()
                    .is_some_and(|stat| stat.mtime_ms.is_some())
                {
                    daemon.call::<Value>(Call::MakeDir { path: folder }).await?;
                }
                if redrawn {
                    let reply = daemon
                        .call::<PlotRedraw>(Call::RedrawPlot {
                            session: item.session.clone(),
                            run: item.run.clone(),
                            image,
                            index,
                            file: Some(path.clone()),
                            format: format.to_string(),
                            width: width / f64::from(dpi),
                            height: height / f64::from(dpi),
                            dpi,
                            show: false,
                        })
                        .await?;
                    if reply.can {
                        let size = if format == "png" {
                            format!("{width:.0} × {height:.0} px")
                        } else {
                            format!(
                                "{:.1} × {:.1} in",
                                width / f64::from(dpi),
                                height / f64::from(dpi)
                            )
                        };
                        return Ok(Some(size));
                    }
                    unavailable = Some(reply);
                    let original_path = std::path::Path::new(&path)
                        .with_extension(extension(original_format))
                        .to_string_lossy()
                        .into_owned();
                    if original_path != path {
                        path = original_path;
                        let exists = daemon
                            .call::<Vec<FileStat>>(Call::StatFiles {
                                paths: vec![path.clone()],
                            })
                            .await?
                            .first()
                            .is_some_and(|stat| stat.mtime_ms.is_some());
                        if exists && replace.as_deref() != Some(path.as_str()) {
                            return Ok(None);
                        }
                    }
                }
                // As drawn: the image's own text (base64 for a PNG).
                let text = match (&item.value, blob) {
                    (Value::String(text), _) => text.clone(),
                    (_, Some(hash)) => {
                        daemon
                            .call::<sandhill_proto::Blob>(Call::GetBlob { hash, near })
                            .await?
                            .data
                    }
                    _ => anyhow::bail!("the plot has no image"),
                };
                let base64 = item.mime != "image/svg+xml";
                daemon
                    .call::<Value>(Call::WriteFile {
                        path: path.clone(),
                        text,
                        expected_mtime_ms: None,
                        base64,
                        create: false,
                    })
                    .await?;
                Ok(Some("as drawn".into()))
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                let shown = this.relative_path(&path);
                if let Some(zoom) = this.plot_history.zoom.as_mut() {
                    if !Rc::ptr_eq(&zoom.room, &room) {
                        return;
                    }
                    zoom.working = false;
                    let used_original = !redrawn || unavailable.is_some();
                    if let Some(unavailable) = unavailable {
                        zoom.redraw = Some(unavailable);
                        zoom.redraw_preview = false;
                        zoom.fitting = None;
                        zoom.preview_note = None;
                    }
                    if used_original {
                        zoom.format = original_format;
                        if let Some(name) = std::path::Path::new(&path).file_name() {
                            zoom.name.update(cx, |input, cx| {
                                input.set_value(name.to_string_lossy().into_owned(), window, cx)
                            });
                        }
                    }
                    match outcome {
                        Ok(Some(size)) => {
                            zoom.replace = None;
                            zoom.note = Some(format!("Saved {shown}, {size}."));
                        }
                        Ok(None) => {
                            zoom.replace = Some(path.clone());
                            zoom.note = Some(format!(
                                "{shown} is already there. Save again to replace it."
                            ));
                        }
                        Err(err) => zoom.note = Some(format!("Could not save the plot: {err:#}")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A path shown relative to the project.
    fn relative_path(&self, path: &str) -> String {
        match &self.project {
            Some(project) => path
                .strip_prefix(project.trim_end_matches('/'))
                .map_or(path, |rest| rest.trim_start_matches('/'))
                .to_string(),
            None => path.to_string(),
        }
    }

    /// The large view over the window, when open.
    pub(crate) fn render_zoom(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let zoom = self.plot_history.zoom.as_ref()?;
        let p = *Palette::get(cx);
        let (list, at) = self.zoom_plots();
        let item = list.get(at?)?;
        let at = at?;
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let formats: &[&'static str] = if redrawn {
            &["png", "svg", "pdf"]
        } else {
            plain_formats(&item.mime)
        };
        let explain = match &zoom.redraw {
            None => "Checking whether this plot can be redrawn…".to_string(),
            Some(r) if r.can => zoom.preview_note.clone().unwrap_or_else(|| {
                if zoom.redraw_preview {
                    "Drag the title to move. Drag the corner to resize.".into()
                } else {
                    "Preview is scaled only. Save still redraws at the export size.".into()
                }
            }),
            Some(r) => match &r.reason {
                Some(why) => format!("Saved as drawn: {why}"),
                None => "Saved as drawn: its session can't draw it again.".into(),
            },
        };
        let image: AnyElement = match zoom.preview.clone().or_else(|| self.plot_image(item)) {
            Some(i) => img(i)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div().child("Loading the plot…").into_any_element(),
        };
        let segment = |f: &'static str, cx: &mut Context<Self>| {
            let on = zoom.format == f;
            div()
                .id(SharedString::from(format!("fmt-{f}")))
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(p.bg).text_color(p.text))
                .child(f.to_uppercase())
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_zoom_format(f, window, cx)),
                )
        };
        let reason = zoom
            .redraw
            .as_ref()
            .and_then(|r| r.reason.clone())
            .unwrap_or_else(|| {
                if redrawn {
                    "Redraw the preview through its live session".into()
                } else {
                    explain.clone()
                }
            });
        let mut units = v_flex().gap(px(2.));
        for unit in [PlotUnit::Pixels, PlotUnit::Inches, PlotUnit::Centimetres] {
            units = units.child(
                look::bar_button(
                    SharedString::from(format!("zoom-unit-{}", unit.label())),
                    unit.label(),
                    None,
                    &p,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(zoom) = &mut this.plot_history.zoom {
                        zoom.unit = unit;
                        zoom.units_open = false;
                    }
                    this.sync_zoom_dimensions(window, cx);
                    this.remember_zoom_sizing(cx);
                })),
            );
        }
        let unit_picker = div()
            .relative()
            .child(
                look::bar_button("zoom-unit", zoom.unit.label(), Some("chevron.down"), &p)
                    .on_prepaint(self.popup_triggers.track("zoom-unit"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(zoom) = &mut this.plot_history.zoom {
                            zoom.units_open = !zoom.units_open;
                        }
                        cx.notify();
                    })),
            )
            .when(zoom.units_open, |d| {
                d.child(
                    div().absolute().top(px(28.)).left_0().child(
                        deferred(
                            crate::composer_view::popup(&p)
                                .id("zoom-units")
                                .occlude()
                                .w(px(78.))
                                .text_size(px(look::SMALL))
                                .child(units)
                                .on_mouse_down_out(cx.listener(
                                    |this, event: &MouseDownEvent, _, cx| {
                                        if this.popup_triggers.contains("zoom-unit", event.position)
                                        {
                                            return;
                                        }
                                        if let Some(zoom) = &mut this.plot_history.zoom {
                                            zoom.units_open = false;
                                        }
                                        cx.notify();
                                    },
                                )),
                        )
                        .with_priority(3),
                    ),
                )
            });
        let back = at > 0;
        let on = at + 1 < list.len();
        let card = v_flex()
            .id("plot-zoom")
            .occlude()
            .relative()
            .left(zoom.offset.x)
            .top(zoom.offset.y)
            .text_size(px(look::SMALL))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .map(|d| match zoom.size {
                Some(size) => d.w(size.width).h(size.height),
                None => d.w(relative(0.86)).h(relative(0.86)),
            })
            .max_w(relative(0.96))
            .max_h(relative(0.96))
            .bg(p.bg)
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.glass_edge)
            .shadow(vec![BoxShadow {
                color: p.shadow.opacity(0.35),
                offset: point(px(0.), px(12.)),
                blur_radius: px(36.),
                spread_radius: px(0.),
                inset: false,
            }])
            .child(
                h_flex()
                    .flex_none()
                    .h(px(36.))
                    .px(px(8.))
                    .gap(px(4.))
                    .border_b(px(0.5))
                    .border_color(p.glass_edge)
                    .child(
                        div()
                            .id("zoom-title-drag")
                            .cursor(CursorStyle::ClosedHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    if let Some(zoom) = &mut this.plot_history.zoom {
                                        zoom.moving = Some((event.position, zoom.offset));
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .flex_1()
                            .min_w_0()
                            .pl(px(6.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!(
                                "{} · {} of {} · {}",
                                source(item),
                                at + 1,
                                list.len(),
                                ago(now_ms() - item.ts_ms)
                            )),
                    )
                    .child(
                        icon_button("zoom-back", "previous", &p, false)
                            .when(!back, |d| d.opacity(0.35))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step_zoom(-1, window, cx)),
                            ),
                    )
                    .child(
                        icon_button("zoom-next", "next", &p, false)
                            .when(!on, |d| d.opacity(0.35))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step_zoom(1, window, cx)),
                            ),
                    )
                    .child(
                        icon_button("zoom-close", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.plot_history.zoom = None;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .m(px(14.))
                    .relative()
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .map(|d| match zoom.initial_picture {
                                Some(picture) => d
                                    .w(picture.width)
                                    .h(picture.height)
                                    .max_w(relative(1.))
                                    .max_h(relative(1.)),
                                None => d.size_full(),
                            })
                            .child(image)
                            .child(self.zoom_room_watcher(cx)),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .min_w_0()
                    .h(px(190.))
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_t(px(0.5))
                    .border_color(p.glass_edge)
                    .text_size(px(look::SMALL))
                    .child(
                        h_flex().gap(px(10.)).h(px(26.)).flex_none().child(
                            h_flex()
                                .gap(px(2.))
                                .p(px(2.))
                                .rounded(px(8.))
                                .bg(p.ink(6.))
                                .text_color(p.muted)
                                .children(formats.iter().map(|f| segment(f, cx))),
                        ),
                    )
                    .child(
                        h_flex()
                            .h(px(28.))
                            .flex_none()
                            .gap(px(8.))
                            .child("Width")
                            .child(div().w(px(86.)).child(Input::new(&zoom.width).small()))
                            .child("Height")
                            .child(div().w(px(86.)).child(Input::new(&zoom.height).small()))
                            .child(unit_picker)
                            .child("at")
                            .child(div().w(px(68.)).child(Input::new(&zoom.dpi_input).small()))
                            .child("dpi"),
                    )
                    .child(
                        h_flex()
                            .h(px(22.))
                            .flex_none()
                            .gap(px(8.))
                            .child(
                                Switch::new("zoom-follow")
                                    .small()
                                    .color(p.accent)
                                    .checked(zoom.follow)
                                    .accessibility_label("Size follows window")
                                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                                        if let Some(zoom) = &mut this.plot_history.zoom {
                                            zoom.follow = *on;
                                            if *on && let Some(room) = zoom.room.get() {
                                                zoom.physical =
                                                    window_inches(room.width, room.height);
                                            }
                                        }
                                        this.sync_zoom_dimensions(window, cx);
                                        this.remember_zoom_sizing(cx);
                                    })),
                            )
                            .child("Size follows window")
                            .child(div().w(px(12.)))
                            .child(
                                div()
                                    .id("zoom-redraw-tip")
                                    .child(
                                        Switch::new("zoom-redraw")
                                            .small()
                                            .color(p.accent)
                                            .checked(redrawn && zoom.redraw_preview)
                                            .disabled(!redrawn)
                                            .accessibility_label("Redraw")
                                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                                let Some(zoom) = &mut this.plot_history.zoom else {
                                                    return;
                                                };
                                                zoom.redraw_preview = *on;
                                                zoom.fitting = None;
                                                zoom.preview_note = None;
                                                let room = zoom.room.clone();
                                                if *on {
                                                    this.fit_zoom(room, cx);
                                                }
                                                cx.notify();
                                            })),
                                    )
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(reason.clone())
                                            .build(window, cx)
                                    }),
                            )
                            .child("Redraw"),
                    )
                    // Status changes must not change the footer's height:
                    // that would resize the canvas and trigger another redraw.
                    .child(
                        div()
                            .id("plot-zoom-status")
                            .h(px(18.))
                            .flex_none()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(p.faint)
                            .child(zoom.note.clone().unwrap_or_else(|| explain.clone()))
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(explain.clone())
                                    .build(window, cx)
                            }),
                    )
                    .child(
                        h_flex()
                            .h(px(28.))
                            .flex_none()
                            .gap(px(8.))
                            .child("Folder")
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .child(self.relative_path(&zoom.folder)),
                            )
                            .child(
                                look::bar_button("zoom-folder", "Choose…", Some("folder"), &p)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.choose_plot_folder(window, cx)
                                    })),
                            )
                            .child("Name")
                            .child(div().w(px(150.)).child(Input::new(&zoom.name).small()))
                            .child(
                                div()
                                    .id("zoom-save")
                                    .h(px(28.))
                                    .px(px(14.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(RADIUS))
                                    .bg(p.accent)
                                    .text_color(p.primary_text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .child(if zoom.replace.is_some() {
                                        "Replace"
                                    } else {
                                        "Save"
                                    })
                                    .when(zoom.working || zoom.redraw.is_none(), |d| d.opacity(0.5))
                                    .when(!zoom.working && zoom.redraw.is_some(), |d| {
                                        d.on_click(cx.listener(|this, _, window, cx| {
                                            this.save_plot(window, cx)
                                        }))
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("plot-zoom-resize")
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(20.))
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .text_color(p.muted)
                    .child("◢")
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "Drag to resize the plot preview",
                        )
                        .build(window, cx)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            let viewport = window.viewport_size();
                            if let Some(zoom) = this.plot_history.zoom.as_mut() {
                                let current = zoom
                                    .size
                                    .unwrap_or(size(viewport.width * 0.86, viewport.height * 0.86));
                                let current = size(
                                    current.width.min(viewport.width * 0.96),
                                    current.height.min(viewport.height * 0.96),
                                );
                                zoom.resizing = Some((event.position, current));
                            }
                            cx.stop_propagation();
                        }),
                    ),
            );
        let folder_dialog = zoom.folder_dialog.clone();
        Some(
            deferred(
                div()
                    .id("plot-zoom-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .children(folder_dialog)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.plot_history.zoom = None;
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        History, PlotItem, PreviewSize, clamp_zoom_offset, from_notebook, plain_formats, plot_item,
        plot_name, preview_size_changed, redraw_target, source, window_inches,
    };
    use crate::prefs::PlotUnit;
    use gpui_kit::{point, px, size};
    use serde_json::json;

    #[test]
    fn export_units_preserve_physical_size_and_dpi_only_changes_pixels() {
        for unit in [PlotUnit::Pixels, PlotUnit::Inches, PlotUnit::Centimetres] {
            let value = unit.display_value(6.4, 300);
            assert!((unit.inches(value, 300) - 6.4).abs() < 1e-10);
        }
        assert_eq!(PlotUnit::Pixels.display_value(6.4, 300), 1920.);
        assert_eq!(PlotUnit::Pixels.display_value(6.4, 150), 960.);
        assert_eq!(PlotUnit::Inches.display_value(6.4, 150), 6.4);
        assert!((PlotUnit::Centimetres.display_value(1., 300) - 2.54).abs() < 1e-10);
    }

    #[test]
    fn follow_window_uses_screen_points_not_backing_pixels() {
        let (width, height) = window_inches(614.4, 460.8);
        assert!((width - 6.4).abs() < 1e-10);
        assert!((height - 4.8).abs() < 1e-10);
        assert!((PlotUnit::Pixels.display_value(width, 300) - 1920.).abs() < 1e-10);
    }

    #[test]
    fn dragging_keeps_title_reachable_on_all_edges() {
        let card = size(px(720.), px(600.));
        let viewport = size(px(1200.), px(900.));
        let low = clamp_zoom_offset(point(px(-2000.), px(-2000.)), card, viewport);
        let high = clamp_zoom_offset(point(px(2000.), px(2000.)), card, viewport);
        assert_eq!(low, point(px(-864.), px(-150.)));
        assert_eq!(high, point(px(864.), px(714.)));
        let inside = point(px(30.), px(45.));
        assert_eq!(clamp_zoom_offset(inside, card, viewport), inside);
    }

    fn item(run: &str, ts: i64, exact: bool) -> PlotItem {
        plot_item(
            run,
            "s1",
            ts,
            exact,
            None,
            None,
            &json!({"image/png": format!("{run}-png")}),
        )
        .unwrap()
    }

    #[test]
    fn keeps_plots_in_time_order_and_takes_the_journal_time() {
        let mut h = History::default();
        assert!(h.add(vec![item("r2", 20, false), item("r1", 10, true)]));
        assert_eq!(
            h.items.iter().map(|p| p.run.as_str()).collect::<Vec<_>>(),
            ["r1", "r2"]
        );
        // The journal's time replaces the run's start; a guess never replaces it.
        assert!(!h.add(vec![item("r2", 25, true)]));
        assert_eq!(h.items[1].ts_ms, 25);
        h.add(vec![item("r2", 30, false)]);
        assert_eq!(h.items[1].ts_ms, 25);
        h.cleared = Some(10);
        assert_eq!(h.shown().len(), 1); // Ended sessions retain their recorded plots.
    }

    #[test]
    fn names_plots_by_file_and_number() {
        let mut a = item("r1", 1, true);
        a.cell = Some(("/p/scripts/qc plots.R".into(), "x".into()));
        let mut b = plot_item(
            "r1",
            "s1",
            2,
            true,
            None,
            None,
            &json!({"image/svg+xml": "<svg/>"}),
        )
        .unwrap();
        b.cell = a.cell.clone();
        let all = vec![&a, &b];
        assert_eq!(plot_name(&a, &all), "qc-plots-1");
        assert_eq!(plot_name(&b, &all), "qc-plots-2");
        assert_eq!(redraw_target(&b, &all).1, 1);
        assert_eq!(plain_formats(&b.mime), ["svg"]);
        assert_eq!(source(&b), "qc plots.R");
        assert!(from_notebook(&Some(("/p/a.ipynb".into(), "c".into()))));
        assert!(!from_notebook(&Some(("/p/a.py".into(), "c".into()))));
    }

    #[test]
    fn preview_resize_ignores_rounding_noise_but_keeps_real_changes() {
        let fit = crate::prefs::PlotSizing::Fit;
        let accepted = PreviewSize {
            width: 800.,
            height: 500.,
            dpi: 192,
        };
        assert!(preview_size_changed(None, accepted, fit));
        let mut room = Some(accepted);
        for (width, height) in [(800., 500.), (801., 499.), (799., 501.), (800., 500.)] {
            let measured = PreviewSize {
                width,
                height,
                ..accepted
            };
            assert!(!preview_size_changed(room, measured, fit));
            // Mirror the watcher: ignored noise never moves the anchor.
            if preview_size_changed(room, measured, fit) {
                room = Some(measured);
            }
        }
        assert!(preview_size_changed(
            room,
            PreviewSize {
                width: 802.,
                ..accepted
            },
            fit,
        ));
        assert!(preview_size_changed(
            room,
            PreviewSize {
                height: 502.,
                ..accepted
            },
            fit,
        ));
        assert!(preview_size_changed(
            room,
            PreviewSize {
                dpi: 96,
                ..accepted
            },
            fit,
        ));
    }

    #[test]
    fn fixed_preview_sizes_keep_subpixel_physical_changes() {
        let fixed = crate::prefs::PlotSizing::Fixed {
            width: 3.5,
            height: 3.,
            dpi: 300,
        };
        let previous = PreviewSize {
            width: 336.,
            height: 288.,
            dpi: 300,
        };
        assert!(!preview_size_changed(Some(previous), previous, fixed));
        assert!(preview_size_changed(
            Some(previous),
            PreviewSize {
                width: 336.5,
                ..previous
            },
            fixed,
        ));
    }

    #[test]
    fn fixed_requests_ignore_pane_resizing_and_keep_physical_dimensions() {
        let fixed = crate::prefs::PlotSizing::Fixed {
            width: 3.5,
            height: 3.,
            dpi: 300,
        };
        assert_eq!(super::redraw_size(fixed, 200., 100., 2.), (3.5, 3., 300));
        assert_eq!(super::redraw_size(fixed, 800., 500., 1.), (3.5, 3., 300));
        assert_eq!(
            super::size_key("plot", fixed, 200., 100.),
            super::size_key("plot", fixed, 800., 500.)
        );
    }
}
