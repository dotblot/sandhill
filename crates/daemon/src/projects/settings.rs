//! A project's shared settings, in `.sandhill/project.toml`: the kernel each
//! language runs in, where documents run, the env files its kernels load,
//! and where its agent conversations are kept. The folder is made only when a setting is first saved, so opening
//! someone's repository leaves no trace in it.
//!
//! Saving changes only the keys Sandhill knows and keeps the rest of the
//! file, comments included, as people wrote it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sandhill_proto::{ChatHistory, DocumentsRunIn, ProjectSettings};
use serde::Deserialize;
use toml_edit::{Array, DocumentMut, Item, Table, value};

/// What a kernel setting says to use the environment the project's files
/// point to.
pub const DETECT: &str = "detect";

const TEMPLATE: &str = r#"# Sandhill project settings. Commit this file to share them with everyone
# who opens the project.

[kernels]
# The Jupyter kernel each language runs in, by name, or "detect" for the
# environment the project's files point to (renv.lock, environment.yml,
# pixi.toml, .venv).
r = "detect"
python = "detect"

[run]
# Where notebook and Quarto chunks run: "document" (the document's own
# folder, where `quarto render` runs them) or "project" (the project's
# folder). The Console and scripts run in the project's folder.
documents = "document"

[env]
# Files of NAME=value lines whose variables the project's kernels start with.
files = [".env"]

[agents]
# Where agent conversations are kept: "project" (in .sandhill/agents/, so
# they go with the folder to another computer; git ignores them) or
# "machine" (on the computer the project is on).
history = "project"
"#;

const GITIGNORE: &str =
    "# Written by Sandhill: saved outputs and agent conversations stay out of git.\noutputs/\n";

pub fn file(root: &Path) -> PathBuf {
    root.join(".sandhill/project.toml")
}

/// The project's settings, `None` when it has no `project.toml`.
pub fn read(root: &Path) -> Result<Option<ProjectSettings>> {
    let path = file(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let parsed: File =
        toml::from_str(&text).with_context(|| format!("{} isn't valid", relative(root, &path)))?;
    let kernel = |name: Option<String>| name.filter(|n| !n.is_empty() && n != DETECT);
    let defaults = ProjectSettings::default();
    Ok(Some(ProjectSettings {
        r: kernel(parsed.kernels.r),
        python: kernel(parsed.kernels.python),
        documents: parsed.run.documents.unwrap_or(defaults.documents),
        env_files: parsed.env.files.unwrap_or(defaults.env_files),
        chat_history: parsed.agents.history.unwrap_or(defaults.chat_history),
    }))
}

/// Writes the settings, making `.sandhill/` and its `.gitignore` if need be.
pub fn write(root: &Path, settings: &ProjectSettings) -> Result<()> {
    let path = file(root);
    let dir = path.parent().context("no folder for project.toml")?;
    std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, GITIGNORE)?;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(err) => return Err(err).with_context(|| format!("cannot read {}", path.display())),
    };
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("{} isn't valid", relative(root, &path)))?;
    let kernel = |name: &Option<String>| value(name.as_deref().unwrap_or(DETECT));
    set(&mut doc, "kernels", "r", kernel(&settings.r));
    set(&mut doc, "kernels", "python", kernel(&settings.python));
    let documents = match settings.documents {
        DocumentsRunIn::Document => "document",
        DocumentsRunIn::Project => "project",
    };
    set(&mut doc, "run", "documents", value(documents));
    let files: Array = settings.env_files.iter().map(String::as_str).collect();
    set(&mut doc, "env", "files", value(files));
    let history = match settings.chat_history {
        ChatHistory::Project => "project",
        ChatHistory::Machine => "machine",
    };
    set(&mut doc, "agents", "history", value(history));
    std::fs::write(&path, doc.to_string())
        .with_context(|| format!("cannot write {}", path.display()))
}

/// Has `.sandhill/.gitignore` ignore `line` (`agents/`), making it if need be.
pub fn ignore(root: &Path, line: &str) -> Result<()> {
    let dir = file(root)
        .parent()
        .context("no .sandhill folder")?
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(".gitignore");
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| GITIGNORE.to_string());
    if text.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(line);
    text.push('\n');
    std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()))
}

