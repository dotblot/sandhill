//! The Plots pane, as the Tauri app has it (`PlotsPane.tsx`,
//! `model/plots.ts`): every plot the project's sessions drew outside
//! notebooks (the Console, scripts, agents), newest last, read from the
//! daemon's journal and added to as runs draw more. The arrows and
//! thumbnails go back through them; the magnifier opens one large, to save
//! it as a file.

use std::collections::HashSet;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, FileStat, PlotRedraw, SessionPlots};
use serde_json::Value;

use crate::look::{self, Palette, RADIUS, icon_button};
use crate::workbench::{BottomTab, Plot, Workbench, is_script, plot_key};

/// The image types shown, best first: an SVG can be saved at any size.
const MIMES: [&str; 4] = ["image/svg+xml", "image/png", "image/jpeg", "image/gif"];
/// Plots read from each session's journal when it is first seen.
const LOAD: u32 = 100;
/// How many plots the pane keeps, all sessions together.
const MAX_PLOTS: usize = 300;
/// Where plots are saved unless the user typed another folder last.
const FOLDER: &str = "figures";
/// Pixels in an inch for saved PNGs.
const DPI: u32 = 300;

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
    pub format: &'static str,
    pub width: Entity<InputState>,
    pub height: Entity<InputState>,
    pub name: Entity<InputState>,
    /// What happened last: saved, or why not.
    pub note: Option<String>,
    /// A path that is there already; saving again replaces it.
    pub replace: Option<String>,
    pub working: bool,
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
    /// The folder plots were saved in last.
    folder: Option<String>,
}

