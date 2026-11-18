//! Read-only AnnData preview. The file stays on the daemon's device.

use std::path::Path;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, H5adColumnKind, H5adMatrix, H5adStorage, H5adSummary, H5adTable};

use crate::daemon::Daemon;
use crate::editor::html_table::Table;
use crate::editor::output_table::SelectionOwner;
use crate::editor::table::format_number;
use crate::look::{self, Palette};
use crate::workbench::{PreviewTab, Workbench};

pub(crate) fn is_h5ad(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("h5ad"))
}

enum State {
    Loading,
    Failed(SharedString),
    Ready(Box<SummaryView>),
}

pub(crate) struct H5adView {
    state: State,
    scroll: ScrollHandle,
    _load: Task<()>,
}

impl H5adView {
    fn new(path: String, daemon: &Daemon, cx: &mut Context<Self>) -> Self {
        let load = daemon.call::<H5adSummary>(Call::DescribeH5ad { path });
        let task = cx.spawn(async move |this, cx| {
            let result = load.await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(summary) => State::Ready(Box::new(SummaryView::new(summary, cx))),
                    Err(error) => {
                        State::Failed(format!("Could not preview this .h5ad: {error:#}").into())
                    }
                };
                cx.notify();
            })
            .ok();
        });
        Self {
            state: State::Loading,
            scroll: ScrollHandle::new(),
            _load: task,
        }
    }
}

impl Render for H5adView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::get(cx);
        let body = match &self.state {
            State::Loading => div().text_color(p.muted).child("Reading AnnData summary…"),
            State::Failed(message) => div().text_color(p.err).child(message.clone()),
            State::Ready(summary) => summary.render(p),
        };
        look::PaneScroll::new(
            "h5ad",
            0.,
            look::PANE_PAD_Y,
            div()
                .id("h5ad-content")
                .px(px(look::PANE_PAD_X))
                .pb(px(look::PANE_PAD_X))
                .text_color(p.text)
                .text_size(px(look::small()))
                .child(body),
        )
        .handle(&self.scroll)
    }
}

struct SummaryView {
    size: SharedString,
    x: SharedString,
    layers: Vec<(SharedString, SharedString)>,
    embeddings: Vec<(SharedString, SharedString)>,
    other: Vec<(SharedString, SharedString)>,
    obs: TablePreview,
    var: TablePreview,
}

impl SummaryView {
    fn new(summary: H5adSummary, cx: &mut Context<H5adView>) -> Self {
        let H5adSummary {
            n_obs,
            n_vars,
            x,
            layers,
            obs,
            var,
            obsm,
            varm,
            obsp,
            varp,
            uns,
            raw_n_vars,
        } = summary;
        let mut other = Vec::new();
        for (label, entries) in [("varm", varm), ("obsp", obsp), ("varp", varp)] {
            if !entries.is_empty() {
                other.push((
                    label.into(),
                    entries
                        .into_iter()
                        .map(|(key, dims)| format!("{key} ({})", shape(&dims)))
                        .collect::<Vec<_>>()
                        .join(", ")
                        .into(),
                ));
            }
        }
        if !uns.is_empty() {
            other.push(("uns".into(), uns.join(", ").into()));
        }
        other.push((
            "raw".into(),
            raw_n_vars.map_or_else(
                || "Not present".into(),
                |n| format!("Present · {} genes", number(n)).into(),
            ),
        ));
        Self {
            size: format!("{} cells × {} genes", number(n_obs), number(n_vars)).into(),
            x: x.map_or_else(|| "Not present".into(), |x| matrix(&x)),
            layers: layers
                .into_iter()
                .map(|(name, value)| (name.into(), matrix(&value)))
                .collect(),
            embeddings: obsm
                .into_iter()
                .map(|(name, dims)| (name.into(), shape(&dims).into()))
                .collect(),
            other,
            obs: TablePreview::new(obs, n_obs, cx),
            var: TablePreview::new(var, n_vars, cx),
        }
    }

    fn render(&self, p: &Palette) -> Div {
        v_flex()
            .gap(px(look::GAP_12))
            .child(
                section("Size", p)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.size.clone()),
                    )
                    .child(detail("X".into(), self.x.clone(), p)),
            )
            .child(section("Layers", p).child(details(&self.layers, p)))
            .child(section("Embeddings (obsm)", p).child(details(&self.embeddings, p)))
            .child(section("Also inside", p).child(details(&self.other, p)))
            .child(self.obs.render("Cell table (obs)", p))
            .child(self.var.render("Gene table (var)", p))
    }
}

fn section(label: &'static str, p: &Palette) -> Div {
    v_flex().gap(px(look::GAP_4)).min_w_0().child(
        div()
            .text_size(px(look::title()))
            .text_color(p.text)
            .font_weight(FontWeight::SEMIBOLD)
            .child(label),
    )
}

fn detail(name: SharedString, description: SharedString, p: &Palette) -> Div {
    h_flex()
        .items_start()
        .gap(px(look::GAP_8))
        .min_w_0()
        .child(
            div()
                .flex_none()
                .w(relative(0.30))
                .font_family(look::mono())
                .whitespace_normal()
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(p.muted)
                .whitespace_normal()
                .child(description),
        )
}

