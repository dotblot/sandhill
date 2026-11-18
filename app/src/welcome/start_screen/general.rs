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

        let mut kinds = h_flex()
            .w_full()
            .flex_wrap()
            .gap_y(px(look::GAP_8))
            .pt(px(look::GAP_8));
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
            .gap(px(look::GAP_12))
            .child(
                v_flex()
                    .gap(px(look::GAP_6))
                    .child(div().text_size(px(look::WELCOME_TITLE)).font_weight(FontWeight::BOLD).child("General"))
                    .child(div().text_size(px(look::TAB_TEXT)).text_color(p.muted).child(
                        "Choices for every project on this Mac. The buttons beside the editor, notebooks and panes change the same choices.",
                    )),
            )
            .child(look::settings_group(
                "Appearance",
                std::iter::once(
                    row("Theme", Some("System follows the Mac's light or dark appearance."), &p)
                        .child(themes)
                        .into_any_element(),
                )
                .chain(cfg!(target_os = "macos").then(|| {
                    switch_row(
                        "general-glass",
                        "Glass",
                        "The window shows a blur of what is behind it. Off, it is solid.",
                        crate::prefs::glass(),
                        |on, cx| choose(Choice::Glass(on), cx),
                        &p,
                    )
                }))
                .collect(),
                &p,
            ))
            .child(look::settings_group(
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
            .child(look::settings_group(
                "Editor",
                vec![
                    v_flex()
                        .w_full()
                        .px(px(look::PANE_PAD_X))
                        .py(px(look::PANE_PAD_Y))
                        .child(look::settings_label(
                            "Wrap long lines",
                            Some(look::settings_detail("For each kind of file. The Wrap button in the editor changes the same choice.", &p)),
                        ))
                        .child(kinds)
                        .into_any_element(),
                ],
                &p,
            ))
            .child(look::settings_group(
                "Files",
                vec![row("Sort files by", Some("Folders always come first."), &p)
                    .child(sorts)
                    .into_any_element()],
                &p,
            ))
            .child(look::settings_group(
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

/// A row with its name and grey line on the left; the caller adds the
/// control.
fn row(label: &'static str, detail: Option<&'static str>, p: &Palette) -> Div {
    look::settings_row(label, detail.map(|detail| look::settings_detail(detail, p)))
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