impl History {
    /// The plots shown: of live sessions, after a clear.
    fn shown(&self, live: &[String]) -> Vec<&PlotItem> {
        self.items
            .iter()
            .filter(|p| live.contains(&p.session))
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
fn from_notebook(cell: &Option<(String, String)>) -> bool {
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
    /// The sessions whose plots show: every kernel open in this window.
    fn live_sessions(&self) -> Vec<String> {
        self.open
            .iter()
            .chain(&self.others)
            .filter_map(|o| o.session.id())
            .chain(self.loose.id())
            .map(str::to_string)
            .collect()
    }

    /// Reads the plots of sessions not read yet. Called as the window draws.
    pub(crate) fn sync_plots(&mut self, cx: &mut Context<Self>) {
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
        let read = daemon.call::<SessionPlots>(Call::SessionPlots {
            session: session.clone(),
            limit: Some(limit),
        });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| match read {
                Ok(read) => {
                    let items = read
                        .plots
                        .iter()
                        .filter_map(|p| {
                            let cell = p.cell.clone().map(|c| (c.file, c.cell_id));
                            if from_notebook(&cell) {
                                return None;
                            }
                            plot_item(&p.run, &session, p.ts_ms, true, cell, p.by.clone(), &p.data)
                        })
                        .collect();
                    this.plot_history.add(items);
                    cx.notify();
                }
                Err(_) => {
                    this.plot_history.loaded.remove(&session);
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
        if run.quiet || from_notebook(&run.cell) {
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
            .collect();
        if items.is_empty() {
            return;
        }
        self.plot_history.drawing.insert(run_id.to_string());
        if self.plot_history.add(items) {
            self.plot_history.picked = None;
            self.bottom_tab = BottomTab::Plots;
        }
        cx.notify();
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
        let list = self.plot_history.shown(&self.live_sessions());
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

    fn clear_plots(&mut self, cx: &mut Context<Self>) {
        let (list, _) = self.plots_now();
        if let Some(last) = list.iter().map(|p| p.ts_ms).max() {
            self.plot_history.cleared = Some(last);
            self.plot_history.picked = None;
            self.plot_history.zoom = None;
        }
        cx.notify();
    }

    /// The buttons over the Plots pane.
    pub(crate) fn plot_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let (list, at) = self.plots_now();
        let has = at.is_some();
        let back = at.is_some_and(|a| a > 0);
        let on = at.is_some_and(|a| a + 1 < list.len());
        vec![
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
            return div()
                .px(px(14.))
                .py(px(8.))
                .text_color(p.muted)
                .child("Plots that R and Python draw show here: from the Console, scripts and agents. Notebooks' plots stay under their cells.")
                .into_any_element();
        };
        let item = list[at];
        let main: AnyElement = match self.plot_image(item) {
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
            .child(
                div()
                    .id("plot-stage")
                    .flex_1()
                    .min_h_0()
                    .m(px(8.))
                    .relative()
                    .overflow_hidden()
                    .child(main)
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
            .children(thumbs)
            .into_any_element()
    }

    /// Opens the plot in front large, asking its session whether it can
    /// draw it again at any size.
    pub(crate) fn open_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (list, at) = self.plots_now();
        let Some(at) = at else {
            return;
        };
        let item = list[at].clone();
        let (image, index) = redraw_target(&item, &list);
        let folder = self
            .plot_history
            .folder
            .clone()
            .unwrap_or_else(|| FOLDER.to_string());
        let name = plot_name(&item, &list);
        let format = plain_formats(&item.mime)[0];
        let path = if folder.is_empty() {
            format!("{name}.{}", extension(format))
        } else {
            format!("{folder}/{name}.{}", extension(format))
        };
        let width = cx.new(|cx| InputState::new(window, cx).default_value("1800"));
        let height = cx.new(|cx| InputState::new(window, cx).default_value("1200"));
        let name = cx.new(|cx| InputState::new(window, cx).default_value(path));
        self.plot_history.zoom = Some(Zoom {
            redraw: None,
            format,
            width,
            height,
            name,
            note: None,
            replace: None,
            working: false,
        });
        let Some(daemon) = self.daemon() else {
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
        let key = item.key.clone();
        cx.spawn(async move |this, cx| {
            let answer = asked.await.unwrap_or(PlotRedraw {
                can: false,
                reason: None,
                png: None,
            });
            this.update(cx, |this, cx| {
                let (list, at) = this.plots_now();
                if at.map(|a| list[a].key.as_str()) != Some(key.as_str()) {
                    return;
                }
                if let Some(zoom) = this.plot_history.zoom.as_mut() {
                    if answer.can {
                        zoom.format = "png";
                    }
                    zoom.redraw = Some(answer);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
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
    fn save_plot(&mut self, cx: &mut Context<Self>) {
        let (list, at) = self.plots_now();
        let (Some(at), Some(daemon), Some(project)) = (at, self.daemon(), self.project.clone())
        else {
            return;
        };
        let item = list[at].clone();
        let (image, index) = redraw_target(&item, &list);
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        let typed = zoom.name.read(cx).value().trim().to_string();
        if typed.is_empty() {
            return;
        }
        let path = if typed.starts_with('/') {
            typed.clone()
        } else {
            format!("{}/{typed}", project.trim_end_matches('/'))
        };
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let format = zoom.format;
        let number = |input: &Entity<InputState>, fallback: f64| {
            input
                .read(cx)
                .value()
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| *v > 0.)
                .map_or(fallback, |v| v.clamp(100., 6000.))
        };
        let (width, height) = (number(&zoom.width, 1800.), number(&zoom.height, 1200.));
        let replace = zoom.replace.as_deref() == Some(path.as_str());
        zoom.working = true;
        zoom.note = None;
        let folder = typed
            .rsplit_once('/')
            .map_or(String::new(), |(f, _)| f.to_string());
        if !typed.starts_with('/') {
            self.plot_history.folder = Some(folder);
        }
        let stat = daemon.call::<Vec<FileStat>>(Call::StatFiles {
            paths: vec![path.clone()],
        });
        let blob = item.value[sandhill_proto::BLOB_KEY]
            .as_str()
            .map(str::to_string);
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        cx.spawn(async move |this, cx| {
            let outcome: anyhow::Result<Option<String>> = async {
                let exists = stat.await?.first().is_some_and(|s| s.mtime_ms.is_some());
                if exists && !replace {
                    return Ok(None);
                }
                if redrawn {
                    daemon
                        .call::<PlotRedraw>(Call::RedrawPlot {
                            session: item.session.clone(),
                            run: item.run.clone(),
                            image,
                            index,
                            file: Some(path.clone()),
                            format: format.to_string(),
                            width: width / f64::from(DPI),
                            height: height / f64::from(DPI),
                            dpi: DPI,
                            show: false,
                        })
                        .await?;
                    let size = if format == "png" {
                        format!("{width:.0} × {height:.0} px")
                    } else {
                        format!(
                            "{:.1} × {:.1} in",
                            width / f64::from(DPI),
                            height / f64::from(DPI)
                        )
                    };
                    return Ok(Some(size));
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
            this.update(cx, |this, cx| {
                let shown = this.relative_path(&path);
                if let Some(zoom) = this.plot_history.zoom.as_mut() {
                    zoom.working = false;
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
        let (list, at) = self.plots_now();
        let item = list.get(at?)?;
        let at = at?;
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let formats: &[&'static str] = if redrawn {
            &["png", "svg", "pdf"]
        } else {
            plain_formats(&item.mime)
        };
        let explain = match &zoom.redraw {
            None => "Asking its session whether it can draw it again…".to_string(),
            Some(r) if r.can => {
                "Drawn again by its session at this size, so lines and text stay sharp.".into()
            }
            Some(r) => match &r.reason {
                Some(why) => format!("Saved as drawn: {why}"),
                None => "Saved as drawn: its session can't draw it again.".into(),
            },
        };
        let image: AnyElement = match self.plot_image(item) {
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
        let sized = redrawn;
        let back = at > 0;
        let on = at + 1 < list.len();
        let card = v_flex()
            .id("plot-zoom")
            .occlude()
            .w(relative(0.86))
            .h(relative(0.86))
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
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.step_plot(-1, cx);
                                this.open_zoom(window, cx);
                            })),
                    )
                    .child(
                        icon_button("zoom-next", "next", &p, false)
                            .when(!on, |d| d.opacity(0.35))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.step_plot(1, cx);
                                this.open_zoom(window, cx);
                            })),
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
                    .child(image),
            )
            .child(
                v_flex()
                    .flex_none()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(10.))
                    .border_t(px(0.5))
                    .border_color(p.glass_edge)
                    .text_size(px(look::SMALL + 1.))
                    .child(
                        h_flex()
                            .gap(px(10.))
                            .child(
                                h_flex()
                                    .gap(px(2.))
                                    .p(px(2.))
                                    .rounded(px(8.))
                                    .bg(p.ink(6.))
                                    .text_color(p.muted)
                                    .children(formats.iter().map(|f| segment(f, cx))),
                            )
                            .when(sized, |d| {
                                d.child(div().w(px(80.)).child(Input::new(&zoom.width).small()))
                                    .child("×")
                                    .child(div().w(px(80.)).child(Input::new(&zoom.height).small()))
                                    .child(
                                        div().text_color(p.faint).child(format!("px at {DPI} dpi")),
                                    )
                            }),
                    )
                    .child(div().text_color(p.faint).child(explain))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(div().flex_1().child(Input::new(&zoom.name).small()))
                            .child(
                                div()
                                    .id("zoom-save")
                                    .h(px(28.))
                                    .px(px(14.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(RADIUS))
                                    .bg(p.accent)
                                    .text_color(p.accent_text)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .child(if zoom.replace.is_some() {
                                        "Replace"
                                    } else {
                                        "Save"
                                    })
                                    .when(zoom.working || zoom.redraw.is_none(), |d| d.opacity(0.5))
                                    .when(!zoom.working && zoom.redraw.is_some(), |d| {
                                        d.on_click(cx.listener(|this, _, _, cx| this.save_plot(cx)))
                                    }),
                            ),
                    )
                    .when_some(zoom.note.clone(), |d, note| {
                        d.child(div().text_color(p.muted).child(note))
                    }),
            );
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
        History, PlotItem, from_notebook, plain_formats, plot_item, plot_name, redraw_target,
        source,
    };
    use serde_json::json;

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
        let live = vec!["s1".to_string()];
        h.cleared = Some(10);
        assert_eq!(h.shown(&live).len(), 1);
        assert!(h.shown(&[]).is_empty());
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
}
