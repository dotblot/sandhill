//! The Packages pane, as the Tauri app has it (`PackagesPane.tsx`,
//! `model/help.ts`): what's installed for the active session's language (R's
//! libraries, or the Python environment), with what's attached or imported
//! first. A click opens a package's help. Installing, updating and removing
//! run in the Console, where their output shows, and the list is read again
//! after. Updates are checked when asked, as that reaches the repositories
//! from the session: CRAN and Bioconductor for R, PyPI and conda's channels
//! for Python.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, Package, PackageUpdate, PackageUpdates, Packages as Listed};

use crate::help::HelpRequest;
use crate::look::{self, Palette, RADIUS, capsule, icon, icon_button};
use crate::workbench::{TopTab, Workbench};

/// How long after a run finishes the list is read again.
const REFRESH: Duration = Duration::from_millis(250);
/// What the daemon says when the session runs other code.
const BUSY: &str = "the session is busy running code";

pub(crate) struct Packages {
    filter: Entity<InputState>,
    names: Entity<InputState>,
    /// The session the list is for.
    session: Option<String>,
    listed: Option<Listed>,
    error: Option<String>,
    /// The list needs reading again once the pane shows.
    stale: bool,
    loading: bool,
    installing: bool,
    conda: bool,
    invalid: bool,
    /// What checking for updates found, or why it couldn't.
    updates: Option<Result<PackageUpdates, String>>,
    checking: bool,
    /// A package whose remove button was pressed once: pressing it again
    /// removes it.
    confirm_remove: Option<String>,
    generation: u64,
}

/// Makes the Packages pane's state, with its filter and install boxes.
pub(crate) fn new_packages(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Packages, Vec<Subscription>) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter packages"));
    let names = cx.new(|cx| InputState::new(window, cx).placeholder("Packages to install"));
    let subscriptions = vec![
        cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if let InputEvent::Change = event {
                cx.notify();
            }
        }),
        cx.subscribe_in(
            &names,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.packages.invalid = false;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.install_packages(window, cx),
                _ => {}
            },
        ),
    ];
    let packages = Packages {
        filter,
        names,
        session: None,
        listed: None,
        error: None,
        stale: true,
        loading: false,
        installing: false,
        conda: false,
        invalid: false,
        updates: None,
        checking: false,
        confirm_remove: None,
        generation: 0,
    };
    (packages, subscriptions)
}

fn is_python(language: &str) -> bool {
    language.to_ascii_lowercase().starts_with("python")
}

/// What a package's row says of it: attached (or imported), or loaded.
pub(crate) fn package_status(pkg: &Package, language: &str) -> &'static str {
    if pkg.attached {
        if language.eq_ignore_ascii_case("r") {
            "attached"
        } else {
            "imported"
        }
    } else if pkg.loaded {
        "loaded"
    } else {
        ""
    }
}