/// Sets `[section] key`, keeping the comments around it.
fn set(doc: &mut DocumentMut, section: &str, key: &str, new: Item) {
    if !doc.contains_table(section) {
        doc.insert(section, Item::Table(Table::new()));
    }
    if let Some(table) = doc[section].as_table_mut() {
        match table.get_mut(key) {
            // Replacing the value keeps the key, and the comment above it.
            Some(item) if item.is_value() => {
                let mut new = new;
                if let (Some(old), Some(fresh)) = (item.as_value(), new.as_value_mut()) {
                    *fresh.decor_mut() = old.decor().clone();
                }
                *item = new;
            }
            _ => {
                table.insert(key, new);
            }
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[derive(Deserialize, Default)]
struct File {
    #[serde(default)]
    kernels: Kernels,
    #[serde(default)]
    run: Run,
    #[serde(default)]
    env: Env,
    #[serde(default)]
    agents: Agents,
}

#[derive(Deserialize, Default)]
struct Kernels {
    r: Option<String>,
    python: Option<String>,
}

#[derive(Deserialize, Default)]
struct Run {
    documents: Option<DocumentsRunIn>,
}

#[derive(Deserialize, Default)]
struct Env {
    files: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct Agents {
    history: Option<ChatHistory>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ProjectSettings {
        ProjectSettings {
            r: Some("ark".into()),
            python: Some("conda-env-omicverse-py".into()),
            documents: DocumentsRunIn::Project,
            env_files: vec![".env".into(), "secrets.env".into()],
            chat_history: ChatHistory::Machine,
        }
    }

    #[test]
    fn a_project_without_the_file_has_no_settings() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()).unwrap(), None);
        assert!(!dir.path().join(".sandhill").exists());
    }

    #[test]
    fn saved_settings_read_back_with_the_folder_ignoring_outputs() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &settings()).unwrap();
        assert_eq!(read(dir.path()).unwrap(), Some(settings()));
        let ignore = std::fs::read_to_string(dir.path().join(".sandhill/.gitignore")).unwrap();
        assert!(ignore.lines().any(|l| l == "outputs/"));
        let text = std::fs::read_to_string(file(dir.path())).unwrap();
        assert!(text.starts_with("# Sandhill project settings."));
        assert!(text.contains("python = \"conda-env-omicverse-py\""));
        assert!(text.contains("history = \"machine\""), "{text}");

        super::ignore(dir.path(), "agents/").unwrap();
        super::ignore(dir.path(), "agents/").unwrap();
        let ignored = std::fs::read_to_string(dir.path().join(".sandhill/.gitignore")).unwrap();
        assert_eq!(
            ignored.lines().filter(|l| *l == "agents/").count(),
            1,
            "{ignored}"
        );

        // "detect" is no kernel.
        write(dir.path(), &ProjectSettings::default()).unwrap();
        let text = std::fs::read_to_string(file(dir.path())).unwrap();
        assert!(text.contains("r = \"detect\""), "{text}");
        assert_eq!(read(dir.path()).unwrap(), Some(ProjectSettings::default()));
    }

    #[test]
    fn saving_keeps_what_people_wrote() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".sandhill")).unwrap();
        std::fs::write(
            file(dir.path()),
            "# Ours: the lab's server.\n[kernels]\n# Seurat lives here.\nr = \"ir\"  # not Ark\n\n[extra]\nnote = 1\n",
        )
        .unwrap();
        let read_back = read(dir.path()).unwrap().unwrap();
        assert_eq!(read_back.r.as_deref(), Some("ir"));
        assert_eq!(read_back.python, None);
        assert_eq!(read_back.env_files, vec![".env".to_string()]);

        write(dir.path(), &settings()).unwrap();
        let text = std::fs::read_to_string(file(dir.path())).unwrap();
        for kept in [
            "# Ours: the lab's server.",
            "# Seurat lives here.",
            "# not Ark",
            "[extra]",
            "note = 1",
        ] {
            assert!(text.contains(kept), "lost {kept:?}:\n{text}");
        }
        assert!(text.contains("r = \"ark\""), "{text}");
        assert_eq!(read(dir.path()).unwrap(), Some(settings()));
    }

    #[test]
    fn a_broken_file_says_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".sandhill")).unwrap();
        std::fs::write(file(dir.path()), "[kernels\n").unwrap();
        let err = read(dir.path()).unwrap_err().to_string();
        assert!(err.contains(".sandhill/project.toml"), "{err}");
    }
}
