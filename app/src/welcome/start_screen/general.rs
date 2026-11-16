//! Manage → General: this Mac's choices for every project (Huan,
//! 2026-11-15). Each also stays beside what it changes; both save the same
//! choice, through `shell::choices` where open projects must follow.

use gpui_kit::component::checkbox::Checkbox;

use super::*;
use crate::panes::tree::SortBy;
use crate::shell::choices::{Choice, choose};
use crate::workbench::Workbench;

/// The file types whose long lines can wrap, as the editor's Wrap button
/// keys them (`editor_wrap_type`). CSV and TSV never wrap, so their
/// columns keep their colours.
const WRAP_KINDS: [(&str, &str); 16] = [
    ("python", "Python"),
    ("r", "R"),
    ("markdown", "Markdown"),
    ("text", "Plain text"),
    ("bash", "Shell"),
    ("json", "JSON"),
    ("yaml", "YAML"),
    ("toml", "TOML"),
    ("fasta", "FASTA"),
    ("fastq", "FASTQ"),
    ("bio-table", "Genomics table"),
    ("tree", "Tree (Newick, Nexus)"),
    ("nextflow", "Nextflow"),
    ("wdl", "WDL"),
    ("bibtex", "BibTeX"),
    ("code", "Other code"),
];

impl AppWindow {
    pub(super) fn general_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let p = welcome_palette(cx);
        let now = crate::prefs::general();

        let mut themes = look::tab_capsule(&p).gap(px(1.));
        for (ix, (theme, label)) in [
            (None, "System"),
            (Some("light"), "Light"),
            (Some("dark"), "Dark"),
        ]
        .into_iter()
        .enumerate()
        {
            themes = themes.child(
                Workbench::tab(
                    ("general-theme", ix),
                    None,
                    label,
                    now.appearance.as_deref() == theme,
                    &p,
                )
                .on_click(move |_, window, cx| look::choose_appearance(theme, window, cx)),
            );
        }

        let sorted = SortBy::from_key(&now.tree_sort);
        let mut sorts = look::tab_capsule(&p).gap(px(1.));
        for (ix, by) in SortBy::ALL.into_iter().enumerate() {
            sorts = sorts.child(
                Workbench::tab(("general-sort", ix), None, by.label(), sorted == by, &p)
                    .on_click(move |_, _, cx| choose(Choice::Sort(by), cx)),
            );
        }

        let mut kinds = h_flex().w_full().flex_wrap().gap_y(px(8.)).pt(px(10.));
        for (ix, (kind, label)) in WRAP_KINDS.into_iter().enumerate() {
            kinds = kinds.child(
                div().w(relative(0.25)).whitespace_nowrap().child(
                    Checkbox::new(("general-wrap", ix))
                        .label(label)
                        .checked(crate::prefs::editor_soft_wrap(kind))
                        .on_click(move |on: &bool, _, cx| choose(Choice::Wrap(kind, *on), cx)),
                ),
            );
        }