/// Packages whose name or title has `text`, names first.
pub(crate) fn filter_packages<'a>(packages: &'a [Package], text: &str) -> Vec<&'a Package> {
    let q = text.trim().to_lowercase();
    if q.is_empty() {
        return packages.iter().collect();
    }
    let named = |p: &Package| p.name.to_lowercase().contains(&q);
    let mut out: Vec<&Package> = packages.iter().filter(|p| named(p)).collect();
    out.extend(
        packages
            .iter()
            .filter(|p| !named(p) && p.title.to_lowercase().contains(&q)),
    );
    out
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// A pip requirement: `name`, `name[extra,other]`, then version clauses
/// like `>=1.10,<2`.
fn is_requirement(text: &str) -> bool {
    let mut rest = text;
    let Some(first) = rest.chars().next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let end = rest
        .find(|c: char| !(is_word(c) || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest = &rest[end..];
    if let Some(after) = rest.strip_prefix('[') {
        let Some(close) = after.find(']') else {
            return false;
        };
        let extras = &after[..close];
        if extras.is_empty()
            || !extras
                .chars()
                .all(|c| is_word(c) || matches!(c, '.' | ',' | '-'))
        {
            return false;
        }
        rest = &after[close + 1..];
    }
    if rest.is_empty() {
        return true;
    }
    rest.split(',').all(|clause| {
        let Some(op) = clause.chars().next().filter(|c| "<>=!~".contains(*c)) else {
            return false;
        };
        let version = clause[op.len_utf8()..]
            .strip_prefix('=')
            .unwrap_or(&clause[op.len_utf8()..]);
        !version.is_empty()
            && version
                .chars()
                .all(|c| is_word(c) || matches!(c, '.' | '*' | '+' | '-'))
    })
}

fn is_r_package(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

/// The names typed to install: R's package names (separated by spaces or
/// commas), or pip's requirements (`scanpy[leiden]>=1.10`). None when one
/// isn't a name.
pub(crate) fn package_names(text: &str, language: &str) -> Option<Vec<String>> {
    let python = is_python(language);
    let names: Vec<String> = text
        .split(|c: char| c.is_whitespace() || (!python && c == ','))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect();
    let valid = |n: &String| {
        if python {
            is_requirement(n)
        } else {
            is_r_package(n)
        }
    };
    (!names.is_empty() && names.iter().all(valid)).then_some(names)
}

fn r_vector(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("{n:?}")).collect();
    if quoted.len() == 1 {
        quoted[0].clone()
    } else {
        format!("c({})", quoted.join(", "))
    }
}

/// The Console code that installs packages: BiocManager when it is installed
/// (so Bioconductor's packages work too), otherwise install.packages(), with
/// a CRAN mirror set if there isn't one; pip, or conda when asked, in Python.
pub(crate) fn install_code(
    language: &str,
    names: &[String],
    cran: bool,
    bioconductor: bool,
    conda: bool,
) -> String {
    if is_python(language) {
        return if conda {
            format!(
                "%conda install -y -c conda-forge -c bioconda {}",
                names.join(" ")
            )
        } else {
            format!("%pip install {}", names.join(" "))
        };
    }
    let mirror = if cran {
        ""
    } else {
        "options(repos = c(CRAN = \"https://cloud.r-project.org\"))\n"
    };
    if bioconductor {
        format!(
            "{mirror}BiocManager::install({}, update = FALSE, ask = FALSE)",
            r_vector(names)
        )
    } else {
        format!("{mirror}install.packages({})", r_vector(names))
    }
}

/// The Console code that updates packages: R's as they are installed;
/// in Python, conda's own with conda (which keeps the environment
/// consistent) and the rest with pip.
pub(crate) fn update_code(
    language: &str,
    packages: &[&Package],
    cran: bool,
    bioconductor: bool,
) -> String {
    let names = |list: &[&&Package]| {
        list.iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    if !is_python(language) {
        let names: Vec<String> = packages.iter().map(|p| p.name.clone()).collect();
        return install_code(language, &names, cran, bioconductor, false);
    }
    let (conda, pip): (Vec<&&Package>, Vec<&&Package>) =
        packages.iter().partition(|p| p.installer == "conda");
    let mut lines = Vec::new();
    if !conda.is_empty() {
        lines.push(format!("%conda update -y {}", names(&conda)));
    }
    if !pip.is_empty() {
        lines.push(format!("%pip install -U {}", names(&pip)));
    }
    lines.join("\n")
}

/// The Console code that removes packages: conda's own with conda's tool.
pub(crate) fn remove_code(language: &str, packages: &[&Package]) -> String {
    if !is_python(language) {
        let names: Vec<String> = packages.iter().map(|p| p.name.clone()).collect();
        return format!("remove.packages({})", r_vector(&names));
    }
    let (conda, pip): (Vec<&&Package>, Vec<&&Package>) =
        packages.iter().partition(|p| p.installer == "conda");
    let names = |list: &[&&Package]| {
        list.iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut lines = Vec::new();
    if !conda.is_empty() {
        lines.push(format!("%conda remove -y {}", names(&conda)));
    }
    if !pip.is_empty() {
        lines.push(format!("%pip uninstall -y {}", names(&pip)));
    }
    lines.join("\n")
}

/// The topic to open for a package: R's package overview, Python's module.
pub(crate) fn package_topic(pkg: &Package, language: &str) -> HelpRequest {
    if language.eq_ignore_ascii_case("r") {
        return HelpRequest {
            topic: String::new(),
            package: Some(pkg.name.clone()),
            search: false,
        };
    }
    let wanted = pkg.name.to_lowercase().replace('-', "_");
    let module = pkg
        .modules
        .iter()
        .find(|m| m.to_lowercase() == wanted)
        .or_else(|| pkg.modules.iter().find(|m| !m.starts_with('_')))
        .cloned()
        .unwrap_or_else(|| pkg.name.replace('-', "_"));
    HelpRequest {
        topic: module,
        package: None,
        search: false,
    }
}

/// Updates for what's installed now, by name: one updated since drops out.
fn pending<'a>(
    updates: &'a PackageUpdates,
    listed: Option<&Listed>,
) -> Vec<(&'a str, &'a PackageUpdate)> {
    let now: HashMap<&str, &str> = listed
        .map(|l| {
            l.packages
                .iter()
                .map(|p| (p.name.as_str(), p.version.as_str()))
                .collect()
        })
        .unwrap_or_default();
    let mut out: Vec<(&str, &PackageUpdate)> = Vec::new();
    for u in &updates.updates {
        if now.get(u.name.as_str()) == Some(&u.installed.as_str())
            && !out.iter().any(|(n, _)| *n == u.name)
        {
            out.push((u.name.as_str(), u));
        }
    }
    out
}

fn short_error(err: &str) -> String {
    let err = err.strip_prefix("daemon error: ").unwrap_or(err);
    err.strip_prefix("could not check for updates: ")
        .unwrap_or(err)
        .to_string()
}

impl Workbench {
    fn language_name(&self) -> &'static str {
        if self.active_language().is_some_and(|l| is_python(&l)) {
            "Python"
        } else {
            "R"
        }
    }

    /// Reads the list when the pane shows and it's out of date, or the
    /// session changed. Called while drawing.
    pub(crate) fn sync_packages(&mut self, cx: &mut Context<Self>) {
        let session = self.active_session().map(str::to_string);
        if session != self.packages.session {
            self.packages.generation += 1;
            self.packages.session = session;
            self.packages.listed = None;
            self.packages.error = None;
            self.packages.updates = None;
            self.packages.conda = false;
            self.packages.confirm_remove = None;
            self.packages.stale = true;
        }
        if self.packages.stale
            && !self.packages.loading
            && self.show_files
            && self.top_tab == TopTab::Packages
        {
            self.refresh_packages(cx);
        }
    }

    /// A run in the session finished: the list may have changed.
    pub(crate) fn packages_run_finished(&mut self, cx: &mut Context<Self>) {
        self.packages.stale = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
    }

    pub(crate) fn refresh_packages(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.active_session().map(str::to_string))
        else {
            return;
        };
        self.packages.stale = false;
        self.packages.loading = true;
        self.packages.generation += 1;
        let mine = self.packages.generation;
        let read = daemon.call::<Listed>(Call::ListPackages { session });
        cx.spawn(async move |this, cx| {
            let read = read.await;
            this.update(cx, |this, cx| {
                if this.packages.generation != mine {
                    return;
                }
                this.packages.loading = false;
                match read {
                    Ok(read) => {
                        this.packages.listed = Some(read);
                        this.packages.error = None;
                    }
                    // Busy with other code: read again when it finishes.
                    Err(err) if format!("{err:#}").contains(BUSY) => {}
                    Err(err) => this.packages.error = Some(format!("{err:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn check_updates(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(session)) =
            (self.daemon(), self.active_session().map(str::to_string))
        else {
            return;
        };
        self.packages.checking = true;
        let name = self.language_name();
        let found = daemon.call::<PackageUpdates>(Call::PackageUpdates {
            session: session.clone(),
        });
        cx.spawn(async move |this, cx| {
            let found = found.await;
            this.update(cx, |this, cx| {
                this.packages.checking = false;
                if this.packages.session.as_deref() != Some(&session) {
                    return;
                }
                this.packages.updates = Some(found.map_err(|err| {
                    let err = format!("{err:#}");
                    if err.contains(BUSY) {
                        format!("{name} is busy; check again once it finishes.")
                    } else {
                        format!("Could not check for updates: {}", short_error(&err))
                    }
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn update_packages(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        let Some(listed) = &self.packages.listed else {
            return;
        };
        if names.is_empty() {
            return;
        }
        let language = self.active_language().unwrap_or_default();
        let picked: Vec<&Package> = listed
            .packages
            .iter()
            .filter(|p| names.contains(&p.name))
            .collect();
        let code = update_code(&language, &picked, listed.cran, listed.bioconductor);
        self.run_in_console(code, cx);
    }

    fn install_packages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let language = self.active_language().unwrap_or_default();
        let typed = self.packages.names.read(cx).value().to_string();
        let Some(picked) = package_names(&typed, &language) else {
            self.packages.invalid = true;
            cx.notify();
            return;
        };
        let Some(listed) = &self.packages.listed else {
            return;
        };
        let code = install_code(
            &language,
            &picked,
            listed.cran,
            listed.bioconductor,
            self.packages.conda,
        );
        self.run_in_console(code, cx);
        self.packages
            .names
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.packages.invalid = false;
        self.packages.installing = false;
        cx.notify();
    }

    fn remove_package(&mut self, name: String, cx: &mut Context<Self>) {
        if self.packages.confirm_remove.as_deref() != Some(&name) {
            self.packages.confirm_remove = Some(name);
            cx.notify();
            return;
        }
        self.packages.confirm_remove = None;
        let language = self.active_language().unwrap_or_default();
        let Some(pkg) = self
            .packages
            .listed
            .as_ref()
            .and_then(|l| l.packages.iter().find(|p| p.name == name))
        else {
            return;
        };
        let code = remove_code(&language, &[pkg]);
        self.run_in_console(code, cx);
        cx.notify();
    }

    /// The buttons on the right of the Packages tab's strip.
    pub(crate) fn package_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let listed = self.packages.listed.is_some();
        let mut tools = vec![
            icon_button("check-updates", "doc.download", &p, false)
                .when(!listed || self.packages.checking, |d| d.opacity(0.35))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Check for updates")
                        .build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.packages.listed.is_some() && !this.packages.checking {
                        this.check_updates(cx)
                    }
                }))
                .into_any_element(),
        ];
        tools.push(
            icon_button("install-packages", "add", &p, false)
                .when(self.packages.installing, |d| d.bg(p.ink(10.)))
                .when(!listed, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.packages.listed.is_some() {
                        this.packages.installing = !this.packages.installing;
                        if this.packages.installing {
                            this.packages
                                .names
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    }
                }))
                .into_any_element(),
        );
        tools.push(
            icon_button("refresh-packages", "refresh", &p, false)
                .when(self.active_session().is_none(), |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.refresh_packages(cx)))
                .into_any_element(),
        );
        tools
    }

    /// The Packages tab's body.
    pub(crate) fn render_packages(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let language = self.active_language().unwrap_or_default();
        let python = is_python(&language);
        let name = self.language_name();
        let listed = self.packages.listed.as_ref();
        let count = listed.map(|l| {
            let n = l.packages.len();
            format!("{n} installed")
        });
        let filter_bar = h_flex()
            .flex_none()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(
                div().flex_1().child(
                    Input::new(&self.packages.filter)
                        .small()
                        .prefix(icon("filter").size(px(13.))),
                ),
            )
            .when_some(count, |d, c| {
                d.child(
                    div()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child(c),
                )
            });
        let mut top: Vec<AnyElement> = vec![filter_bar.into_any_element()];
        if self.packages.installing && listed.is_some() {
            top.push(self.render_install(python, cx));
        }
        if let Some(bar) = self.render_updates(cx) {
            top.push(bar);
        }
        let mut body: Vec<AnyElement> = Vec::new();
        let muted_note = |text: String, color: Hsla| {
            div()
                .px(px(14.))
                .py(px(6.))
                .text_color(color)
                .child(text)
                .into_any_element()
        };
        if self.active_session().is_none() {
            body.push(muted_note(
                "Start R or Python in the Console, and its installed packages show here.".into(),
                p.muted,
            ));
        } else {
            if let Some(err) = &self.packages.error {
                body.push(muted_note(err.clone(), p.err));
            }
            if listed.is_none() && self.packages.loading {
                body.push(muted_note(format!("Reading {name}'s packages…"), p.muted));
            }
            if let Some(listed) = listed {
                let filter = self.packages.filter.read(cx).value().to_string();
                let shown = filter_packages(&listed.packages, &filter);
                if shown.is_empty() {
                    body.push(muted_note(
                        format!("No packages match \"{}\".", filter.trim()),
                        p.muted,
                    ));
                }
                let updates: Vec<(&str, &PackageUpdate)> = match &self.packages.updates {
                    Some(Ok(found)) => pending(found, Some(listed)),
                    _ => Vec::new(),
                };
                let (used, rest): (Vec<&Package>, Vec<&Package>) =
                    shown.into_iter().partition(|p| p.attached);
                for (title, group) in [
                    (if python { "Imported" } else { "Attached" }, used),
                    ("Installed", rest),
                ] {
                    if group.is_empty() {
                        continue;
                    }
                    body.push(
                        div()
                            .px(px(14.))
                            .pt(px(8.))
                            .pb(px(2.))
                            .text_size(px(look::SMALL))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(p.faint)
                            .child(title)
                            .into_any_element(),
                    );
                    for pkg in group {
                        let update = updates
                            .iter()
                            .find(|(n, _)| *n == pkg.name)
                            .map(|(_, u)| *u);
                        body.push(self.package_row(pkg, &language, update, cx));
                    }
                }
            }
        }
        v_flex()
            .size_full()
            .children(top)
            .child(
                div()
                    .id("packages")
                    .flex_1()
                    .min_h_0()
                    .pb(px(4.))
                    .overflow_y_scroll()
                    .children(body),
            )
            .into_any_element()
    }

    fn package_row(
        &self,
        pkg: &Package,
        language: &str,
        update: Option<&PackageUpdate>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let hover = p.ink(7.);
        let asked = package_topic(pkg, language);
        let confirming = self.packages.confirm_remove.as_deref() == Some(pkg.name.as_str());
        let remove_name = pkg.name.clone();
        let update_name = pkg.name.clone();
        let status = package_status(pkg, language);
        h_flex()
            .id(SharedString::from(format!("pkg-{}", pkg.name)))
            .mx(px(6.))
            .px(px(8.))
            .gap_2()
            .h(px(24.))
            .rounded(px(RADIUS))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .hover(move |d| d.bg(hover))
            .on_click(
                cx.listener(move |this, _, window, cx| this.ask_help(asked.clone(), window, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(p.faint)
                    .child(icon("package").size(px(13.))),
            )
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.text)
                    .child(pkg.name.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(p.faint)
                    .child(pkg.version.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .italic()
                    .text_color(p.muted)
                    .child(pkg.title.clone()),
            )
            .map(|d| match update {
                Some(u) => d.child(
                    div()
                        .id(SharedString::from(format!("pkg-update-{}", pkg.name)))
                        .flex_none()
                        .px(px(6.))
                        .rounded_full()
                        .text_size(px(look::SMALL))
                        .text_color(p.accent)
                        .hover(|d| d.bg(p.ink(10.)))
                        .child(format!("update {}", u.available))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.update_packages(vec![update_name.clone()], cx)
                        })),
                ),
                None => d.child(
                    div()
                        .flex_none()
                        .text_size(px(look::SMALL))
                        .text_color(p.faint)
                        .child(status),
                ),
            })
            .when(!pkg.base, |d| {
                d.child(if confirming {
                    div()
                        .id(SharedString::from(format!("pkg-remove-{}", pkg.name)))
                        .flex_none()
                        .px(px(6.))
                        .rounded_full()
                        .text_size(px(look::SMALL))
                        .text_color(p.err)
                        .hover(|d| d.bg(p.ink(10.)))
                        .child("Remove?")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.remove_package(remove_name.clone(), cx)
                        }))
                        .into_any_element()
                } else {
                    icon_button(
                        SharedString::from(format!("pkg-remove-{}", pkg.name)),
                        "delete",
                        &p,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_package(remove_name.clone(), cx)
                    }))
                    .into_any_element()
                })
            })
            .into_any_element()
    }

    fn render_install(&self, python: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let listed = self.packages.listed.as_ref();
        let how = if python {
            if self.packages.conda {
                "Runs %conda install in the Console, with conda-forge and bioconda"
            } else {
                "Runs %pip install in the Console"
            }
        } else if listed.is_some_and(|l| l.bioconductor) {
            "Runs BiocManager::install() in the Console: CRAN and Bioconductor"
        } else {
            "Runs install.packages() in the Console: CRAN. For Bioconductor, install BiocManager first"
        };
        let conda_ok = python && listed.is_some_and(|l| l.conda);
        let typed = !self.packages.names.read(cx).value().trim().is_empty();
        let choice =
            |id: &'static str, label: &'static str, conda: bool, cx: &mut Context<Self>| {
                Self::tab(id, None, label, self.packages.conda == conda, &p).on_click(cx.listener(
                    move |this, _, _, cx| {
                        this.packages.conda = conda;
                        cx.notify();
                    },
                ))
            };
        v_flex()
            .flex_none()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .border_b(px(0.5))
            .border_color(p.glass_edge)
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.packages.names).small()),
                    )
                    .when(conda_ok, |d| {
                        d.child(
                            capsule(&p, 22.)
                                .gap(px(1.))
                                .child(choice("install-pip", "pip", false, cx))
                                .child(choice("install-conda", "conda", true, cx)),
                        )
                    })
                    .child(
                        Button::new("install-go")
                            .xsmall()
                            .primary()
                            .label("Install")
                            .disabled(!typed)
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.install_packages(window, cx)
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(look::SMALL))
                    .text_color(if self.packages.invalid {
                        p.err
                    } else {
                        p.faint
                    })
                    .child(if self.packages.invalid {
                        "Type package names, separated by spaces."
                    } else {
                        how
                    }),
            )
            .into_any_element()
    }

    fn render_updates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        if !self.packages.checking && self.packages.updates.is_none() {
            return None;
        }
        let listed = self.packages.listed.as_ref();
        let mut failed = false;
        let mut names: Vec<String> = Vec::new();
        let python = self.active_language().is_some_and(|l| is_python(&l));
        let text = if self.packages.checking {
            if python && listed.is_some_and(|l| l.conda) {
                "Checking conda's channels and PyPI for updates…".to_string()
            } else if python {
                "Checking PyPI for updates…".to_string()
            } else if listed.is_some_and(|l| l.bioconductor) {
                "Checking CRAN and Bioconductor for updates…".to_string()
            } else {
                "Checking CRAN for updates…".to_string()
            }
        } else {
            match self.packages.updates.as_ref()? {
                Err(err) => {
                    failed = true;
                    err.clone()
                }
                Ok(found) => {
                    names = pending(found, listed)
                        .iter()
                        .map(|(n, _)| n.to_string())
                        .collect();
                    let missed = if found.unreached.is_empty() {
                        String::new()
                    } else {
                        format!(" Couldn't reach {}.", found.unreached.join(", "))
                    };
                    let n = names.len();
                    if n == 0 {
                        format!("Everything is up to date.{missed}")
                    } else {
                        format!(
                            "{n} update{}: {}.{missed}",
                            if n == 1 { "" } else { "s" },
                            names.join(", ")
                        )
                    }
                }
            }
        };
        let checking = self.packages.checking;
        Some(
            h_flex()
                .flex_none()
                .gap(px(6.))
                .px(px(14.))
                .py(px(5.))
                .border_b(px(0.5))
                .border_color(p.glass_edge)
                .text_size(px(look::SMALL))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(if failed { p.err } else { p.muted })
                        .child(text),
                )
                .when(!checking && !names.is_empty(), |d| {
                    d.child(
                        Button::new("update-all")
                            .xsmall()
                            .label("Update all")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.update_packages(names.clone(), cx)
                            })),
                    )
                })
                .when(!checking, |d| {
                    d.child(
                        icon_button("hide-updates", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.packages.updates = None;
                                cx.notify();
                            },
                        )),
                    )
                })
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        filter_packages, install_code, package_names, package_topic, remove_code, update_code,
    };
    use sandhill_proto::Package;

    fn pkg(name: &str, title: &str) -> Package {
        serde_json::from_value(serde_json::json!({
            "name": name, "version": "1.0", "title": title
        }))
        .unwrap()
    }

    #[test]
    fn names_are_checked() {
        assert_eq!(
            package_names("DESeq2, harmony", "R"),
            Some(vec!["DESeq2".into(), "harmony".into()])
        );
        assert_eq!(package_names("DESeq2 ;rm", "R"), None);
        assert_eq!(
            package_names("scanpy[leiden]>=1.10,<2 numpy", "python"),
            Some(vec!["scanpy[leiden]>=1.10,<2".into(), "numpy".into()])
        );
        assert_eq!(package_names("scanpy;import os", "python"), None);
        assert_eq!(package_names("  ", "python"), None);
    }

    #[test]
    fn code_to_install_and_remove() {
        let names = vec!["DESeq2".to_string()];
        assert_eq!(
            install_code("R", &names, true, true, false),
            "BiocManager::install(\"DESeq2\", update = FALSE, ask = FALSE)"
        );
        assert_eq!(
            install_code("R", &["a".into(), "b".into()], false, false, false),
            "options(repos = c(CRAN = \"https://cloud.r-project.org\"))\ninstall.packages(c(\"a\", \"b\"))"
        );
        assert_eq!(
            install_code("python", &names, false, false, true),
            "%conda install -y -c conda-forge -c bioconda DESeq2"
        );
        let mut a = pkg("numpy", "");
        a.installer = "conda".into();
        let b = pkg("scanpy", "");
        assert_eq!(
            remove_code("python", &[&a, &b]),
            "%conda remove -y numpy\n%pip uninstall -y scanpy"
        );
        assert_eq!(remove_code("R", &[&b]), "remove.packages(\"scanpy\")");
        assert_eq!(
            update_code("python", &[&a, &b], false, false),
            "%conda update -y numpy\n%pip install -U scanpy"
        );
        assert_eq!(
            update_code("R", &[&b], true, false),
            "install.packages(\"scanpy\")"
        );
    }

    #[test]
    fn filter_and_topic() {
        let list = vec![
            pkg("ggplot2", "Grammar of graphics"),
            pkg("grid", "The grid ggplot uses"),
        ];
        let found: Vec<&str> = filter_packages(&list, "ggplot")
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(found, ["ggplot2", "grid"]);
        let mut sk = pkg("scikit-learn", "");
        sk.modules = vec!["_distributor".into(), "sklearn".into()];
        assert_eq!(package_topic(&sk, "python").topic, "sklearn");
        assert_eq!(
            package_topic(&sk, "R").package.as_deref(),
            Some("scikit-learn")
        );
    }
}
