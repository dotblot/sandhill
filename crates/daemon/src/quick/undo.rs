//! The single last package change for each environment. The install worker
//! is the only writer and keeps the previous snapshot if nothing changed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use sandhill_proto::{
    InstallTarget, InstallTool, PackageAction, PackagePlan, QuickInstalled, UndoInfo,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{environments, install, package_plan, record};

const RETENTION: Duration = Duration::from_secs(30 * 86_400);

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Track {
    pub target: InstallTarget,
    pub folder: PathBuf,
    pub tool: InstallTool,
    pub path: String,
}

impl Track {
    pub fn from_plan(plan: &PackagePlan) -> Result<Self> {
        let tool = plan.change.tool.context("No tool was chosen.")?;
        let path = plan
            .tools
            .iter()
            .find(|t| t.tool == tool)
            .context("The install tool wasn't found.")?
            .path
            .clone();
        Ok(Self {
            target: plan.change.target.clone(),
            folder: PathBuf::from(&plan.folder),
            tool,
            path,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self.tool {
            InstallTool::R => record::R_PACKAGE,
            InstallTool::Conda => record::CONDA_PACKAGE,
            InstallTool::Pip | InstallTool::Uv => record::PYTHON_PACKAGE,
        }
    }

    fn inventory(&self) -> Result<Inventory> {
        match self.tool {
            InstallTool::R => r_inventory(&self.folder),
            InstallTool::Conda => conda_inventory(&self.folder),
            InstallTool::Pip | InstallTool::Uv => {
                let InstallTarget::Python { python, .. } = &self.target else {
                    bail!("Python wasn't selected.")
                };
                Ok(
                    package_plan::python_inventory(Path::new(python), &self.folder)?
                        .into_iter()
                        .map(|(name, version)| {
                            (
                                name,
                                Item {
                                    version,
                                    detail: String::new(),
                                },
                            )
                        })
                        .collect(),
                )
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Item {
    version: String,
    // Conda's build tag, or R's DESCRIPTION and directory identity.
    detail: String,
}

type Inventory = BTreeMap<String, Item>;

fn r_inventory(folder: &Path) -> Result<Inventory> {
    use std::os::unix::fs::MetadataExt;
    let packages = record::r_packages(folder)?;
    packages
        .into_iter()
        .map(|(name, version)| {
            let path = folder.join(&name);
            let meta = path.symlink_metadata()?;
            let description = fs::read(path.join("DESCRIPTION"))?;
            let hash = sandhill_proto::hex(&Sha256::digest(&description));
            Ok((
                name,
                Item {
                    version,
                    detail: format!(
                        "{hash}:{}:{}:{}",
                        meta.ino(),
                        meta.mtime(),
                        meta.mtime_nsec()
                    ),
                },
            ))
        })
        .collect()
}

fn conda_inventory(folder: &Path) -> Result<Inventory> {
    Ok(fs::read_dir(folder.join("conda-meta"))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let (name_version, build) = name.to_str()?.strip_suffix(".json")?.rsplit_once('-')?;
            let (name, version) = name_version.rsplit_once('-')?;
            (package_plan::python_spec(name) == Some(name)).then(|| {
                (
                    name.into(),
                    Item {
                        version: version.into(),
                        detail: build.into(),
                    },
                )
            })
        })
        .collect())
}

#[derive(Serialize, Deserialize)]
struct Change {
    track: Track,
    before: Inventory,
    after: Inventory,
    owned: Vec<QuickInstalled>,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    info: UndoInfo,
    changes: Vec<Change>,
    #[serde(default)]
    created: Option<(QuickInstalled, String)>,
}

pub(super) struct Pending {
    id: String,
    dir: tempfile::TempDir,
    snapshot: Snapshot,
}

pub(super) fn begin(record_path: &Path, title: String, tracks: Vec<Track>) -> Result<Pending> {
    let id = environments::id(
        &tracks
            .first()
            .context("No environment was selected.")?
            .target,
    );
    let root = record_path.with_file_name("undo");
    fs::create_dir_all(&root)?;
    let dir = tempfile::Builder::new()
        .prefix(".pending-")
        .tempdir_in(root)?;
    let rows = record::load(record_path)?;
    let mut changes = Vec::new();
    for (index, track) in tracks.into_iter().enumerate() {
        ensure!(
            environments::id(&track.target) == id,
            "Undo must record one environment at a time."
        );
        let before = track.inventory()?;
        let owned = rows
            .iter()
            .filter(|row| {
                row.kind == track.kind()
                    && Path::new(&row.place) == track.folder
                    && before
                        .get(&row.name)
                        .is_some_and(|item| item.version == row.version)
            })
            .cloned()
            .collect();
        if track.tool == InstallTool::R {
            // Dependencies can replace packages as well as the requested changes.
            // Clone the library's package directories before the installer runs,
            // then keep only the ones the change actually touched.
            let backups = dir.path().join(index.to_string());
            fs::create_dir(&backups)?;
            for name in before.keys() {
                let from = track.folder.join(name);
                let backup = backups.join(name);
                if from.is_symlink() {
                    std::os::unix::fs::symlink(fs::read_link(from)?, backup)?;
                } else {
                    clone_package(&from, &backup)?;
                }
            }
        }
        changes.push(Change {
            track,
            before,
            after: Inventory::new(),
            owned,
        });
    }
    Ok(Pending {
        id,
        dir,
        snapshot: Snapshot {
            info: UndoInfo {
                what: title,
                date: record::today(),
            },
            changes,
            created: None,
        },
    })
}

fn clone_package(from: &Path, to: &Path) -> Result<()> {
    if cfg!(target_os = "macos")
        && Command::new("/bin/cp")
            .args(["-c", "-pR"])
            .arg(from)
            .arg(to)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    {
        return Ok(());
    }
    if to.is_dir() {
        fs::remove_dir_all(to)?;
    }
    ensure!(
        Command::new("/bin/cp")
            .arg("-pR")
            .arg(from)
            .arg(to)
            .status()?
            .success(),
        "Couldn't save a copy of {} for Undo.",
        from.display()
    );
    Ok(())
}

fn changed<'a>(before: &'a Inventory, after: &'a Inventory) -> BTreeSet<&'a str> {
    before
        .keys()
        .chain(after.keys())
        .filter(|name| before.get(*name) != after.get(*name))
        .map(String::as_str)
        .collect()
}

impl Pending {
    pub fn finish(mut self, record: &Path) -> Result<()> {
        for (index, change) in self.snapshot.changes.iter_mut().enumerate() {
            change.after = change.track.inventory()?;
            if change.track.tool == InstallTool::R {
                for name in change.before.keys() {
                    if change.before.get(name) == change.after.get(name) {
                        let backup = self.dir.path().join(index.to_string()).join(name);
                        if backup.is_dir() {
                            fs::remove_dir_all(backup)?;
                        }
                    }
                }
            }
        }
        if self.snapshot.created.is_none()
            && self.snapshot.changes.iter().all(|c| c.before == c.after)
        {
            return Ok(());
        }
        crate::files::write_atomic(
            &self.dir.path().join("snapshot.json"),
            &serde_json::to_vec(&self.snapshot)?,
        )?;
        let target = environments::state_path(record, "undo", &self.id);
        if target.is_dir() {
            let old = tempfile::Builder::new()
                .prefix(".old-")
                .tempdir_in(target.parent().unwrap())?;
            let backup = old.path().join("snapshot");
            fs::rename(&target, &backup)?;
            if let Err(error) = fs::rename(self.dir.path(), &target) {
                let _ = fs::rename(backup, &target);
                return Err(error.into());
            }
        } else {
            fs::rename(self.dir.path(), &target)?;
        }
        Ok(())
    }
}

pub(super) fn created(record: &Path, id: &str, item: QuickInstalled) -> Result<()> {
    let identity = record::directory_identity(Path::new(&item.place))
        .context("The new environment couldn't be checked for Undo.")?;
    let root = record.with_file_name("undo");
    fs::create_dir_all(&root)?;
    Pending {
        id: id.into(),
        dir: tempfile::Builder::new()
            .prefix(".pending-")
            .tempdir_in(root)?,
        snapshot: Snapshot {
            info: UndoInfo {
                what: "setting up the project's packages".into(),
                date: record::today(),
            },
            changes: Vec::new(),
            created: Some((item, identity)),
        },
    }
    .finish(record)
}

fn expired(dir: &Path) -> bool {
    dir.metadata()
        .and_then(|m| m.modified())
        .and_then(|m| m.elapsed().map_err(std::io::Error::other))
        .is_ok_and(|elapsed| elapsed > RETENTION)
}

pub(super) fn prune(record: &Path) {
    if let Ok(_lock) = record::lock(record, false) {
        prune_locked(record);
    }
}

pub(super) fn prune_locked(record: &Path) {
    for entry in fs::read_dir(record.with_file_name("undo"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if expired(&entry.path()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn load(record: &Path, id: &str) -> Result<(PathBuf, Snapshot)> {
    let path = environments::state_path(record, "undo", id);
    let none = "There's no recent change to undo in this environment.";
    ensure!(!expired(&path), "{none}");
    let bytes = fs::read(path.join("snapshot.json")).context(none)?;
    Ok((
        path,
        serde_json::from_slice(&bytes).context("The saved change couldn't be read.")?,
    ))
}

pub(super) fn info(record: &Path, id: &str) -> Option<UndoInfo> {
    Some(load(record, id).ok()?.1.info)
}

fn commands(path: &Path, snapshot: &Snapshot) -> Result<(Vec<String>, Vec<String>)> {
    let mut commands = Vec::new();
    let mut notes = Vec::new();
    for (index, change) in snapshot.changes.iter().enumerate() {
        let track = &change.track;
        let names = changed(&change.before, &change.after);
        if names.is_empty() {
            continue;
        }
        let now = track.inventory()?;
        let conflict = if track.tool == InstallTool::R {
            names.iter().any(|name| {
                now.get(*name) != change.after.get(*name)
                    || (!change.after.contains_key(*name)
                        && track.folder.join(name).symlink_metadata().is_ok())
            })
        } else {
            now != change.after
        };
        ensure!(
            !conflict,
            "This environment changed outside Sandhill since that change. Undo would overwrite those changes."
        );
        let mut restore = Vec::new();
        let mut remove = Vec::new();
        for name in names {
            if let Some(before) = change.before.get(name) {
                notes.push(format!(
                    "Put back {name} {} in {}.",
                    before.version,
                    track.folder.display()
                ));
                match track.tool {
                    InstallTool::R => {
                        let backup = path.join(index.to_string()).join(name);
                        ensure!(
                            backup
                                .symlink_metadata()
                                .is_ok_and(|meta| meta.is_dir() || meta.is_symlink()),
                            "Sandhill couldn't save the previous folder for {name}. Undo is not available."
                        );
                        let target = track.folder.join(name);
                        commands.push(install::quote([
                            "/bin/rm",
                            "-rf",
                            "--",
                            &target.to_string_lossy(),
                        ]));
                        commands.push(install::quote([
                            "/bin/cp",
                            "-pRP",
                            "--",
                            &backup.to_string_lossy(),
                            &target.to_string_lossy(),
                        ]));
                    }
                    InstallTool::Conda => {
                        restore.push(format!("{name}={}={}", before.version, before.detail))
                    }
                    _ => restore.push(format!("{name}=={}", before.version)),
                }
            } else {
                notes.push(format!("Remove {name} from {}.", track.folder.display()));
                if track.tool == InstallTool::R {
                    commands.push(install::quote([
                        "/bin/rm",
                        "-rf",
                        "--",
                        &track.folder.join(name).to_string_lossy(),
                    ]));
                } else {
                    remove.push(name.to_owned());
                }
            }
        }
        if track.tool == InstallTool::Conda {
            let folder = track.folder.to_string_lossy();
            if !remove.is_empty() {
                commands.push(install::quote(
                    [track.path.as_str(), "remove", "-y", "-p", &folder]
                        .into_iter()
                        .chain(remove.iter().map(String::as_str)),
                ));
            }
            if !restore.is_empty() {
                commands.push(install::quote(
                    [
                        track.path.as_str(),
                        "install",
                        "-y",
                        "-p",
                        &folder,
                        "--override-channels",
                        "-c",
                        "conda-forge",
                        "-c",
                        "bioconda",
                    ]
                    .into_iter()
                    .chain(restore.iter().map(String::as_str)),
                ));
            }
        } else if let InstallTarget::Python { python, .. } = &track.target {
            let mut command = Vec::new();
            if !remove.is_empty() {
                command.push(package_plan::python_command(
                    track.tool,
                    &track.path,
                    python,
                    PackageAction::Remove,
                    &remove,
                ));
            }
            if !restore.is_empty() {
                command.push(package_plan::python_command(
                    track.tool,
                    &track.path,
                    python,
                    PackageAction::Install,
                    &restore,
                ));
            }
            let command = command.join(" && ");
            if let Some(folder) = crate::own_env::containing(Path::new(python)) {
                commands.push(crate::own_env::command(&folder, &command));
            } else {
                commands.push(command);
            }
        }
    }
    Ok((commands, notes))
}

fn created_plan(record: &Path, snapshot: &Snapshot) -> Result<Option<(String, Vec<String>)>> {
    let Some((item, identity)) = &snapshot.created else {
        return Ok(None);
    };
    ensure!(
        record::directory_identity(Path::new(&item.place)).as_ref() == Some(identity),
        "The folder has changed since Sandhill set it up, or it is no longer there."
    );
    let search = crate::kernelspecs::Search::from_env().in_own(crate::own_env::chosen(record));
    let command = record::undo_creation(record, item, &search)?;
    Ok(Some((
        command,
        vec![format!(
            "Remove only the folder Sandhill created: {}.",
            item.place
        )],
    )))
}

pub(super) fn plan(record: &Path, id: &str) -> Result<(String, String, Vec<String>)> {
    let (path, snapshot) = load(record, id)?;
    let (command, notes) = if let Some(plan) = created_plan(record, &snapshot)? {
        plan
    } else {
        let (commands, notes) = commands(&path, &snapshot)?;
        (commands.join(" && "), notes)
    };
    Ok((snapshot.info.what, command, notes))
}

pub(super) fn run(record_path: &Path, id: &str, approved: &str) -> Result<()> {
    let (path, snapshot) = load(record_path, id)?;
    if let Some((command, _)) = created_plan(record_path, &snapshot)? {
        ensure!(
            command == approved,
            "The last change is different now. Review Undo Last Change again."
        );
        record::remove(
            record_path,
            &[record::key(&snapshot.created.as_ref().unwrap().0)],
        )?;
        fs::remove_dir_all(path)?;
        return Ok(());
    }
    let (commands, _) = commands(&path, &snapshot)?;
    let command = commands.join(" && ");
    ensure!(
        command == approved,
        "The last change is different now. Review Undo Last Change again."
    );
    let mut rows = record::load(record_path)?;
    let changed_libraries: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.kind == record::RENV_LIBRARY
                && snapshot
                    .changes
                    .iter()
                    .any(|change| change.track.folder.starts_with(&row.place))
                && record::directory_identity(Path::new(&row.place)).as_ref() == Some(&row.version)
        })
        .map(|row| row.place.clone())
        .collect();
    install::run_command(&command)?;
    for change in snapshot.changes {
        let names = changed(&change.before, &change.after);
        let current = change.track.inventory()?;
        let restored = if change.track.tool == InstallTool::R {
            names.iter().all(|name| {
                current.get(*name).map(|p| &p.version)
                    == change.before.get(*name).map(|p| &p.version)
            })
        } else {
            current == change.before
        };
        ensure!(
            restored,
            "The tool didn't put back every package. See the messages above."
        );
        rows.retain(|row| {
            !(row.kind == change.track.kind()
                && Path::new(&row.place) == change.track.folder
                && names.contains(row.name.as_str()))
        });
        rows.extend(
            change
                .owned
                .into_iter()
                .filter(|row| names.contains(row.name.as_str())),
        );
    }
    for row in rows
        .iter_mut()
        .filter(|row| row.kind == record::RENV_LIBRARY && changed_libraries.contains(&row.place))
    {
        if let Some(identity) = record::directory_identity(Path::new(&row.place)) {
            row.version = identity;
        }
    }
    record::save(record_path, &rows)?;
    fs::remove_dir_all(path)?;
    println!("The last change was undone.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r_package(dir: &Path, name: &str, version: &str) {
        fs::create_dir_all(dir.join(name)).unwrap();
        fs::write(
            dir.join(name).join("DESCRIPTION"),
            format!("Package: {name}\nVersion: {version}\n"),
        )
        .unwrap();
    }

    #[test]
    fn undo_restores_a_folder_and_refuses_external_changes() {
        let temp = tempfile::tempdir().unwrap();
        let library = temp.path().join("lib");
        let record = temp.path().join(record::FILE);
        r_package(&library, "test", "1.0");
        let track = Track {
            target: InstallTarget::R {
                r: "/some/R".into(),
                library: library.to_string_lossy().into_owned(),
            },
            folder: library.clone(),
            tool: InstallTool::R,
            path: "/some/R".into(),
        };
        let id = environments::id(&track.target);
        let pending = begin(&record, "Update test".into(), vec![track]).unwrap();
        r_package(&library, "test", "2.0");
        r_package(&library, "added", "1.1");
        pending.finish(&record).unwrap();
        let (_, cmd, notes) = plan(&record, &id).unwrap();
        assert!(notes.iter().any(|line| line.contains("Put back test 1.0")));
        assert!(notes.iter().any(|line| line.contains("Remove added")));
        r_package(&library, "added", "9.9");
        assert!(
            plan(&record, &id)
                .unwrap_err()
                .to_string()
                .contains("outside Sandhill")
        );
        r_package(&library, "added", "1.1");
        run(&record, &id, &cmd).unwrap();
        assert_eq!(
            record::package_version(&library, "test").as_deref(),
            Some("1.0")
        );
        assert!(!library.join("added").exists());
        assert!(info(&record, &id).is_none());
    }

    #[test]
    fn conda_inventory_keeps_exact_builds() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("conda-meta");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("lib-name-1.0-h01_0.json"), "{}").unwrap();
        assert_eq!(
            conda_inventory(temp.path()).unwrap()["lib-name"],
            Item {
                version: "1.0".into(),
                detail: "h01_0".into()
            }
        );
    }

    #[test]
    fn undo_plans_remove_new_packages_and_pin_conda_builds() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let meta = root.join("conda-meta");
        fs::create_dir(&meta).unwrap();
        fs::write(meta.join("old-1.0-build_0.json"), "{}").unwrap();
        let track = Track {
            target: InstallTarget::Python {
                python: "/env/bin/python".into(),
                prefix: root.to_string_lossy().into_owned(),
            },
            folder: root.into(),
            tool: InstallTool::Conda,
            path: "/conda".into(),
        };
        let record = root.join(record::FILE);
        let id = environments::id(&track.target);
        let pending = begin(&record, "Update old".into(), vec![track]).unwrap();
        fs::remove_file(meta.join("old-1.0-build_0.json")).unwrap();
        fs::write(meta.join("old-2.0-build_1.json"), "{}").unwrap();
        fs::write(meta.join("added-1.1-build_0.json"), "{}").unwrap();
        pending.finish(&record).unwrap();
        let (_, command, notes) = plan(&record, &id).unwrap();
        assert!(command.contains("'remove' '-y'") && command.contains("'added'"));
        assert!(command.contains("'old=1.0=build_0'") && command.contains("'--override-channels'"));
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn undo_plans_pip_and_uv_from_environment_inventory() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let program = root.join("python");
        let site = root.join("lib/python/site-packages");
        fs::create_dir_all(site.join("existing-1.0.dist-info")).unwrap();
        let db = site.join("existing-1.0.dist-info/inventory.json");
        fs::write(
            &program,
            format!(
                "#!/bin/sh\ncat {}\n",
                install::quote([db.to_str().unwrap()])
            ),
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let record = root.join(record::FILE);
        for tool in [InstallTool::Pip, InstallTool::Uv] {
            fs::write(&db, br#"{"existing":"1.0"}"#).unwrap();
            let track = Track {
                target: InstallTarget::Python {
                    python: program.to_string_lossy().into_owned(),
                    prefix: root.to_string_lossy().into_owned(),
                },
                folder: root.into(),
                tool,
                path: "/uv".into(),
            };
            let id = environments::id(&track.target);
            let pending = begin(&record, "Update existing".into(), vec![track]).unwrap();
            fs::write(&db, br#"{"existing":"1.1","added":"0.2"}"#).unwrap();
            pending.finish(&record).unwrap();
            let (_, command, notes) = plan(&record, &id).unwrap();
            assert!(command.contains("'uninstall'") && command.contains("'added'"));
            assert!(command.contains("'existing==1.0'"));
            assert_eq!(command.contains("'--python'"), tool == InstallTool::Uv);
            assert_eq!(notes.len(), 2);
        }
    }
}

#[cfg(test)]
#[test]
fn undo_new_renv_library_keeps_activation_and_shared_cache() {
    let temp = crate::quick::test_common::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    let library = project.join("renv/library");
    let cache = root.join("cache");
    fs::create_dir_all(&library).unwrap();
    fs::create_dir(&cache).unwrap();
    fs::write(project.join("renv/activate.R"), "# keep").unwrap();
    let record_path = root.join(record::FILE);
    let row = record::entry(
        record::RENV_LIBRARY,
        "project",
        record::directory_identity(&library).unwrap(),
        &library.to_string_lossy(),
        "renv",
        "Set up this project's packages",
    );
    record::add(&record_path, [row.clone()]).unwrap();
    created(&record_path, "test-r", row).unwrap();
    let (_, command, _) = plan(&record_path, "test-r").unwrap();
    run(&record_path, "test-r", &command).unwrap();
    assert!(!library.exists());
    assert!(project.join("renv/activate.R").is_file());
    assert!(cache.is_dir());
}