        v_flex()
            .w_full()
            .gap(px(22.))
            .child(
                v_flex()
                    .gap(px(6.))
                    .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child("General"))
                    .child(div().text_size(px(14.)).text_color(p.muted).child(
                        "Choices for every project on this Mac. The buttons beside the editor, notebooks and panes change the same choices.",
                    )),
            )
            .child(group(
                "Appearance",
                vec![row("Theme", Some("System follows the Mac's light or dark appearance."), &p)
                    .child(themes)
                    .into_any_element()],
                &p,
            ))
            .child(group(
                "Notebooks and Quarto",
                vec![
                    switch_row(
                        "general-nb-console",
                        "Show their runs in the Console",
                        "Code run in notebooks and Quarto documents appears in the Console too.",
                        now.notebook_console,
                        |on, cx| choose(Choice::NotebookConsole(on), cx),
                        &p,
                    ),
                    switch_row(
                        "general-nb-plots",
                        "Show their plots in Plots",
                        "Plots go to the Plots pane as well as under their cells.",
                        now.notebook_plots,
                        |on, cx| choose(Choice::NotebookPlots(on), cx),
                        &p,
                    ),
                    switch_row(
                        "general-ghost",
                        "Suggest the next cell after each run",
                        "With Claude Code or Codex, as the notebook's Suggest button does.",
                        now.ghost_cells,
                        |on, cx| choose(Choice::GhostCells(on), cx),
                        &p,
                    ),
                    switch_row(
                        "general-follow",
                        "Follow the AI's edits",
                        "Notebooks scroll to, and light up, the cells an agent adds or changes.",
                        now.follow_agent,
                        |on, cx| choose(Choice::FollowAgent(on), cx),
                        &p,
                    ),
                ],
                &p,
            ))
            .child(group(
                "Editor",
                vec![
                    v_flex()
                        .w_full()
                        .px(px(14.))
                        .py(px(10.))
                        .child(label_column(
                            "Wrap long lines",
                            Some("For each kind of file. The Wrap button in the editor changes the same choice."),
                            &p,
                        ))
                        .child(kinds)
                        .into_any_element(),
                ],
                &p,
            ))
            .child(group(
                "Files",
                vec![row("Sort files by", Some("Folders always come first."), &p)
                    .child(sorts)
                    .into_any_element()],
                &p,
            ))
            .child(group(
                "Plots and tables",
                vec![
                    switch_row(
                        "general-plot-settings",
                        "Show the settings column in plot windows",
                        "Format, size and where plots are saved. Each plot window can still hide it.",
                        now.plot_settings,
                        |on, cx| {
                            crate::prefs::save_plot_settings_shown(on);
                            cx.refresh_windows();
                        },
                        &p,
                    ),
                    switch_row(
                        "general-column-panel",
                        "Show the column panel when a table opens",
                        "In the Data Explorer: each column's missing values and statistics.",
                        now.column_panel,
                        |on, cx| {
                            crate::prefs::save_column_stats(on);
                            cx.refresh_windows();
                        },
                        &p,
                    ),
                ],
                &p,
            ))
            .into_any_element()
    }
}

/// A titled box of rows with lines between them, as in the Mac's settings.
fn group(title: &'static str, rows: Vec<AnyElement>, p: &Palette) -> Div {
    let mut body = v_flex()
        .w_full()
        .rounded(px(look::RADIUS))
        .border_1()
        .border_color(p.line);
    for (ix, row) in rows.into_iter().enumerate() {
        body = body.child(
            div()
                .w_full()
                .when(ix > 0, |d| d.border_t_1().border_color(p.line))
                .child(row),
        );
    }
    v_flex()
        .w_full()
        .gap(px(6.))
        .child(sidebar_heading(title, p).px(px(14.)).py(px(0.)))
        .child(body)
}

/// A row's name, with a muted line under it.
fn label_column(label: &'static str, detail: Option<&'static str>, p: &Palette) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(2.))
        .child(label)
        .children(detail.map(|detail| div().text_size(px(12.5)).text_color(p.muted).child(detail)))
}

/// A row with its name on the left; the caller adds the control.
fn row(label: &'static str, detail: Option<&'static str>, p: &Palette) -> Div {
    h_flex()
        .w_full()
        .min_h(px(44.))
        .gap(px(16.))
        .px(px(14.))
        .py(px(10.))
        .child(label_column(label, detail, p))
}

fn switch_row(
    id: &'static str,
    label: &'static str,
    detail: &'static str,
    on: bool,
    change: impl Fn(bool, &mut App) + 'static,
    p: &Palette,
) -> AnyElement {
    row(label, Some(detail), p)
        .child(
            Switch::new(id)
                .small()
                .color(p.accent)
                .checked(on)
                .accessibility_label(label)
                .on_change(move |on: &bool, _, cx| change(*on, cx)),
        )
        .into_any_element()
}
