//! Manage → General: this Mac's choices for every project (Huan,
//! 2026-11-15). Each also stays beside what it changes; both save the same
//! choice, through `shell::choices` where open projects must follow.

use std::ops::RangeInclusive;

use gpui_kit::base::ElementExt as _;
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
                    .child(div().text_size(px(look::welcome_title())).font_weight(FontWeight::BOLD).child("General"))
                    .child(div().text_size(px(look::tab_text())).text_color(p.muted).child(
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
            .child(self.fonts_group(&p, cx))
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

    /// The code font and the two text sizes (Huan, 2026-11-18). The
    /// interface always uses the system font.
    fn fonts_group(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let now = cx
            .try_global::<look::Typography>()
            .cloned()
            .unwrap_or_default();
        let font_detail = match &now.missing {
            Some(font) => format!("{font} is not installed, so code uses the system font."),
            None => {
                "Any monospaced font on this Mac. The interface always uses the system font.".into()
            }
        };
        let font = now.code_font.clone();
        let ui_size = now.ui_size;
        let code_size = now.code_size;
        let default = now.code_font.is_none()
            && now.code_size == look::DEFAULT_SIZE
            && now.ui_size == look::DEFAULT_SIZE;
        look::settings_group(
            "Fonts",
            vec![
                look::settings_row("Code font", Some(look::settings_detail(font_detail, p)))
                    .child(self.font_select(&now, p, cx))
                    .into_any_element(),
                row(
                    "Code size",
                    Some("The editor, notebooks, the Console, the terminal, and code in Help and AI replies."),
                    p,
                )
                .child(stepper("general-code-size", code_size, look::CODE_SIZES, {
                    let font = font.clone();
                    move |size, window, cx| {
                        look::choose_typography(font.clone(), size, ui_size, window, cx)
                    }
                }))
                .into_any_element(),
                row(
                    "Interface size",
                    Some("Text in menus, panes, tabs and buttons; rows and buttons grow to fit."),
                    p,
                )
                .child(stepper("general-ui-size", ui_size, look::UI_SIZES, {
                    move |size, window, cx| {
                        look::choose_typography(font.clone(), code_size, size, window, cx)
                    }
                }))
                .into_any_element(),
                row("Defaults", Some("SF Mono for code, and 15 for both sizes."), p)
                    .child(
                        look::button("general-fonts-reset", "Restore", None, look::ButtonSize::Row)
                            .disabled(default)
                            .on_click(|_, window, cx| {
                                look::choose_typography(
                                    None,
                                    look::DEFAULT_SIZE,
                                    look::DEFAULT_SIZE,
                                    window,
                                    cx,
                                )
                            }),
                    )
                    .into_any_element(),
            ],
            p,
        )
        .into_any_element()
    }

    /// The code font picker: the system's first, then every installed
    /// monospaced font, found the first time the list opens.
    fn font_select(&self, now: &look::Typography, p: &Palette, cx: &mut Context<Self>) -> Div {
        const TRIGGER: &str = "general-code-font";
        let p = *p;
        let system = if cfg!(target_os = "macos") {
            "System (SF Mono)"
        } else {
            "System"
        };
        let current = now.code_font.clone().filter(|_| now.missing.is_none());
        let shown: SharedString = current.clone().map_or(system.into(), Into::into);
        let hover = p.ink(look::BUTTON_HOVER);
        let button = h_flex()
            .id(TRIGGER)
            .on_prepaint(self.start.font_trigger.track(TRIGGER))
            .min_w_0()
            .max_w(px(240.))
            .h(px(look::control_row()))
            .gap(px(look::GAP_4))
            .px(px(look::GAP_8))
            .rounded_full()
            .bg(p.cap_bg)
            .text_size(px(look::tab_text()))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(shown),
            )
            .child(
                icon("chevron.down")
                    .size(px(look::ICON_SMALL))
                    .text_color(p.faint),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.start.font_menu = !this.start.font_menu;
                if this.start.font_menu {
                    this.start.dialog_focus.focus(window, cx);
                    if this.start.mono_fonts.is_none() {
                        this.list_mono_fonts(cx);
                    }
                }
                cx.notify();
            }));
        let list = self.start.font_menu.then(|| {
            let finding = self.start.mono_fonts.is_none();
            let options = std::iter::once((None, SharedString::from(system))).chain(
                self.start
                    .mono_fonts
                    .iter()
                    .flatten()
                    .map(|font| (Some(font.clone()), SharedString::from(font.clone()))),
            );
            let (code_size, ui_size) = (now.code_size, now.ui_size);
            crate::agents::composer_view::popup(&p)
                .id("general-code-font-list")
                .track_focus(&self.start.dialog_focus)
                .min_w(px(220.))
                .max_w(px(320.))
                .max_h(px(360.))
                .overflow_y_scroll()
                .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if !this.start.font_trigger.contains(TRIGGER, event.position) {
                        this.start.font_menu = false;
                        cx.notify();
                    }
                }))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.start.font_menu = false;
                        cx.notify();
                    }
                }))
                .children(options.enumerate().map(|(ix, (font, label))| {
                    let on = font == current;
                    h_flex()
                        .id(("general-code-font-item", ix))
                        .flex_none()
                        .h(px(look::row()))
                        .px(px(look::GAP_8))
                        .gap(px(look::GAP_4))
                        .rounded(px(look::CONTROL_RADIUS))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(div().w(px(look::ICON)).flex_none().when(on, |d| {
                            d.child(icon("check").size(px(look::ICON)).text_color(p.muted))
                        }))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start.font_menu = false;
                            look::choose_typography(font.clone(), code_size, ui_size, window, cx);
                            cx.notify();
                        }))
                }))
                .when(finding, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .h(px(look::row()))
                            .px(px(look::GAP_8))
                            .flex()
                            .items_center()
                            .text_color(p.muted)
                            .child("Finding fonts…"),
                    )
                })
        });
        div()
            .relative()
            .flex_none()
            .min_w_0()
            .child(button)
            .when_some(list, |d, list| {
                d.child(
                    div()
                        .absolute()
                        .right_0()
                        .top(px(look::control_row() + look::GAP_2))
                        .child(
                            deferred(
                                anchored()
                                    .anchor(Anchor::TopRight)
                                    .snap_to_window_with_margin(px(look::PANE_PAD_Y))
                                    .child(list),
                            )
                            .with_priority(3),
                        ),
                )
            })
    }

    /// Finds the installed monospaced fonts off the main thread; it opens
    /// every font family.
    fn list_mono_fonts(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let fonts = cx
                .background_spawn(async { look::monospaced_families() })
                .await;
            this.update(cx, |this, cx| {
                this.start.mono_fonts = Some(fonts);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// A size with − and + either side, each a point at a time, within `range`.
fn stepper(
    id: &'static str,
    value: f32,
    range: RangeInclusive<f32>,
    change: impl Fn(f32, &Window, &mut App) + 'static,
) -> Div {
    let change = Rc::new(change);
    let smaller = change.clone();
    h_flex()
        .flex_none()
        .gap(px(look::GAP_4))
        .items_center()
        .child(
            look::button((id, 0usize), "−", None, look::ButtonSize::Row)
                .disabled(value <= *range.start())
                .on_click(move |_, window, cx| smaller(value - 1., window, cx)),
        )
        .child(
            div()
                .min_w(px(28.))
                .text_center()
                .text_size(px(look::small()))
                .child(format!("{value}")),
        )
        .child(
            look::button((id, 1usize), "+", None, look::ButtonSize::Row)
                .disabled(value >= *range.end())
                .on_click(move |_, window, cx| change(value + 1., window, cx)),
        )
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
