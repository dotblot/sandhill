//! Quarto Preview: `quarto preview` of a document (`.qmd`, `.Rmd`) in a
//! terminal of its own, which renders it again each time it is saved and
//! serves it on this machine's `127.0.0.1`, where the app's Viewer reaches
//! it as it does an app. See `Call::PreviewDocument`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use sandhill_proto::{AgentOptions, PreviewInfo, TERMINAL_PREVIEW};

use crate::kernelspecs::{self, Search};
use crate::terminal::{Launch, Terminals};

/// What to say when Quarto isn't found.
pub fn install_advice() -> String {
    let how = if cfg!(target_os = "macos") {
        "download the installer there, or run `brew install --cask quarto`"
    } else {
        "on Ubuntu, download the .deb there and run `sudo apt install ./quarto-*-linux-amd64.deb`"
    };
    format!(
        "Quarto is not installed on this machine. Install it from \
         https://quarto.org/docs/get-started/ ({how}), or into a conda environment with \
         `mamba install -c conda-forge quarto`; or set QUARTO_PATH to where it is."
    )
}

/// How much of Quarto's output to give when it stops before showing the page.
const TAIL: u64 = 2000;
/// How much of its output to read for where it says to browse.
const SAID: u64 = 64 << 10;
/// How long after its page answers Quarto has to say where it is, else `/`.
const SAYS_WHERE: Duration = Duration::from_secs(3);

/// Whether a file is a document Quarto previews.
pub fn previewable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("qmd") || e.eq_ignore_ascii_case("rmd"))
}

/// Quarto on this machine: `$QUARTO_PATH` (as RStudio and the quarto R
/// package read it), then the `PATH` and the usual places, then a conda
/// environment's, then the copy RStudio or Positron bundle.
pub fn find_quarto(search: &Search) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("QUARTO_PATH").map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    if let Some(path) = crate::agents::find("quarto") {
        return Some(path);
    }
    let mut candidates: Vec<PathBuf> = vec![
        "/opt/quarto/bin/quarto".into(),
        "/usr/local/bin/quarto".into(),
        "/Applications/quarto/bin/quarto".into(),
    ];
    candidates.extend(
        kernelspecs::conda_envs(search)
            .into_iter()
            .map(|env| env.join("bin/quarto")),
    );
    candidates.extend([
        "/usr/lib/rstudio-server/bin/quarto/bin/quarto".into(),
        "/usr/lib/rstudio/resources/app/bin/quarto/bin/quarto".into(),
        "/Applications/RStudio.app/Contents/Resources/app/quarto/bin/quarto".into(),
        "/Applications/Positron.app/Contents/Resources/app/quarto/bin/quarto".into(),
    ]);
    candidates.into_iter().find(|p| p.is_file())
}

/// The terminal to run `quarto preview` of `file` in, serving it at `port`.
pub fn launch(quarto: &Path, file: &Path, port: u16, project: Option<String>) -> Launch {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Launch {
        kind: TERMINAL_PREVIEW.into(),
        title: format!("Preview {name}"),
        program: quarto.to_path_buf(),
        args: vec![
            "preview".into(),
            file.to_string_lossy().into_owned(),
            "--no-browser".into(),
            "--port".into(),
            port.to_string(),
        ],
        env: Vec::new(),
        cwd: file.parent().unwrap_or(Path::new("/")).to_path_buf(),
        project,
        options: AgentOptions::default(),
        size: None,
        job: None,
    }
}

/// The port of a preview of `file` that a terminal runs, from its command.
pub fn port_of(command: &[String], file: &Path) -> Option<u16> {
    let file = file.to_string_lossy();
    if command.get(1).map(String::as_str) != Some("preview")
        || command.get(2).map(String::as_str) != Some(&file)
    {
        return None;
    }
    let at = command.iter().position(|a| a == "--port")?;
    command.get(at + 1)?.parse().ok()
}

/// The preview of `file` still running, if there is one.
pub fn running(terminals: &Terminals, file: &Path) -> Option<(String, u16)> {
    terminals
        .list()
        .into_iter()
        .filter(|t| t.kind == TERMINAL_PREVIEW && t.running)
        .find_map(|t| {
            let port = port_of(&terminals.command(&t.id).ok()?, file)?;
            Some((t.id, port))
        })
}