fn details(entries: &[(SharedString, SharedString)], p: &Palette) -> Div {
    if entries.is_empty() {
        return div().text_color(p.muted).child("None");
    }
    v_flex().gap(px(look::GAP_4)).children(
        entries
            .iter()
            .map(|(name, description)| detail(name.clone(), description.clone(), p)),
    )
}

fn number(n: u64) -> String {
    format_number(n as f64)
}

fn shape(dims: &[u64]) -> String {
    dims.iter()
        .map(|&n| number(n))
        .collect::<Vec<_>>()
        .join(" × ")
}

fn matrix(matrix: &H5adMatrix) -> SharedString {
    let (storage, nnz, share) = match matrix.storage {
        H5adStorage::Dense => return format!("Dense · {}", matrix.dtype).into(),
        H5adStorage::Csr { nnz, share } => ("CSR", nnz, share),
        H5adStorage::Csc { nnz, share } => ("CSC", nnz, share),
    };
    format!(
        "Sparse ({storage}) · {} · {}% non-zero ({} stored value{})",
        matrix.dtype,
        percent(share),
        number(nnz),
        if nnz == 1 { "" } else { "s" }
    )
    .into()
}

/// A share as a percentage with two significant digits below 1%
/// ("0.82", "0.0031"), one decimal below 10% and none above.
fn percent(share: f64) -> String {
    let p = share * 100.;
    if p <= 0. || !p.is_finite() {
        return "0".into();
    }
    let decimals = if p >= 10. {
        0
    } else if p >= 1. {
        1
    } else {
        ((1. - p.log10().floor()) as usize).min(6)
    };
    format!("{p:.decimals$}")
}

fn kind(kind: H5adColumnKind) -> SharedString {
    match kind {
        H5adColumnKind::Number { dtype } => format!("Number ({dtype})").into(),
        H5adColumnKind::Bool => "Bool".into(),
        H5adColumnKind::Text => "Text".into(),
        H5adColumnKind::Category { count, examples } => {
            let mut text = format!("Category ({})", number(count));
            if !examples.is_empty() {
                text.push_str(": ");
                text.push_str(&examples.join(", "));
                if count > examples.len() as u64 {
                    text.push_str(", …");
                }
            }
            text.into()
        }
    }
}

struct TablePreview {
    schema: Vec<(SharedString, SharedString)>,
    caption: SharedString,
    grid: Entity<SelectionOwner>,
}

impl TablePreview {
    fn new(table: H5adTable, nrows: u64, cx: &mut Context<H5adView>) -> Self {
        let H5adTable {
            index_name,
            index,
            columns,
            column_count,
        } = table;
        let mut schema = Vec::with_capacity(columns.len());
        let mut headers = Vec::with_capacity(columns.len() + 1);
        headers.push(index_name);
        let mut rows: Vec<_> = index
            .into_iter()
            .map(|value| {
                let mut row = Vec::with_capacity(columns.len() + 1);
                row.push(value);
                row
            })
            .collect();
        for column in columns {
            schema.push((column.name.clone().into(), kind(column.kind)));
            headers.push(column.name);
            for (row, value) in rows.iter_mut().zip(column.values) {
                row.push(value);
            }
        }
        let mut caption = format!("{} of {} rows", rows.len(), number(nrows));
        if schema.len() as u64 != column_count {
            caption.push_str(&format!(
                " · {} of {} columns",
                schema.len(),
                number(column_count)
            ));
        }
        let grid = cx.new(|cx| {
            SelectionOwner::new(
                Table {
                    columns: headers,
                    rows,
                    caption: None,
                },
                cx,
            )
        });
        Self {
            schema,
            caption: caption.into(),
            grid,
        }
    }

    fn render(&self, label: &'static str, p: &Palette) -> Div {
        section(label, p)
            .when(!self.schema.is_empty(), |d| {
                d.child(details(&self.schema, p))
            })
            .child(div().text_color(p.muted).child(self.caption.clone()))
            .child(self.grid.clone())
    }
}

impl Workbench {
    pub(crate) fn open_h5ad(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.previews.contains_key(&path) {
            let Some(daemon) = self.daemon() else { return };
            let view = cx.new(|cx| H5adView::new(path.clone(), &daemon, cx));
            self.tabs.push(path.clone());
            self.previews.insert(path.clone(), PreviewTab::H5ad(view));
        }
        self.show_file_tab(&path, cx);
    }

    pub(crate) fn h5ad_toolbar(&self, path: &str, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let path = path.to_string();
        h_flex()
            .gap(px(look::GAP_8))
            .px(px(look::GAP_8))
            .child(
                div()
                    .text_size(px(look::small()))
                    .text_color(p.muted)
                    .child("AnnData · Read only"),
            )
            .child(
                look::button(
                    "load-h5ad",
                    "Load into Python…",
                    Some("data.import"),
                    look::ButtonSize::Row,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_h5ad_import(path.clone(), window, cx)
                })),
            )
    }
}
