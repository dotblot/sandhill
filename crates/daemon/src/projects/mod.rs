//! Projects: folders the app opens as a unit, like an RStudio project.
//!
//! The daemon remembers which folders were opened, and when, and keeps each
//! project's personal state (open tabs and the like) in its journal rather
//! than in the folder, so opening a shared repository leaves no trace in it.
//! Settings to share with collaborators go in `.sandhill/project.toml`
//! (see [`settings`]).

pub mod envfiles;
pub mod environments;
pub mod settings;

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use sandhill_proto::{ProjectInfo, ProjectSettings, ProjectSetup};
use serde_json::Value;

use crate::files;
use crate::journal::Journal;
use crate::kernelspecs::{self, Search};

/// How many projects `list_recent_projects` returns.
const RECENT_LIMIT: usize = 50;

/// The form a project path is stored and compared in: absolute, `~`
/// expanded, without `.` components or a trailing slash.
pub fn normalize(path: &str) -> Result<String> {
    let resolved = files::resolve(path)?;
    let clean: PathBuf = resolved
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    Ok(clean.to_string_lossy().into_owned())
}

pub fn open(journal: &Journal, path: &str) -> Result<ProjectInfo> {
    let path = normalize(path)?;
    if !Path::new(&path).is_dir() {
        bail!("{path} is not a folder");
    }
    let state = journal.open_project(&path)?;
    let mut info = info(&path, crate::journal::now_ms());
    info.state = state;
    Ok(info)
}

pub fn recent(journal: &Journal) -> Result<Vec<ProjectInfo>> {
    Ok(journal
        .recent_projects()?
        .into_iter()
        .take(RECENT_LIMIT)
        .map(|(path, opened_ms)| info(&path, opened_ms))
        .collect())
}

/// A project's settings, what its files point to, and so the kernel each
/// language runs in.
pub fn setup(root: &Path) -> ProjectSetup {
    let (settings, saved, problem) = match settings::read(root) {
        Ok(Some(settings)) => (settings, true, None),
        Ok(None) => (ProjectSettings::default(), false, None),
        Err(err) => (
            ProjectSettings::default(),
            settings::file(root).exists(),
            Some(format!("{err:#}")),
        ),
    };
    let search = Search::from_env().in_project(Some(root));
    let kernels = kernelspecs::list(&search);
    let found = environments::find(root, &kernels, &search);
    let kernel = |language: &str, chosen: &Option<String>| {
        chosen.clone().or_else(|| {
            found
                .iter()
                .filter(|f| f.language == language)
                .find_map(|f| f.kernel.clone())
        })
    };
    ProjectSetup {
        r: kernel("r", &settings.r),
        python: kernel("python", &settings.python),
        settings,
        saved,
        problem,
        found,
    }
}

/// The variables of the project's env files, which its kernels start with.
pub fn kernel_env(root: &Path) -> Vec<(String, String)> {
    let files = settings::read(root)
        .ok()
        .flatten()
        .unwrap_or_default()
        .env_files;
    envfiles::load(root, &files)
}

fn info(path: &str, opened_ms: i64) -> ProjectInfo {
    let p = Path::new(path);
    ProjectInfo {
        path: path.to_string(),
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string()),
        opened_ms,
        exists: p.is_dir(),
        state: Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_compared_in_one_form() {
        assert_eq!(normalize("/data/proj/").unwrap(), "/data/proj");
        assert_eq!(normalize("/data/./proj").unwrap(), "/data/proj");
        assert_eq!(normalize("/").unwrap(), "/");
        assert!(normalize("proj").is_err());
    }
}
