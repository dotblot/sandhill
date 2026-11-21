//! R and Python → Advanced, collapsed at the bottom of the page: where
//! packages are downloaded from and the system's proxy, the suggested tool
//! for each language, each R's Bioconductor release, and Install Without
//! Asking. A beginner never needs it: the suggested choice is always picked.

use sandhill_proto::{SourceInfo, SourcePrefs};

use super::*;

#[derive(Default)]
pub(super) struct State {
    open: bool,
    sources: Option<Result<Vec<SourceInfo>, String>>,
    /// Reading the sources; `true` while they are checked again.
    loading: Option<bool>,
    generation: u64,
}

impl State {
    /// Another device: its sources are read afresh; the section stays as
    /// open as it was.
    pub(super) fn leave_device(&mut self) {
        *self = Self {
            open: self.open,
            generation: self.generation + 1,
            ..Self::default()
        };
    }
}

/// The kinds of package source, in the order shown.
const KINDS: [(&str, &str); 4] = [
    ("cran", "CRAN"),
    ("bioconductor", "Bioconductor"),
    ("conda", "conda-forge and bioconda"),
    ("pypi", "PyPI"),
];

/// The tools offered for each language, Automatic first.
const TOOLS: [(&str, &str, &[InstallTool]); 2] = [
    ("r", "R", &[InstallTool::R, InstallTool::Conda]),
    (
        "python",
        "Python",
        &[InstallTool::Conda, InstallTool::Uv, InstallTool::Pip],
    ),
];

fn kind_label(kind: &str) -> &str {
    KINDS
        .iter()
        .find(|(id, _)| *id == kind)
        .map_or(kind, |(_, label)| *label)
}

/// The preference for `kind`.
fn source_pref<'a>(sources: &'a mut SourcePrefs, kind: &str) -> Option<&'a mut String> {
    match kind {
        "cran" => Some(&mut sources.cran),
        "bioconductor" => Some(&mut sources.bioconductor),
        "conda" => Some(&mut sources.conda),
        "pypi" => Some(&mut sources.pypi),
        _ => None,
    }
}

/// "answered in 120 ms", "not answering", or nothing before a check.
fn timing(works: bool, ms: Option<u32>) -> Option<String> {
    match (works, ms) {
        (false, _) => Some("not answering".into()),
        (true, Some(ms)) => Some(format!("{ms} ms")),
        (true, None) => None,
    }
}

/// The line under a kind: what is used now and why.
fn source_status(info: &SourceInfo) -> String {
    let chosen = info.options.iter().find(|option| option.url == info.chosen);
    let name = chosen.map_or(info.chosen.as_str(), |option| option.name.as_str());
    let speed = chosen
        .and_then(|option| timing(option.works, option.ms))
        .map(|speed| format!(" ({speed})"))
        .unwrap_or_default();
    if info.automatic {
        format!("Automatic: {name}{speed}, the fastest that works.")
    } else {
        format!("Always {name}{speed}.")
    }
}

impl AppWindow {
    /// Reads the sources when Advanced is open and they aren't read yet.
    pub(super) fn quick_load_advanced(&mut self, cx: &mut Context<Self>) {
        let state = &self.start.quick.advanced;
        if state.open && state.sources.is_none() && state.loading.is_none() {
            self.quick_list_sources(false, cx);
        }
    }