/// A port nothing on this machine listens on now.
pub fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// Waits until the preview's page answers; fails with Quarto's last words
/// if it stops first. The page is where Quarto says to browse: a
/// document in a Quarto project is a page of the project's site.
pub async fn ready(terminals: &Terminals, id: &str, port: u16) -> Result<PreviewInfo> {
    let mut answered = None;
    loop {
        let said = || -> Result<String> {
            Ok(crate::mcp::text::strip_ansi(&terminals.output_tail(id, SAID)?).replace('\r', ""))
        };
        if answered.is_none()
            && tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
        {
            answered = Some(tokio::time::Instant::now());
        }
        if let Some(at) = answered {
            let path = browse_path(&said()?, port);
            if path.is_some() || at.elapsed() > SAYS_WHERE {
                return Ok(PreviewInfo {
                    url: format!("http://127.0.0.1:{port}{}", path.as_deref().unwrap_or("/")),
                    terminal: terminals.info(id)?,
                });
            }
        } else if !terminals.info(id)?.running {
            let said = said()?;
            let mut start = said.len().saturating_sub(TAIL as usize);
            while !said.is_char_boundary(start) {
                start += 1;
            }
            let tail = &said[start..];
            bail!("Quarto stopped before showing the page:\n{}", tail.trim());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Where Quarto says to browse at `port`, from its output: `Browse at
/// http://localhost:4200/report.html` gives `/report.html`.
fn browse_path(said: &str, port: u16) -> Option<String> {
    let line = said.lines().rev().find(|l| l.contains("Browse at http"))?;
    let url = line[line.find("http")?..].split_whitespace().next()?;
    let rest = url.split_once("://")?.1;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    host.ends_with(&format!(":{port}"))
        .then(|| format!("/{path}"))
}

/// Quarto's environment for a project: its env files' variables, as its
/// kernels get, and the Python of its Python kernel (`QUARTO_PYTHON`), so a
/// document's Python chunks run where the project's do.
pub fn env(project: &Path) -> Vec<(String, String)> {
    let mut env = crate::projects::kernel_env(project);
    let python = crate::projects::settings::read(project)
        .ok()
        .flatten()
        .and_then(|s| s.python);
    if let Some(name) = python {
        let search = Search::from_env().in_project(Some(project));
        if let Some(spec) = kernelspecs::find(&search, &name)
            && let Some(exe) = spec.kernelspec.argv.first()
            && Path::new(exe)
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("python"))
        {
            env.push(("QUARTO_PYTHON".into(), exe.clone()));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarto_documents_are_previewed() {
        assert!(previewable(Path::new("/p/report.qmd")));
        assert!(previewable(Path::new("/p/qc.Rmd")));
        assert!(!previewable(Path::new("/p/qc.R")));
        assert!(!previewable(Path::new("/p/umap.ipynb")));
    }

    #[test]
    fn the_page_is_where_quarto_says_to_browse() {
        let said = "Preparing to preview\nWatching files for changes\nBrowse at http://localhost:4312/analysis/report.html\n";
        assert_eq!(
            browse_path(said, 4312).as_deref(),
            Some("/analysis/report.html")
        );
        assert_eq!(
            browse_path("Browse at http://localhost:4312/\n", 4312).as_deref(),
            Some("/")
        );
        assert_eq!(
            browse_path("Browse at http://localhost:4312\n", 4312).as_deref(),
            Some("/")
        );
        // Another preview's.
        assert_eq!(browse_path(said, 9999), None);
        assert_eq!(browse_path("Output created: report.html\n", 4312), None);
    }

    #[test]
    fn a_running_preview_is_found_by_its_file() {
        let launch = launch(
            Path::new("/opt/quarto/bin/quarto"),
            Path::new("/p/report.qmd"),
            4312,
            None,
        );
        assert_eq!(launch.cwd, Path::new("/p"));
        assert_eq!(launch.title, "Preview report.qmd");
        let mut command = vec![launch.program.to_string_lossy().into_owned()];
        command.extend(launch.args);
        assert_eq!(port_of(&command, Path::new("/p/report.qmd")), Some(4312));
        assert_eq!(port_of(&command, Path::new("/p/other.qmd")), None);
        assert_eq!(
            port_of(&["bash".into(), "-l".into()], Path::new("/p/report.qmd")),
            None
        );
    }
}
