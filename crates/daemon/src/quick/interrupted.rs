//! Interrupted installs (`docs/design/clean-footprint-plan.md`, section 11):
//! the install worker leaves a marker in `state/installs/` while it runs, with
//! what it is doing, the change to run again and the snapshot or new folder
//! that undoes it. A marker whose worker is gone (Sandhill quit, the computer
//! slept, the connection dropped) is an install that stopped part way, which
//! the R and Python page offers to finish or undo.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail, ensure};
use sandhill_proto::{InterruptedInstall, NewRuntime, OwnSetup, PackageChange};
use serde::{Deserialize, Serialize};

use super::install::{self, Job};
use super::{env_actions, own, record, undo};
use crate::lifecycle::ProcessIdentity;
use crate::own_env;

/// What runs again to finish an install, through its usual planner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Rerun {
    Packages(PackageChange),
    Runtime(NewRuntime),
    OwnSetup(OwnSetup),
}

/// A folder the install was making, which wasn't there before it started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Made {
    /// A new environment: the whole folder is Sandhill's.
    Folder(PathBuf),
    /// The first set-up of Sandhill's own folder: only the files it makes
    /// there (the folder itself may be one the user chose).
    Own(PathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Marker {
    id: String,
    /// In plain words: "Setting up Sandhill's own R and Python".
    what: String,
    folder: String,
    date: String,
    worker: Option<ProcessIdentity>,
    rerun: Option<Rerun>,
    /// The folders of the step-3 snapshots begun before changing packages.
    #[serde(default)]
    undo: Vec<PathBuf>,
    #[serde(default)]
    made: Option<Made>,
}

impl Marker {
    fn gone(&self) -> bool {
        self.worker.as_ref().is_none_or(|worker| !worker.alive())
    }

    fn snapshots(&self) -> Vec<PathBuf> {
        self.undo
            .iter()
            .filter(|dir| undo::pending(dir))
            .cloned()
            .collect()
    }

    fn made_here(&self) -> Option<&Made> {
        self.made.as_ref().filter(|made| match made {
            Made::Folder(folder) => removable_folder(folder),
            Made::Own(folder) => !own_env::marked(folder) && own::partial(folder),
        })
    }

    fn info(&self) -> InterruptedInstall {
        InterruptedInstall {
            id: self.id.clone(),
            what: self.what.clone(),
            folder: self.folder.clone(),
            date: self.date.clone(),
            can_finish: self.rerun.is_some(),
            can_undo: !self.snapshots().is_empty() || self.made_here().is_some(),
        }
    }

    pub fn rerun(&self) -> Option<&Rerun> {
        self.rerun.as_ref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A folder that is Sandhill's new environment to remove: a real folder,
/// not the home folder or one holding it.
fn removable_folder(folder: &Path) -> bool {
    folder.is_absolute()
        && folder.parent().is_some()
        && folder.symlink_metadata().is_ok_and(|m| m.is_dir())
        && install::home().is_none_or(|home| {
            !crate::kernelspecs::canonical(&home).starts_with(crate::kernelspecs::canonical(folder))
        })
}

fn dir(record: &Path) -> PathBuf {
    record.with_file_name("installs")
}

fn path(record: &Path, id: &str) -> Result<PathBuf> {
    ensure!(
        !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "That install wasn't found."
    );
    Ok(dir(record).join(format!("{id}.json")))
}

fn read(path: &Path) -> Option<Marker> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write(path: &Path, marker: &Marker) -> Result<()> {
    crate::files::write_atomic(path, &serde_json::to_vec(marker)?)
}

/// The marker of the job this worker process runs.
static CURRENT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Removes the marker when the install ends, however it ends.
pub(super) struct Guard(PathBuf);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        *CURRENT.lock().unwrap() = None;
    }
}

/// "Install DESeq2" → "Installing DESeq2".
fn doing(title: &str) -> String {
    let (verb, rest) = title.split_once(' ').unwrap_or((title, ""));
    let verb = match verb {
        "Install" => "Installing",
        "Update" => "Updating",
        "Remove" => "Removing",
        "Create" => "Creating",
        other => other,
    };
    format!("{verb} {rest}").trim_end().into()
}

fn describe(job: &Job) -> Option<(String, String, Option<Rerun>, Option<Made>)> {
    match job {
        Job::Packages(plan) => Some((
            doing(&install::package_title(plan)),
            plan.folder.clone(),
            Some(Rerun::Packages(plan.change.clone())),
            None,
        )),
        Job::Runtime(plan) => {
            let made = match &plan.runtime {
                NewRuntime::RConda { prefix }
                | NewRuntime::PythonConda { prefix, .. }
                | NewRuntime::PythonUv { prefix, .. } => Some(Made::Folder(prefix.into())),
                NewRuntime::Miniforge => Some(Made::Folder(plan.folder.clone().into())),
                NewRuntime::RFromCran | NewRuntime::RFromHomebrew => None,
            };
            Some((
                doing(install::runtime_title(&plan.runtime)),
                plan.folder.clone(),
                Some(Rerun::Runtime(plan.runtime.clone())),
                made,
            ))
        }
        Job::OwnSetup(work) => {
            let setup = &work.plan().setup;
            let folder = PathBuf::from(&setup.folder);
            Some((
                "Setting up Sandhill's own R and Python".into(),
                setup.folder.clone(),
                Some(Rerun::OwnSetup(setup.clone())),
                (!own_env::marked(&folder)).then_some(Made::Own(folder)),
            ))
        }
        Job::Environment(work) => env_actions::interrupted(work),
        Job::ProjectSetup(work) => Some((
            "Setting up a project's packages".into(),
            work.plan().folder.clone(),
            None,
            work.made().map(Made::Folder),
        )),
        _ => None,
    }
}

pub(super) fn describe_packages(
    plan: &sandhill_proto::PackagePlan,
) -> (String, String, Option<Rerun>, Option<Made>) {
    (
        doing(&install::package_title(plan)),
        plan.folder.clone(),
        (!plan.change.packages.is_empty()).then(|| Rerun::Packages(plan.change.clone())),
        None,
    )
}

/// Writes this worker's marker; it goes when the returned guard drops.
pub(super) fn begin(record: &Path, job: &Job) -> Result<Option<Guard>> {
    let Some((what, folder, rerun, made)) = describe(job) else {
        return Ok(None);
    };
    let marker = Marker {
        id: uuid::Uuid::new_v4().simple().to_string(),
        what,
        folder,
        date: record::today(),
        worker: ProcessIdentity::read(std::process::id()),
        rerun,
        undo: Vec::new(),
        made,
    };
    fs::create_dir_all(dir(record))?;
    let path = path(record, &marker.id)?;
    write(&path, &marker)?;
    *CURRENT.lock().unwrap() = Some(path.clone());
    Ok(Some(Guard(path)))
}

/// A step-3 snapshot was begun (its pending folder); an interruption can
/// be undone from it. The lock is held across reading and writing the
/// marker, so two snapshots begun at once both stay on it.
pub(super) fn note_snapshot(pending: &Path) {
    let current = CURRENT.lock().unwrap();
    if let Some(path) = current.as_ref()
        && let Some(mut marker) = read(path)
    {
        marker.undo.push(pending.to_path_buf());
        let _ = write(path, &marker);
    }
}

fn markers(record: &Path) -> Vec<Marker> {
    let mut markers: Vec<_> = fs::read_dir(dir(record))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| read(&entry.path()))
        .collect();
    markers.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
    markers
}

