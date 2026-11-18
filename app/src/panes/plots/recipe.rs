//! What a saved plot can report about its source and kernel.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local, TimeZone as _};
use gpui_kit::{App, SharedString};
use sandhill_proto::{Call, FileText, Package, Packages, SessionStatus};

use super::{PlotItem, Workbench, from_notebook, look};
use crate::daemon::Daemon;
use crate::editor::notebook::{self, Format};
use crate::panes::machine::session_label;

pub(super) const RECIPE_TIMEOUT: Duration = Duration::from_secs(3);
const STOPPED: &str = "the kernel had stopped";

pub(super) struct Recipe {
    source: Source,
    run_number: Option<u64>,
    ran_at: Option<DateTime<FixedOffset>>,
    duration: Option<Duration>,
    kernel: Option<String>,
    device: String,
    language: String,
    code: Code,
    pub(super) stopped: bool,
}

enum Source {
    Document { name: String, cell: Option<usize> },
    Console,
    Agent(String),
}

enum Code {
    Ran(SharedString),
    Current(SharedString),
    Unknown,
}

impl Workbench {
    pub(super) fn plot_recipe(&self, item: &PlotItem, host: &str, cx: &App) -> Recipe {
        let run = self.runs.get(&item.run);
        let session = self
            .console_sessions
            .live
            .iter()
            .chain(&self.machine.sessions)
            .find(|info| info.id == item.session);
        let open = item.cell.as_ref().and_then(|(path, _)| {
            self.open
                .iter()
                .chain(&self.others)
                .find(|open| open.notebook.path == *path)
        });
        let cell = open.zip(item.cell.as_ref()).and_then(|(open, (_, id))| {
            let notebook = open.notebook.source_notebook().unwrap_or(&open.notebook);
            let ix = notebook.cells.iter().position(|cell| cell.id == *id)?;
            Some((notebook, ix))
        });
        let source = match &item.cell {
            Some((path, _)) => Source::Document {
                name: basename(path).to_string(),
                cell: cell
                    .filter(|_| from_notebook(&item.cell))
                    .map(|(_, ix)| ix + 1),
            },
            None => item
                .by
                .as_ref()
                .map_or(Source::Console, |by| Source::Agent(by.clone())),
        };
        let code = run
            .and_then(|run| run.code.as_ref())
            .or(item.journal_code.as_ref())
            .map(|code| Code::Ran(code.clone()))
            .or_else(|| {
                // As on the cell's run line, missing run code does not prove a match.
                // When the exact source exists, keep it even after the cell is edited.
                let (_, ix) = cell?;
                let open = open?;
                let text = if open.notebook.quarto_source() {
                    let whole = open.editors.first()?.read(cx).text().to_string();
                    let (live, matched) = open.notebook.as_quarto_notebook(&whole).ok()?;
                    let (_, new) = matched.into_iter().find(|(old, _)| *old == ix)?;
                    SharedString::from(live.cells.into_iter().nth(new)?.source)
                } else {
                    SharedString::from(open.editors.get(ix)?.read(cx).text().to_string())
                };
                Some(Code::Current(text))
            })
            .unwrap_or(Code::Unknown);
        let run_number = run.and_then(|run| run.execution_count).or_else(|| {
            // Counts saved in a notebook describe its own outputs, not older plots.
            if run.is_none() && item.session.is_empty() {
                let (notebook, ix) = cell?;
                notebook.cells[ix].execution_count
            } else {
                None
            }
        });
        let start = run
            .and_then(|run| run.began_ms.or(run.queued_ms))
            .filter(|ms| *ms > 0);
        let ran_at = start.and_then(|ms| {
            Local
                .timestamp_millis_opt(ms)
                .single()
                .map(|at| at.fixed_offset())
        });
        let duration = run
            .and_then(|run| run.finished_ms)
            .zip(start)
            .and_then(|(end, start)| u64::try_from(end.checked_sub(start)?).ok())
            .map(Duration::from_millis);
        let language = run
            .and_then(|run| run.language)
            .filter(|language| *language != "text")
            .or_else(|| session.map(|info| info.language.as_str()))
            .or_else(|| cell.and_then(|(notebook, ix)| notebook.cell_language(ix)));
        let kernel = session
            .map(|info| session_label(info, &self.kernels))
            .or_else(|| {
                (run.is_some() || !item.session.is_empty())
                    .then(|| language.map(sandhill_proto::language_label))
                    .flatten()
            });
        let device = if host == sandhill_link::LOCAL {
            look::mac_or("This Mac", "This computer").into()
        } else {
            crate::welcome::start_screen::host_label(host)
        };
        Recipe {
            source,
            run_number,
            ran_at,
            duration,
            kernel,
            device,
            language: language.unwrap_or("text").into(),
            code,
            stopped: session.is_some_and(|info| info.status == SessionStatus::Ended),
        }
    }
}

impl Recipe {
    pub(super) fn needs_cell_number(&self) -> bool {
        matches!(self.source, Source::Document { cell: None, .. })
    }

