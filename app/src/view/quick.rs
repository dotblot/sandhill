//! The Explore a Table window (`crate::quick`): a title bar naming the session
//! and its tables, then the Data Explorer, Plots and the Guide side by side
//! (Huan, 2026-11-19).

use gpui_kit::component::Disableable as _;
use sandhill_proto::{Known, QuickColumnKind, QuickPlot, QuickTableKind, QuickValues};

use super::*;
use crate::look::side_pane;
use crate::quick::{self, Edit, Item};

/// The Guide's share of the window; the Data Explorer and Plots split the
/// rest.
const GUIDE_SHARE: f32 = 0.25;
/// The width of a fact's name on the card.
const FACT_LABEL: f32 = 92.;
/// The width of a sample's name beside its group choices.
const SAMPLE_LABEL: f32 = 64.;

impl Workbench {
    pub(super) fn render_quick_top_bar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let name = self.quick_name();
        let unkept = self.quick_unkept();
        let folder = self.quick_folder();
        let mut tables = look::tab_capsule(&p).gap(px(1.));
        let mut any = false;
        for (ix, table) in q.card.tables.iter().enumerate() {
            if !q.tables.iter().any(|t| t.name == table.name) {
                continue;
            }
            any = true;
            let label = match &table.sheet {
                Some(sheet) => format!("{} · {sheet}", table.file),
                None => table.file.clone(),
            };
            let picked = q.viewing.as_deref() == Some(&table.name);
            let name = table.name.clone();
            tables = tables.child(
                Self::tab(("quick-table", ix), None, label, picked, &p).on_click(cx.listener(
                    move |this, _, window, cx| this.quick_pick(name.clone(), window, cx),
                )),
            );
        }
        for (ix, (output, _)) in q.outputs.iter().enumerate() {
            any = true;
            let picked = q.viewing.as_deref() == Some(output);
            let name = output.clone();
            tables = tables.child(
                Self::tab(("quick-output", ix), None, output.clone(), picked, &p).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.quick_view_output(name.clone(), window, cx)
                    }),
                ),
            );
        }
        let ready = q.loaded_in.is_some() && q.busy.is_none();
        div()
            .relative()
            .flex_none()
            .h(px(look::TITLE_BAR))
            .w_full()
            .child(
                div()
                    .id("quick-title-drag")
                    .absolute()
                    .size_full()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            if cfg!(target_os = "macos") {
                                window.titlebar_double_click();
                            } else {
                                window.zoom_window();
                            }
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                h_flex()
                    .absolute()
                    .size_full()
                    .pt(px(6.))
                    .pl(px(if window.is_fullscreen() {
                        6.
                    } else {
                        TITLE_BAR_LEFT
                    }))
                    .pr(px(look::GAP_8))
                    .gap(px(look::GAP_8))
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .when(any, |d| d.child(tables.min_w_0().overflow_x_hidden()))
                    .child(div().flex_1())
                    .map(|d| match (unkept, folder) {
                        (true, _) => d.child(
                            Button::new("quick-save")
                                .small()
                                .label("Save Session…")
                                .disabled(q.keeping.is_some())
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.quick_needs_folder(quick::KeepReason::Save, window, cx)
                                })),
                        ),
                        (false, Some(folder)) => d.child(
                            Button::new("quick-reveal-folder")
                                .small()
                                .label("Show in Finder")
                                .on_click(move |_, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&folder))
                                }),
                        ),
                        (false, None) => d,
                    })
                    .child(
                        Button::new("quick-export")
                            .small()
                            .label("Export…")
                            .disabled(!ready || q.keeping.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_needs_folder(quick::KeepReason::Export, window, cx)
                            })),
                    )
                    .child(
                        Button::new("quick-open-project")
                            .small()
                            .label("Open as Project")
                            .disabled(q.keeping.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.quick_needs_folder(quick::KeepReason::OpenProject, window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_quick_workspace(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        h_flex()
            .size_full()
            .items_stretch()
            .gap(px(GAP))
            .drag_over::<ExternalPaths>(move |d, _, _, _| d.bg(p.accent_soft))
            .on_drop(cx.listener(|this, dropped: &ExternalPaths, _, cx| {
                let paths = dropped
                    .paths()
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect();
                this.quick_add(paths, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .child(self.render_quick_explorer(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .child(self.render_quick_plots(cx)),
            )
            .child(
                div()
                    .w(relative(GUIDE_SHARE))
                    .flex_none()
                    .h_full()
                    .flex()
                    .child(self.render_quick_guide(cx)),
            )
            .into_any_element()
    }

    fn render_quick_explorer(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        if let Some(explorer) = &self.explorer {
            return pane(&p)
                .flex_1()
                .h_full()
                .child(div().flex_1().min_h_0().child(explorer.clone()))
                .into_any_element();
        }
        let waiting = self
            .quick
            .as_ref()
            .and_then(|q| q.busy.clone())
            .unwrap_or_else(|| {
                "Your tables show here. Drop more tables onto this window to add them.".into()
            });
        pane(&p)
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .px(px(look::GAP_12))
            .text_color(p.muted)
            .child(div().max_w(px(320.)).text_center().child(waiting))
            .into_any_element()
    }

    fn render_quick_plots(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let empty = self.plots_now().1.is_none();
        side_pane(&p)
            .relative()
            .flex_1()
            .h_full()
            .child(
                Self::strip(&p)
                    .child(div().pl(px(look::GAP_8)).child("Plots"))
                    .child(div().flex_1())
                    .children(self.plot_tools(cx)),
            )
            .child(div().flex_1().min_h_0().map(|d| {
                if empty {
                    d.flex()
                        .items_center()
                        .justify_center()
                        .px(px(look::GAP_12))
                        .text_color(p.muted)
                        .child(
                            div()
                                .max_w(px(300.))
                                .text_center()
                                .child("Plots you make in the Guide show here. Save one with the save button above."),
                        )
                } else {
                    d.child(self.render_plots(cx))
                }
            }))
            .into_any_element()
    }

    fn render_quick_guide(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let mut body = v_flex()
            .gap(px(look::GAP_8))
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y));
        if q.not_ready {
            body = body.child(
                v_flex()
                    .gap(px(look::GAP_8))
                    .child(div().text_color(p.warn).child(look::mac_or(
                        "R isn't ready to explore tables on this Mac yet. Getting Ready in Welcome shows what is missing and installs it.",
                        "R isn't ready to explore tables on this computer yet. Getting Ready in Welcome shows what is missing and installs it.",
                    )))
                    .child(
                        Button::new("quick-setup")
                            .small()
                            .label("Open Getting Ready")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.defer(|cx| {
                                    crate::shell::app_window::show_welcome(
                                        Some(crate::welcome::start_screen::StartMode::Ready),
                                        cx,
                                    )
                                });
                            })),
                    ),
            );
        }
        if let Some(busy) = &q.busy {
            body = body.child(div().text_color(p.muted).child(busy.clone()));
        }
        if let Some(error) = &q.error {
            body = body.child(div().text_color(p.err).child(error.clone()));
        }
        if let Some(path) = &q.exported {
            let at = path.clone();
            body = body.child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(p.muted)
                            .child("Saved the workbook and methods.txt in results."),
                    )
                    .child(
                        Button::new("quick-reveal")
                            .small()
                            .label("Show in Finder")
                            .on_click(move |_, _, cx| cx.reveal_path(std::path::Path::new(&at))),
                    ),
            );
        }
        if let Some(table) = self.quick_table().cloned() {
            body = body
                .child(heading("What we know", &p))
                .child(self.render_quick_card(&table, cx))
                .child(heading("Make Plots", &p))
                .child(self.render_quick_make(&table, cx));
            if let Some(sentence) = &q.sentence {
                body = body.child(
                    div()
                        .p(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .bg(p.ink(look::ROW_HOVER))
                        .child(sentence.clone()),
                );
            }
        }
        side_pane(&p)
            .flex_1()
            .h_full()
            .child(Self::strip(&p).child(div().pl(px(look::GAP_8)).child("Guide")))
            .child(
                div()
                    .id("quick-guide")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_size(px(look::small()))
                    .child(body),
            )
            .into_any_element()
    }

    /// The facts about the table picked, and the session's own.
    fn render_quick_card(
        &self,
        table: &sandhill_proto::QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let kind = table.kind.value;
        let mut card = v_flex().gap(px(look::GAP_2));
        if table.dates > 0 {
            card = card.child(div().text_color(p.warn).child(format!(
                "{} gene names look like dates Excel made (1-Mar, 2-Sep). Those genes can't be matched by name.",
                table.dates
            )));
        }
        if let Some(question) = &table.question {
            card = card.child(
                div()
                    .text_color(p.warn)
                    .child(format!("{question} Choose below what the table is.")),
            );
        }
        card = card.child(self.fact(
            Item::Kind,
            "Table",
            Some((quick::kind_label(kind).to_string(), table.kind.from)),
            cx,
        ));
        let column =
            |known: &Option<Known<String>>| known.as_ref().map(|k| (k.value.clone(), k.from));
        let gene = table.gene.as_ref().map(|g| {
            let ids = table
                .gene_ids
                .as_ref()
                .map(|i| format!(" ({})", quick::ids_label(i.value)))
                .unwrap_or_default();
            (format!("{}{ids}", g.value), g.from)
        });
        match kind {
            QuickTableKind::Results => {
                card = card
                    .child(self.fact(Item::Gene, "Genes", gene, cx))
                    .child(self.fact(Item::Fc, "Fold change", column(&table.fc), cx))
                    .child(self.fact(Item::Padj, "Adjusted p", column(&table.padj), cx))
                    .child(self.fact(Item::Mean, "Average", column(&table.mean), cx));
                if let Some(samples) = &table.samples {
                    card = card
                        .child(
                            self.fact(
                                Item::Values,
                                "Sample values",
                                table
                                    .values
                                    .as_ref()
                                    .map(|v| (quick::values_label(v.value).to_string(), v.from)),
                                cx,
                            ),
                        )
                        .child(self.fact(
                            Item::Groups,
                            "Groups",
                            self.groups_summary(&samples.value),
                            cx,
                        ));
                }
                let comparison = q.card.comparison.as_ref().map(|c| {
                    (
                        format!("higher in {} than in {}", c.value.higher, c.value.lower),
                        c.from,
                    )
                });
                card = card.child(self.fact(Item::Comparison, "Positive means", comparison, cx));
                card = card.child(self.fact(
                    Item::Cutoffs,
                    "Cutoffs",
                    Some((
                        format!(
                            "adjusted p below {}, |log2 fold change| at least {}",
                            q.card.cutoffs.padj, q.card.cutoffs.lfc
                        ),
                        sandhill_proto::QuickFrom::Told,
                    )),
                    cx,
                ));
            }
            QuickTableKind::Counts | QuickTableKind::Normalized => {
                let samples = table.samples.as_ref();
                card = card
                    .child(self.fact(Item::Gene, "Genes", gene, cx))
                    .child(
                        div()
                            .pl(px(FACT_LABEL))
                            .text_color(p.muted)
                            .child(format!("{} samples", samples.map_or(0, |s| s.value.len()))),
                    )
                    .child(
                        self.fact(
                            Item::Values,
                            "Values",
                            table
                                .values
                                .as_ref()
                                .map(|v| (quick::values_label(v.value).to_string(), v.from)),
                            cx,
                        ),
                    );
                if let Some(samples) = samples {
                    card = card.child(self.fact(
                        Item::Groups,
                        "Groups",
                        self.groups_summary(&samples.value),
                        cx,
                    ));
                }
            }
            QuickTableKind::Enrichment => {
                card = card
                    .child(self.fact(Item::Term, "Terms", column(&table.term), cx))
                    .child(self.fact(Item::Padj, "Adjusted p", column(&table.padj), cx))
                    .child(self.fact(Item::Count, "Gene count", column(&table.count), cx))
                    .child(self.fact(Item::Ratio, "Gene ratio", column(&table.ratio), cx));
            }
            QuickTableKind::SampleSheet | QuickTableKind::Unknown => {}
        }
        // No Quick Plot needs the organism: it shows once known.
        if q.card.organism.is_some() {
            card = card.child(self.fact(
                Item::Organism,
                "Organism",
                q.card.organism.as_ref().map(|o| (o.value.clone(), o.from)),
                cx,
            ));
        }
        card.into_any_element()
    }

    fn groups_summary(&self, samples: &[String]) -> Option<(String, sandhill_proto::QuickFrom)> {
        let q = self.quick.as_ref()?;
        let groups = q.card.groups.as_ref()?;
        let mut counts: Vec<(String, usize)> = Vec::new();
        for (sample, group) in &groups.value {
            if !samples.contains(sample) {
                continue;
            }
            match counts.iter_mut().find(|(g, _)| g == group) {
                Some((_, n)) => *n += 1,
                None => counts.push((group.clone(), 1)),
            }
        }
        if counts.is_empty() {
            return None;
        }
        let text = counts
            .iter()
            .map(|(g, n)| format!("{g} ({n})"))
            .collect::<Vec<_>>()
            .join(", ");
        Some((text, groups.from))
    }

    /// One fact: its name, value and where it came from; a click opens its
    /// choices under it.
    fn fact(
        &self,
        item: Item,
        label: &'static str,
        value: Option<(String, sandhill_proto::QuickFrom)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let open = self.quick.as_ref().is_some_and(|q| q.open == Some(item));
        let hover = p.ink(look::ROW_HOVER);
        let (text, from) = match value {
            Some((text, from)) => (text, Some(from)),
            None => ("Still needed".to_string(), None),
        };
        // Cutoffs nobody changed are Sandhill's defaults.
        let default_cutoffs = item == Item::Cutoffs
            && self
                .quick
                .as_ref()
                .is_some_and(|q| q.card.cutoffs == sandhill_proto::QuickCutoffs::default());
        let row = h_flex()
            .id(SharedString::from(format!("quick-fact-{item:?}")))
            .min_h(px(look::row()))
            .px(px(look::GAP_4))
            .py(px(look::GAP_2))
            .gap(px(look::GAP_4))
            .rounded(px(look::CONTROL_RADIUS))
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(
                div()
                    .w(px(FACT_LABEL))
                    .flex_none()
                    .text_color(p.muted)
                    .child(label),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_color(if from.is_some() { p.text } else { p.warn })
                            .child(text),
                    )
                    .children(from.map(|from| {
                        div().text_size(px(look::tiny())).text_color(p.faint).child(
                            if default_cutoffs {
                                "default"
                            } else {
                                quick::from_label(from)
                            },
                        )
                    })),
            )
            .child(
                icon(if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                })
                .size(px(look::ICON))
                .text_color(p.faint),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.quick_toggle(item, cx)));
        v_flex()
            .child(row)
            .when(open, |d| d.child(self.render_choices(item, cx)))
            .into_any_element()
    }

    /// The choices under an opened fact.
    fn render_choices(&self, item: Item, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let Some(table) = self.quick_table() else {
            return div().into_any_element();
        };
        let loaded = q.tables.iter().find(|t| t.name == table.name);
        // Each row of choices needs ids of its own: shared ids made only
        // the first sample's group choices clickable.
        let pills = |key: &str, choices: Vec<(String, bool, Edit)>, cx: &mut Context<Self>| {
            let mut row = h_flex().flex_wrap().gap(px(look::GAP_4));
            for (ix, (label, picked, edit)) in choices.into_iter().enumerate() {
                let edit = std::cell::RefCell::new(Some(edit));
                row = row.child(
                    Self::tab(
                        SharedString::from(format!("quick-choice-{item:?}-{key}-{ix}")),
                        None,
                        label,
                        picked,
                        &p,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(edit) = edit.borrow_mut().take() {
                            this.quick_edit(edit, cx);
                        }
                    })),
                );
            }
            row
        };
        let body: AnyElement = match item {
            Item::Kind => pills(
                "kind",
                [
                    QuickTableKind::Results,
                    QuickTableKind::Counts,
                    QuickTableKind::Normalized,
                    QuickTableKind::Enrichment,
                    QuickTableKind::SampleSheet,
                    QuickTableKind::Unknown,
                ]
                .into_iter()
                .map(|k| {
                    (
                        quick::kind_label(k).to_string(),
                        table.kind.value == k,
                        Edit::Kind(k),
                    )
                })
                .collect(),
                cx,
            )
            .into_any_element(),
            Item::Gene
            | Item::Fc
            | Item::Padj
            | Item::Mean
            | Item::Term
            | Item::Count
            | Item::Ratio => {
                let current = match item {
                    Item::Gene => &table.gene,
                    Item::Fc => &table.fc,
                    Item::Padj => &table.padj,
                    Item::Mean => &table.mean,
                    Item::Term => &table.term,
                    Item::Count => &table.count,
                    _ => &table.ratio,
                };
                let text = matches!(item, Item::Gene | Item::Term);
                let columns: Vec<(String, bool, Edit)> = loaded
                    .map(|t| {
                        t.columns
                            .iter()
                            .filter(|c| {
                                item == Item::Ratio || (c.kind == QuickColumnKind::Number) != text
                            })
                            .map(|c| {
                                (
                                    c.name.clone(),
                                    current.as_ref().is_some_and(|k| k.value == c.name),
                                    Edit::Column(item, c.name.clone()),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                pills("column", columns, cx).into_any_element()
            }
            Item::Values => pills(
                "values",
                [
                    QuickValues::Counts,
                    QuickValues::Normalized,
                    QuickValues::Logged,
                ]
                .into_iter()
                .map(|v| {
                    (
                        quick::values_label(v).to_string(),
                        table.values.as_ref().is_some_and(|k| k.value == v),
                        Edit::Values(v),
                    )
                })
                .collect(),
                cx,
            )
            .into_any_element(),
            Item::Organism => {
                let now = q.card.organism.as_ref().map(|o| o.value.as_str());
                pills(
                    "organism",
                    vec![
                        (
                            "human".into(),
                            now == Some("human"),
                            Edit::Organism(Some("human".into())),
                        ),
                        (
                            "mouse".into(),
                            now == Some("mouse"),
                            Edit::Organism(Some("mouse".into())),
                        ),
                        (
                            "Other or not sure".into(),
                            now.is_none(),
                            Edit::Organism(None),
                        ),
                    ],
                    cx,
                )
                .into_any_element()
            }
            Item::Groups => {
                let names = quick::group_names(&q.card, &q.added_groups);
                let samples = table
                    .samples
                    .as_ref()
                    .map(|s| s.value.clone())
                    .unwrap_or_default();
                let groups = q
                    .card
                    .groups
                    .as_ref()
                    .map(|g| g.value.clone())
                    .unwrap_or_default();
                let mut list = v_flex().gap(px(look::GAP_4));
                let sheets = quick::sheet_columns(q);
                if sheets.iter().any(|(_, _, cols)| !cols.is_empty()) {
                    let mut from = h_flex().flex_wrap().gap(px(look::GAP_4));
                    let mut ix = 0usize;
                    for (sheet, key, cols) in sheets {
                        for col in cols {
                            let (sheet, key, value) = (sheet.clone(), key.clone(), col.clone());
                            from = from.child(
                                Self::tab(
                                    ("quick-groups-from", ix),
                                    None,
                                    format!("{sheet}: {col}"),
                                    false,
                                    &p,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.quick_groups_from(
                                            sheet.clone(),
                                            key.clone(),
                                            value.clone(),
                                            sandhill_proto::QuickFrom::Told,
                                            cx,
                                        )
                                    },
                                )),
                            );
                            ix += 1;
                        }
                    }
                    list = list
                        .child(
                            div()
                                .text_color(p.muted)
                                .child("Take the groups from a sample sheet:"),
                        )
                        .child(from)
                        .child(div().text_color(p.muted).child("Or place each sample:"));
                }
                for (row, sample) in samples.into_iter().enumerate() {
                    let now = groups
                        .iter()
                        .find(|(s, _)| *s == sample)
                        .map(|(_, g)| g.clone());
                    let mut choices: Vec<(String, bool, Edit)> = names
                        .iter()
                        .map(|name| {
                            (
                                name.clone(),
                                now.as_deref() == Some(name),
                                Edit::Group(sample.clone(), Some(name.clone())),
                            )
                        })
                        .collect();
                    choices.push((
                        "None".into(),
                        now.is_none(),
                        Edit::Group(sample.clone(), None),
                    ));
                    list = list.child(
                        h_flex()
                            .gap(px(look::GAP_8))
                            .child(
                                div()
                                    .w(px(SAMPLE_LABEL))
                                    .flex_none()
                                    .truncate()
                                    .child(sample),
                            )
                            .child(pills(&format!("sample-{row}"), choices, cx)),
                    );
                }
                let input = q.inputs.as_ref().map(|i| i.group.clone());
                list.child(
                    h_flex()
                        .gap(px(look::GAP_8))
                        .children(input.map(|i| div().flex_1().child(Input::new(&i).small())))
                        .child(
                            Button::new("quick-add-group")
                                .small()
                                .label("Add Group")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let name = this.quick_input(|i| &i.group, cx);
                                    if let Some(input) = this
                                        .quick
                                        .as_ref()
                                        .and_then(|q| q.inputs.as_ref())
                                        .map(|i| i.group.clone())
                                    {
                                        input.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                    }
                                    this.quick_edit(Edit::AddGroup(name), cx);
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap(px(look::GAP_8))
                        .child(
                            div()
                                .flex_1()
                                .text_color(p.muted)
                                .child("Or a table with each sample's group:"),
                        )
                        .child(
                            Button::new("quick-import-sheet")
                                .small()
                                .label("Import a Sample Sheet…")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.quick_import_sheet(cx)),
                                ),
                        ),
                )
                .into_any_element()
            }
            Item::Comparison => {
                let inputs = q
                    .inputs
                    .as_ref()
                    .map(|i| (i.higher.clone(), i.lower.clone()));
                // With the groups known, each way round is one click.
                let names = quick::group_names(&q.card, &q.added_groups);
                let now = q.card.comparison.as_ref().map(|c| c.value.clone());
                let mut ways: Vec<(String, bool, Edit)> = Vec::new();
                for higher in &names {
                    for lower in names.iter().filter(|l| *l != higher) {
                        ways.push((
                            format!("{higher} vs {lower}"),
                            now.as_ref()
                                .is_some_and(|c| &c.higher == higher && &c.lower == lower),
                            Edit::Comparison(higher.clone(), lower.clone()),
                        ));
                    }
                }
                ways.truncate(12);
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.muted).child(
                        "Which group is a positive fold change higher in? The table itself doesn't say.",
                    ))
                    .when(!ways.is_empty(), |d| d.child(pills("comparison", ways, cx)))
                    .children(inputs.map(|(higher, lower)| {
                        h_flex()
                            .gap(px(look::GAP_4))
                            .child(div().flex_none().child("Higher in"))
                            .child(div().flex_1().child(Input::new(&higher).small()))
                            .child(div().flex_none().child("than in"))
                            .child(div().flex_1().child(Input::new(&lower).small()))
                    }))
                    .child(
                        Button::new("quick-save-comparison")
                            .small()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let higher = this.quick_input(|i| &i.higher, cx);
                                let lower = this.quick_input(|i| &i.lower, cx);
                                this.quick_edit(Edit::Comparison(higher, lower), cx);
                            })),
                    )
                    .into_any_element()
            }
            Item::Cutoffs => {
                let inputs = q.inputs.as_ref().map(|i| (i.padj.clone(), i.lfc.clone()));
                v_flex()
                    .gap(px(look::GAP_4))
                    .children(inputs.map(|(padj, lfc)| {
                        h_flex()
                            .gap(px(look::GAP_4))
                            .child(div().flex_none().child("Adjusted p below"))
                            .child(div().flex_1().child(Input::new(&padj).small()))
                            .child(div().flex_none().child("log2 fold change"))
                            .child(div().flex_1().child(Input::new(&lfc).small()))
                    }))
                    .child(
                        Button::new("quick-save-cutoffs")
                            .small()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let now = this
                                    .quick
                                    .as_ref()
                                    .map(|q| q.card.cutoffs)
                                    .unwrap_or_default();
                                let padj = this
                                    .quick_input(|i| &i.padj, cx)
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .filter(|v| *v > 0. && *v <= 1.)
                                    .unwrap_or(now.padj);
                                let lfc = this
                                    .quick_input(|i| &i.lfc, cx)
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .filter(|v| *v >= 0.)
                                    .unwrap_or(now.lfc);
                                this.quick_edit(Edit::Cutoffs(padj, lfc), cx);
                            })),
                    )
                    .into_any_element()
            }
        };
        div()
            .pl(px(look::GAP_8))
            .py(px(look::GAP_4))
            .child(body)
            .into_any_element()
    }

    /// A button for each plot the table can make; one that can't be made
    /// yet says what it needs.
    fn render_quick_make(
        &self,
        table: &sandhill_proto::QuickCardTable,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let Some(q) = &self.quick else {
            return div().into_any_element();
        };
        let plots = quick::plots_for(table.kind.value);
        if plots.is_empty() {
            return div()
                .text_color(p.muted)
                .child(match table.kind.value {
                    QuickTableKind::SampleSheet => {
                        "A sample sheet has nothing to plot on its own. Its groups can be set on a counts table's card."
                    }
                    _ => "Choose what this table is above to see what can be made from it.",
                })
                .into_any_element();
        }
        let show_gene = self.quick_input(|i| &i.gene, cx);
        let busy = q.busy.is_some() || q.loaded_in.is_none();
        let mut list = v_flex().gap(px(look::GAP_4));
        for (ix, plot) in plots.iter().copied().enumerate() {
            if plot == QuickPlot::GeneByGroup
                && let Some(input) = q.inputs.as_ref().map(|i| i.gene.clone())
            {
                list = list.child(div().child(Input::new(&input).small()));
            }
            let needs = quick::missing(table, plot, &show_gene);
            let tip = (!needs.is_empty()).then(|| format!("Needs {}.", needs.join(", ")));
            list = list.child(
                div()
                    .id(("quick-make-wrap", ix))
                    .child(
                        Button::new(("quick-make", ix))
                            .small()
                            .w_full()
                            .label(quick::plot_label(plot))
                            .disabled(busy || tip.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| this.quick_make(plot, cx))),
                    )
                    .when_some(tip, |d, tip| {
                        d.tooltip(move |w, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
                        })
                    }),
            );
        }
        list.into_any_element()
    }

    /// "Keep the plots and steps from de_results?" (Huan, 2026-11-19):
    /// where and under which name, in the trust card's style. Asked on
    /// Save Session…, Export…, Open as Project, and closing after making
    /// something; nothing is made beside the tables before this.
    pub(crate) fn render_quick_keep(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let q = self.quick.as_ref()?;
        let keeping = q.keeping.as_ref()?;
        let p = *Palette::get(cx);
        let reason = keeping.reason;
        let closing = reason == quick::KeepReason::Close;
        let title = format!("Keep the plots and steps from {}?", self.quick_name());
        let detail = match reason {
            quick::KeepReason::Close => {
                "Sandhill makes a folder with copies of the tables, the results and steps.R. If you don't keep them, nothing is left behind."
            }
            quick::KeepReason::Export => {
                "Exported files go in a folder of their own, with copies of the tables, the results and steps.R."
            }
            quick::KeepReason::OpenProject => {
                "A project needs a folder: Sandhill makes one with copies of the tables, the results and steps.R."
            }
            quick::KeepReason::Save => {
                "Sandhill makes a folder with copies of the tables, the results and steps.R. What you make next goes there too."
            }
        };
        let input = q.inputs.as_ref().map(|i| i.keep_name.clone());
        let busy = keeping.busy;
        let wide = |button: Button| button.w_full();
        let mut buttons = v_flex().gap(px(look::GAP_8)).child(wide(
            Button::new("quick-keep")
                .primary()
                .label(if busy { "Keeping…" } else { "Keep" })
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.quick_keep(cx))),
        ));
        if closing {
            buttons = buttons.child(wide(
                Button::new("quick-dont-keep")
                    .label("Don't Keep")
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| this.quick_dont_keep(window, cx))),
            ));
        }
        buttons = buttons.child(wide(
            Button::new("quick-keep-cancel")
                .label("Cancel")
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| this.quick_cancel_keep(cx))),
        ));
        let card = v_flex()
            .id("quick-keep-card")
            .occlude()
            .w(px(440.))
            .max_w(relative(0.95))
            .p(px(20.))
            .gap(px(look::GAP_12))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(look::title()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(div().text_color(p.muted).child(detail))
            .child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.muted).child("Folder name"))
                    .children(input.map(|i| Input::new(&i).small())),
            )
            .child(
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().text_color(p.muted).child("In"))
                    .child(
                        h_flex()
                            .gap(px(look::GAP_8))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(keeping.parent.clone()),
                            )
                            .child(
                                Button::new("quick-keep-change")
                                    .small()
                                    .label("Change…")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.quick_keep_change_folder(cx)
                                    })),
                            ),
                    ),
            )
            .when_some(keeping.error.clone(), |d, error| {
                d.child(div().text_color(p.err).child(error))
            })
            .child(buttons);
        Some(
            deferred(
                div()
                    .id("quick-keep-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

fn heading(text: &'static str, p: &Palette) -> Div {
    div()
        .pt(px(look::GAP_8))
        .text_color(p.faint)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}