/// Installs that stopped part way: markers whose worker is gone.
pub(crate) fn list(record: &Path) -> Vec<InterruptedInstall> {
    markers(record)
        .into_iter()
        .filter(Marker::gone)
        .map(|marker| marker.info())
        .collect()
}

/// An interrupted install, by its id.
pub(crate) fn get(record: &Path, id: &str) -> Result<Marker> {
    let marker = read(&path(record, id)?)
        .context("That install wasn't found. It may have been finished or undone already.")?;
    ensure!(marker.gone(), "That install is still running.");
    Ok(marker)
}

/// The marker goes; Undo's job then owns the snapshots it names.
pub(crate) fn clear(record: &Path, id: &str) -> Result<()> {
    match fs::remove_file(path(record, id)?) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// Before finishing: removes the half-made new folder, since its planner
/// wants a new folder. Holds the install lock so no install runs meanwhile.
pub(crate) fn prepare_finish(record_path: &Path, marker: &Marker) -> Result<()> {
    ensure!(
        marker.rerun.is_some(),
        "This install can't be run again. Undo it instead."
    );
    let _lock = record::lock(record_path, false)
        .context("Wait for the current install or removal to finish.")?;
    if let Some(made) = marker.made_here() {
        remove_made(made)?;
    }
    Ok(())
}

/// After Finish started its new run (`started`), or was refused: the marker
/// goes, with the snapshots of the run that stopped once a new run has
/// started. A refused Finish keeps the marker while Undo can still put
/// things back.
pub(crate) fn settle_finish(record: &Path, id: &str, started: bool) -> Result<()> {
    let Some(marker) = read(&path(record, id)?) else {
        return Ok(());
    };
    if started {
        for pending in marker.snapshots() {
            let _ = fs::remove_dir_all(pending);
        }
    } else if marker.info().can_undo {
        return Ok(());
    }
    clear(record, id)
}

/// What undoing an interrupted install does, run by the install worker.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Undo {
    snapshots: Vec<PathBuf>,
    made: Option<Made>,
}

