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

mod pane;
mod save;
mod size_menu;
mod zoom;
mod zoom_view;

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
    /// Save a PDF as pixels at `dpi` (Huan, 2026-11-14).
    rasterize: bool,
    /// The settings column beside the preview is shown.
    settings: bool,
    physical: (f64, f64),
    folder: String,
    folder_dialog: Option<Entity<crate::welcome::folder_dialog::FolderDialog>>,
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
            rasterize: false,
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