    pub(super) fn set_cell_number(&mut self, number: usize) {
        if let Source::Document { cell, .. } = &mut self.source {
            *cell = Some(number);
        }
    }
}

/// Look up the displayed cell number if the plot's notebook is closed.
/// This does not open a tab or change what is known about the code that ran.
pub(super) fn closed_cell_number(
    daemon: &Daemon,
    path: String,
    id: String,
) -> impl Future<Output = Option<usize>> + 'static {
    let call = if notebook::format_of(&path) == Some(Format::Ipynb) {
        Call::ReadNotebook { path }
    } else {
        Call::ReadFile { path }
    };
    let read = daemon.call_within::<FileText>(call, RECIPE_TIMEOUT);
    let number = daemon.language_task(async move {
        let found = tokio::time::timeout(RECIPE_TIMEOUT, async move {
            let file = read.await?;
            tokio::task::spawn_blocking(move || -> anyhow::Result<Option<usize>> {
                let notebook = notebook::parse(&file.path, &file.text)?;
                Ok(notebook
                    .cells
                    .iter()
                    .position(|cell| cell.id == id)
                    .map(|ix| ix + 1))
            })
            .await?
        })
        .await??;
        Ok(found)
    });
    async move { number.await.ok().flatten() }
}

pub(super) fn package_probe(
    daemon: &Daemon,
    session: &str,
    stopped: bool,
) -> impl Future<Output = Result<Packages, &'static str>> + 'static {
    let read = (!stopped && !session.is_empty()).then(|| {
        daemon.call_within::<Packages>(
            Call::ListPackages {
                session: session.into(),
            },
            RECIPE_TIMEOUT,
        )
    });
    async move {
        match read {
            Some(read) => read.await.map_err(|err| {
                let message = err.to_string();
                if [
                    "no running session",
                    "no session",
                    "no such session",
                    "the session ended",
                ]
                .iter()
                .any(|part| message.contains(part))
                {
                    STOPPED
                } else if message.contains("busy running code") {
                    "the kernel was busy"
                } else if message.contains("no answer in") {
                    "the kernel did not answer in time"
                } else {
                    "could not read the package list from the kernel"
                }
            }),
            None if stopped => Err(STOPPED),
            None => Err("the original kernel session was not recorded"),
        }
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn plain(out: &mut String, text: &str) {
    for ch in text.chars() {
        if matches!(ch, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '&') {
            out.push('\\');
        }
        out.push(if ch.is_control() { ' ' } else { ch });
    }
}

/// Render a recipe without reading files, a kernel, or the clock.
pub(super) fn markdown(
    name: &str,
    saved_at: DateTime<FixedOffset>,
    recipe: &Recipe,
    packages: Result<&[Package], &str>,
) -> String {
    let mut out = String::from("# How ");
    plain(&mut out, basename(name));
    let _ = write!(
        out,
        " was made\n\nSaved {} from ",
        saved_at.format("%Y-%m-%d %H:%M")
    );
    match &recipe.source {
        Source::Document { name, cell } => {
            plain(&mut out, name);
            if let Some(cell) = cell {
                let _ = write!(out, ", cell #{cell}");
            }
        }
        Source::Console => out.push_str("the Console"),
        Source::Agent(by) => plain(&mut out, by),
    }
    if let Some(number) = recipe.run_number {
        if matches!(&recipe.source, Source::Document { cell: Some(_), .. }) {
            out.push(',');
        }
        let _ = write!(out, " by run [{number}]");
    }
    if let Some(at) = recipe.ran_at {
        let _ = write!(out, " ({}", at.format("%Y-%m-%d %H:%M"));
        if let Some(duration) = recipe.duration {
            let _ = write!(out, ", {}", crate::editor::cells::duration_text(duration));
        }
        out.push(')');
    }
    out.push_str(".\n\nKernel: ");
    if let Some(kernel) = &recipe.kernel {
        plain(&mut out, kernel);
        out.push_str(" on ");
    } else {
        out.push_str("not recorded. Saved on ");
    }
    plain(&mut out, &recipe.device);
    out.push_str(".\n\n## Code\n\n");
    match &recipe.code {
        Code::Ran(code) | Code::Current(code) => {
            if matches!(&recipe.code, Code::Current(_)) {
                out.push_str(
                    "Note: code as the cell reads now; it may differ from what made the plot.\n\n",
                );
            }
            let fence = crate::editor::doc_text::fence_for(code);
            let language = match recipe.language.as_str() {
                "python3" => "python",
                tag if tag
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_+-".contains(&byte)) =>
                {
                    tag
                }
                _ => "text",
            };
            let _ = writeln!(out, "{fence}{language}");
            out.push_str(code);
            if !code.is_empty() && !code.ends_with('\n') {
                out.push('\n');
            }
            let _ = writeln!(out, "{fence}");
        }
        Code::Unknown => out.push_str("The code that made the plot was not recorded.\n"),
    }
    out.push_str("\n## Packages loaded in the kernel\n\n");
    match packages {
        Ok(packages) => {
            let mut loaded: Vec<_> = packages.iter().filter(|p| p.loaded || p.attached).collect();
            loaded.sort_unstable_by(|a, b| {
                a.name
                    .bytes()
                    .map(|b| b.to_ascii_lowercase())
                    .cmp(b.name.bytes().map(|b| b.to_ascii_lowercase()))
                    .then_with(|| a.name.cmp(&b.name))
                    .then_with(|| a.version.cmp(&b.version))
            });
            if loaded.is_empty() {
                out.push_str("No loaded packages were reported.\n");
            }
            for package in loaded {
                out.push_str("- ");
                plain(&mut out, &package.name);
                if !package.version.is_empty() {
                    out.push(' ');
                    plain(&mut out, &package.version);
                }
                out.push('\n');
            }
        }
        Err(reason) => {
            out.push_str("Packages: not recorded, ");
            plain(&mut out, reason);
            out.push_str(".\n");
        }
    }
    out.push_str("\n## Not recorded\n\nInput files, and earlier cells this code relied on.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(value: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(value).unwrap()
    }

    fn saved_time() -> DateTime<FixedOffset> {
        time("2026-11-18T14:02:00-08:00")
    }

    fn console_recipe() -> Recipe {
        Recipe {
            source: Source::Console,
            run_number: None,
            ran_at: None,
            duration: None,
            kernel: Some("R 4.4.1 (omicverse) · IRkernel".into()),
            device: "lab-server".into(),
            language: "r".into(),
            code: Code::Ran("plot(1:10)".into()),
            stopped: false,
        }
    }

    fn package(name: &str, version: &str, loaded: bool, attached: bool) -> Package {
        Package {
            name: name.into(),
            version: version.into(),
            title: String::new(),
            library: String::new(),
            attached,
            loaded,
            base: false,
            modules: Vec::new(),
            installer: String::new(),
        }
    }

    #[test]
    fn notebook_recipe_records_source_run_and_loaded_packages_in_order() {
        let recipe = Recipe {
            source: Source::Document {
                name: "analysis.ipynb".into(),
                cell: Some(7),
            },
            run_number: Some(12),
            ran_at: Some(time("2026-11-18T13:58:00-08:00")),
            duration: Some(Duration::from_millis(4100)),
            kernel: Some("Python 3.12 (omicverse)".into()),
            device: "This Mac".into(),
            language: "python".into(),
            code: Code::Ran("plt.plot([1, 2, 3])\n".into()),
            stopped: false,
        };
        let packages = [
            package("scanpy", "1.11.2", true, false),
            package("anndata", "0.11.4", false, true),
            package("jupyterlab", "4.3.0", false, false),
            package("matplotlib", "3.10.1", true, true),
        ];
        assert_eq!(
            markdown("/reports/umap.png", saved_time(), &recipe, Ok(&packages)),
            concat!(
                "# How umap.png was made\n\n",
                "Saved 2026-11-18 14:02 from analysis.ipynb, cell #7, by run [12] (2026-11-18 13:58, 4.1s).\n\n",
                "Kernel: Python 3.12 (omicverse) on This Mac.\n\n",
                "## Code\n\n```python\nplt.plot([1, 2, 3])\n```\n\n",
                "## Packages loaded in the kernel\n\n- anndata 0.11.4\n- matplotlib 3.10.1\n- scanpy 1.11.2\n\n",
                "## Not recorded\n\nInput files, and earlier cells this code relied on.\n",
            ),
        );
    }

    #[test]
    fn console_recipe_includes_server_and_r_code() {
        let text = markdown("plot.svg", saved_time(), &console_recipe(), Ok(&[]));
        assert!(text.contains("Saved 2026-11-18 14:02 from the Console."));
        assert!(text.contains("Kernel: R 4.4.1 (omicverse) · IRkernel on lab-server."));
        assert!(text.contains("```r\nplot(1:10)\n```"));
        assert!(text.contains("No loaded packages were reported."));
    }

    #[test]
    fn stopped_kernel_still_gets_a_recipe() {
        let text = markdown("plot.svg", saved_time(), &console_recipe(), Err(STOPPED));
        assert!(text.contains("Packages: not recorded, the kernel had stopped."));
        assert!(text.contains("plot(1:10)"));
        assert!(text.contains("## Not recorded"));
    }

    #[test]
    fn current_code_is_marked_as_unverified_and_keeps_its_fence() {
        let recipe = Recipe {
            source: Source::Document {
                name: "reopened.ipynb".into(),
                cell: Some(3),
            },
            language: "python3".into(),
            code: Code::Current("note = '''\n```\n'''\n\n".into()),
            kernel: None,
            ..console_recipe()
        };
        let text = markdown(
            "plot.svg",
            saved_time(),
            &recipe,
            Err("the original kernel session was not recorded"),
        );
        assert!(text.contains("code as the cell reads now; it may differ from what made the plot"));
        assert!(text.contains("````python\nnote = '''\n```\n'''\n\n````"));
        assert!(text.contains("Kernel: not recorded. Saved on lab-server."));
    }
}