pub(crate) fn undo(marker: &Marker) -> Result<Undo> {
    let snapshots = marker.snapshots();
    let made = marker.made_here().cloned();
    ensure!(
        !snapshots.is_empty() || made.is_some(),
        "There's nothing Sandhill can put back for this install."
    );
    Ok(Undo { snapshots, made })
}

impl Undo {
    pub(super) fn run(self, record_path: &Path) -> Result<()> {
        for pending in &self.snapshots {
            let Some(id) = undo::recover(record_path, pending)? else {
                continue;
            };
            let (what, command, _) = undo::plan(record_path, &id)?;
            println!("Putting back the packages from before {what}.");
            undo::run(record_path, &id, &command)?;
        }
        if let Some(made) = &self.made {
            remove_made(made)?;
        }
        println!("Done.");
        Ok(())
    }
}

fn remove_made(made: &Made) -> Result<()> {
    match made {
        Made::Folder(folder) => {
            if !removable_folder(folder) {
                bail!("Sandhill won't remove {}.", folder.display());
            }
            println!("Removing the half-made {}.", folder.display());
            fs::remove_dir_all(folder)
                .with_context(|| format!("Couldn't remove {}.", folder.display()))
        }
        Made::Own(folder) => {
            ensure!(
                !own_env::marked(folder),
                "Sandhill's own R and Python in {} were finished meanwhile.",
                folder.display()
            );
            println!("Removing what the set-up made in {}.", folder.display());
            own::remove_partial(folder);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::test_common::tempdir;
    use sandhill_proto::{InstallTarget, InstallTool, PackageAction, PackagePlan, RuntimePlan};

    fn packages(folder: &Path) -> PackagePlan {
        PackagePlan {
            change: PackageChange {
                action: PackageAction::Install,
                target: InstallTarget::R {
                    r: "/R/bin/R".into(),
                    library: folder.display().to_string(),
                },
                packages: vec!["DESeq2".into()],
                tool: Some(InstallTool::R),
                reason: String::new(),
            },
            tools: Vec::new(),
            folder: folder.display().to_string(),
            packages: vec![sandhill_proto::PlannedPackage {
                name: "DESeq2".into(),
                spec: "DESeq2".into(),
                source: "Bioconductor".into(),
                version: None,
            }],
            command: "true".into(),
            notes: Vec::new(),
            refused: None,
            refused_fix: None,
        }
    }

    /// The tests share this process's one worker marker.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A marker as a worker that has since gone would have left it.
    fn leave(record: &Path, job: &Job) -> Marker {
        let guard = begin(record, job).unwrap().unwrap();
        let path = guard.0.clone();
        std::mem::forget(guard);
        *CURRENT.lock().unwrap() = None;
        let mut marker = read(&path).unwrap();
        // A process that has ended: this test's start time, a day earlier.
        marker.worker = marker.worker.map(|w| {
            serde_json::from_value(serde_json::json!({
                "pid": w.pid,
                "uid": 0,
                "birth": "0:0",
            }))
            .unwrap()
        });
        write(&path, &marker).unwrap();
        marker
    }

    #[test]
    fn the_marker_lives_while_the_worker_runs_and_goes_when_it_ends() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let record = dir.path().join(record::FILE);
        let job = Job::Packages(packages(dir.path()));
        let guard = begin(&record, &job).unwrap().unwrap();
        // This process is the worker and is alive: nothing is interrupted.
        assert_eq!(markers(&record).len(), 1);
        assert!(list(&record).is_empty());
        let pending = dir.path().join("undo/.pending-x");
        fs::create_dir_all(&pending).unwrap();
        note_snapshot(&pending);
        assert!(markers(&record)[0].undo.contains(&pending));
        drop(guard);
        assert!(markers(&record).is_empty());
        // Jobs that make nothing to finish or undo leave no marker.
        assert!(begin(&record, &Job::Remove(Vec::new())).unwrap().is_none());
    }

    #[test]
    fn a_gone_worker_leaves_an_interrupted_install_to_finish_or_undo() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let record = dir.path().join(record::FILE);
        let plan = packages(dir.path());
        let marker = leave(&record, &Job::Packages(plan.clone()));
        let listed = list(&record);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].what, "Installing DESeq2");
        assert!(listed[0].can_finish);
        // No snapshot was begun: nothing to undo.
        assert!(!listed[0].can_undo);
        assert!(undo(&marker).is_err());
        assert_eq!(
            get(&record, &listed[0].id).unwrap().rerun(),
            Some(&Rerun::Packages(plan.change))
        );
        prepare_finish(&record, &marker).unwrap();
        clear(&record, &listed[0].id).unwrap();
        assert!(list(&record).is_empty());
        assert!(get(&record, "../x").is_err());
    }

    #[test]
    fn undo_removes_only_a_folder_sandhill_was_making() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let record = dir.path().join(record::FILE);
        let prefix = dir.path().join("envs/new");
        let runtime = |prefix: &Path| RuntimePlan {
            runtime: NewRuntime::RConda {
                prefix: prefix.display().to_string(),
            },
            tool: "/mamba".into(),
            folder: prefix.display().to_string(),
            command: "true".into(),
            download_mb: None,
            notes: Vec::new(),
            refused: None,
            refused_fix: None,
        };
        let marker = leave(&record, &Job::Runtime(runtime(&prefix)));
        // The worker stopped before making the folder: nothing to undo.
        assert!(!list(&record)[0].can_undo);
        fs::create_dir_all(prefix.join("conda-meta")).unwrap();
        let info = &list(&record)[0];
        assert!(info.can_finish && info.can_undo);
        assert_eq!(info.what, "Creating an R environment");
        undo(&marker).unwrap().run(&record).unwrap();
        assert!(!prefix.exists());
        assert!(dir.path().join("envs").is_dir());

        // R from Homebrew or CRAN makes no folder of its own to remove.
        let brew = RuntimePlan {
            runtime: NewRuntime::RFromHomebrew,
            ..runtime(dir.path())
        };
        let marker = leave(&record, &Job::Runtime(brew));
        assert!(marker.made.is_none());
        assert!(undo(&marker).is_err());
    }

    #[test]
    fn finishing_a_first_own_setup_clears_only_the_files_it_made() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempdir().unwrap();
        let record = dir.path().join(record::FILE);
        let folder = dir.path().canonicalize().unwrap().join("own");
        fs::create_dir_all(folder.join("conda/pkgs")).unwrap();
        fs::write(folder.join("micromamba"), "").unwrap();
        fs::write(folder.join("notes.txt"), "the user's").unwrap();
        let setup = OwnSetup {
            folder: folder.display().to_string(),
            r: true,
            python: false,
            kits: Vec::new(),
            copy_from: Vec::new(),
        };
        let work = own::plan(&record, setup.clone());
        let marker = leave(&record, &Job::OwnSetup(work));
        let info = &list(&record)[0];
        assert_eq!(info.what, "Setting up Sandhill's own R and Python");
        assert!(info.can_finish && info.can_undo);
        assert_eq!(marker.rerun(), Some(&Rerun::OwnSetup(setup)));
        prepare_finish(&record, &marker).unwrap();
        assert!(!folder.join("conda").exists() && !folder.join("micromamba").exists());
        assert!(folder.join("notes.txt").is_file());
    }
}