    /// `check`: each source is asked again now, not from the day's check.
    fn quick_list_sources(&mut self, check: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.quick_daemon() else {
            return;
        };
        let state = &mut self.start.quick.advanced;
        if state.loading.is_some() {
            return;
        }
        state.loading = Some(check);
        let generation = state.generation;
        let read = daemon.call::<Vec<SourceInfo>>(Call::ListSources { check });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                let state = &mut this.start.quick.advanced;
                if state.generation == generation {
                    state.loading = None;
                    state.sources = Some(read.map_err(|error| format!("{error:#}")));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Saves a choice of this device's install settings; the reply is the
    /// settings as saved.
    fn quick_save_prefs(&mut self, call: Call, sources: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.quick_daemon() else {
            return;
        };
        let q = &mut self.start.quick;
        if q.setting_prefs {
            return;
        }
        q.setting_prefs = true;
        q.prefs_error = None;
        let saved = daemon.call::<InstallPrefs>(call);
        cx.spawn(async move |this, cx| {
            let saved = saved.await;
            this.update(cx, |this, cx| {
                let q = &mut this.start.quick;
                q.setting_prefs = false;
                match saved {
                    Ok(prefs) => {
                        if let Some(Ok(found)) = &mut q.readiness {
                            found.prefs = prefs;
                        }
                        if sources {
                            // The source in use changed: read them again.
                            let state = &mut q.advanced;
                            state.generation += 1;
                            state.loading = None;
                            state.sources = None;
                            this.quick_list_sources(false, cx);
                        }
                    }
                    Err(error) => q.prefs_error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// `value`: `auto`, or a source's URL.
    fn quick_set_source(&mut self, kind: &'static str, value: String, cx: &mut Context<Self>) {
        let Some(Ok(found)) = &self.start.quick.readiness else {
            return;
        };
        let mut sources = found.prefs.sources.clone();
        let Some(pref) = source_pref(&mut sources, kind) else {
            return;
        };
        *pref = value;
        self.quick_save_prefs(Call::SetSources { sources }, true, cx);
    }

    pub(super) fn quick_advanced(
        &self,
        found: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.start.quick.advanced.open;
        let head = h_flex()
            .id("quick-advanced")
            .gap(px(look::GAP_6))
            .px(px(look::PANE_PAD_X))
            .pt(px(look::GAP_12))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                let state = &mut this.start.quick.advanced;
                state.open = !state.open;
                this.quick_load_advanced(cx);
                cx.notify();
            }))
            .child(
                icon(if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                })
                .size(px(look::ICON))
                .text_color(p.muted),
            )
            .child(section_title("Advanced", p));
        let section = v_flex().gap(px(look::GAP_6)).child(head);
        if !open {
            return section;
        }
        let busy = busy || self.start.quick.setting_prefs;
        section
            .child(look::settings_box(self.quick_source_rows(busy, p, cx), p))
            .child(look::settings_box(
                self.quick_tool_rows(found, busy, p, cx),
                p,
            ))
            .child(look::settings_box(self.quick_bioconductor_rows(p), p))
            .child(look::settings_box(
                vec![self.quick_prefs_row(found, p, cx)],
                p,
            ))
    }

    fn quick_source_rows(
        &self,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let state = &self.start.quick.advanced;
        let checking = state.loading == Some(true);
        let mut rows = vec![
            look::settings_row(
                "Where packages come from",
                Some(look::settings_detail(
                    "Sandhill picks the fastest source that works, checked once a day, including the mirrors in mainland China. Choose one to always use it.",
                    p,
                )),
            )
            .child(
                Button::new("quick-sources-check")
                    .small()
                    .label(if checking { "Checking…" } else { "Check Again" })
                    .disabled(state.loading.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.quick_list_sources(true, cx))),
            )
            .into_any_element(),
        ];
        let infos = match &state.sources {
            None => {
                rows.push(look::settings_row("Checking the sources…", None).into_any_element());
                return rows;
            }
            Some(Err(error)) => {
                rows.push(
                    look::settings_row(
                        "Couldn't check the sources",
                        Some(look::settings_detail(error.clone(), p)),
                    )
                    .into_any_element(),
                );
                return rows;
            }
            Some(Ok(infos)) => infos,
        };
        for (kind, _) in KINDS {
            let Some(info) = infos.iter().find(|info| info.kind == kind) else {
                continue;
            };
            let automatic = info.automatic;
            let mut choices = vec![
                Button::new(SharedString::from(format!("source-{kind}-auto")))
                    .small()
                    .label("Automatic")
                    .when(automatic, |b| b.primary())
                    .disabled(busy || automatic)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_set_source(kind, "auto".into(), cx)
                    })),
            ];
            for (ix, option) in info.options.iter().enumerate() {
                let chosen = !automatic && option.url == info.chosen;
                let label = match timing(option.works, option.ms) {
                    Some(speed) => format!("{} · {speed}", option.name),
                    None => option.name.clone(),
                };
                let url = option.url.clone();
                choices.push(
                    Button::new(SharedString::from(format!("source-{kind}-{ix}")))
                        .small()
                        .label(label)
                        .when(chosen, |b| b.primary())
                        .disabled(busy || chosen || !option.works)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.quick_set_source(kind, url.clone(), cx)
                        })),
                );
            }
            rows.push(
                v_flex()
                    .child(look::settings_row(
                        kind_label(&info.kind).to_owned(),
                        Some(look::settings_detail(source_status(info), p)),
                    ))
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap(px(look::GAP_6))
                            .px(px(look::PANE_PAD_X))
                            .pb(px(look::PANE_PAD_Y))
                            .children(choices),
                    )
                    .into_any_element(),
            );
        }
        if let Some(proxy) = infos.iter().find_map(|info| info.proxy.as_ref()) {
            rows.push(
                look::settings_row(
                    "Proxy",
                    Some(look::settings_detail(
                        format!("Downloads go through the system's proxy, {proxy}."),
                        p,
                    )),
                )
                .into_any_element(),
            );
        }
        rows
    }

    fn quick_tool_rows(
        &self,
        found: &QuickReadiness,
        busy: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = vec![
            look::settings_row(
                "Suggested tool",
                Some(look::settings_detail(
                    "The tool shown first when Sandhill plans an install, if it suits the environment. Every plan still lets you pick another.",
                    p,
                )),
            )
            .into_any_element(),
        ];
        for (language, label, tools) in TOOLS {
            let preferred = found
                .prefs
                .preferred_tools
                .iter()
                .find(|choice| choice.language == language)
                .map(|choice| choice.tool);
            let choices = std::iter::once(None)
                .chain(tools.iter().copied().map(Some))
                .map(|tool| {
                    let chosen = preferred == tool;
                    Button::new(SharedString::from(format!(
                        "suggested-{language}-{}",
                        tool.map_or("auto", InstallTool::id)
                    )))
                    .small()
                    .label(tool.map_or("Automatic", InstallTool::label))
                    .when(chosen, |b| b.primary())
                    .disabled(busy || chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.quick_save_prefs(
                            Call::SetPreferredTool {
                                language: language.into(),
                                tool,
                            },
                            false,
                            cx,
                        )
                    }))
                });
            rows.push(
                h_flex()
                    .flex_wrap()
                    .gap(px(look::GAP_6))
                    .px(px(look::PANE_PAD_X))
                    .pb(px(look::PANE_PAD_Y))
                    .child(div().flex_none().child(label))
                    .children(choices)
                    .into_any_element(),
            );
        }
        rows
    }

    /// Each R's Bioconductor release, shown, not chosen.
    fn quick_bioconductor_rows(&self, p: &Palette) -> Vec<AnyElement> {
        let mut rows = vec![
            look::settings_row(
                "Bioconductor release",
                Some(look::settings_detail(
                    "Each R has its own Bioconductor release. It follows the R version: BiocManager picks the release made for that R, so it moves up when R is upgraded.",
                    p,
                )),
            )
            .into_any_element(),
        ];
        let cards = match &self.start.quick.cards.list {
            Some(Ok(cards)) => cards,
            Some(Err(_)) => return rows,
            None => {
                rows.push(look::settings_row("Looking for R…", None).into_any_element());
                return rows;
            }
        };
        let mut any = false;
        for card in cards.iter().filter(|card| card.language == "r") {
            any = true;
            rows.push(
                look::settings_row(
                    card.name.clone(),
                    Some(look::settings_detail(
                        match &card.bioconductor {
                            Some(release) => format!("Bioconductor {release}"),
                            None => "Bioconductor isn't set up for this R yet.".into(),
                        },
                        p,
                    )),
                )
                .into_any_element(),
            );
        }
        if !any {
            rows.push(look::settings_row("No R found", None).into_any_element());
        }
        rows
    }

    fn quick_prefs_row(
        &self,
        readiness: &QuickReadiness,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let q = &self.start.quick;
        let label = "Install Without Asking";
        let switch = Switch::new("quick-without-asking")
            .small()
            .color(p.accent)
            .checked(readiness.prefs.without_asking)
            .accessibility_label(label)
            .disabled(q.setting_prefs || q.checking || q.busy() || q.reviewing.is_some())
            .on_change(cx.listener(|this, on: &bool, _, cx| {
                this.quick_save_prefs(
                    Call::SetInstallPrefs {
                        without_asking: *on,
                    },
                    false,
                    cx,
                )
            }));
        v_flex()
            .child(
                look::settings_row(
                    label,
                    Some(look::settings_detail(
                        "Only installs that use the same folder and tool as last time can run without asking. Add Packages always shows a plan.",
                        p,
                    )),
                )
                .child(switch),
            )
            .when_some(q.prefs_error.clone(), |d, error| {
                d.child(look::settings_row(
                    "Couldn't save this choice",
                    Some(look::settings_detail(error, p)),
                ))
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use sandhill_proto::{SourceInfo, SourceOption, SourcePrefs};

    use super::{source_pref, source_status};

    fn info(automatic: bool, chosen: &str) -> SourceInfo {
        SourceInfo {
            kind: "cran".into(),
            chosen: chosen.into(),
            automatic,
            options: vec![
                SourceOption {
                    name: "CRAN, worldwide".into(),
                    url: "https://cloud.r-project.org".into(),
                    ms: Some(420),
                    works: true,
                },
                SourceOption {
                    name: "Tsinghua University, China".into(),
                    url: "https://mirrors.tuna.tsinghua.edu.cn/CRAN".into(),
                    ms: None,
                    works: false,
                },
            ],
            proxy: None,
        }
    }

    #[test]
    fn the_status_names_the_source_and_its_speed() {
        assert_eq!(
            source_status(&info(true, "https://cloud.r-project.org")),
            "Automatic: CRAN, worldwide (420 ms), the fastest that works."
        );
        assert_eq!(
            source_status(&info(false, "https://mirrors.tuna.tsinghua.edu.cn/CRAN")),
            "Always Tsinghua University, China (not answering)."
        );
    }

    #[test]
    fn each_kind_changes_its_own_preference() {
        let mut sources = SourcePrefs::default();
        *source_pref(&mut sources, "pypi").unwrap() = "https://pypi.org/simple".into();
        assert_eq!(sources.pypi, "https://pypi.org/simple");
        assert_eq!(sources.cran, "auto");
        assert!(source_pref(&mut sources, "other").is_none());
    }
}
