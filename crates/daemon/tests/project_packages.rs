mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, ProjectFilePlan, ProjectPackageFile, ProjectSetupPlan, QuickInstalling, TerminalInfo,
};
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

fn r_in(home: &Path, path: &std::ffi::OsStr, r: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(r);
    command
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("TMPDIR", home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("RENV_PATHS_ROOT", home.join("renv"))
        .env("RENV_CONFIG_SYSREQS_CHECK", "false")
        .env("RENV_CONFIG_PPM_ENABLED", "false")
        .current_dir(home)
        .args(["--vanilla", "--slave", "-e"]);
    command
}

#[tokio::test]
async fn project_renv_lock_is_readable_by_real_renv() {
    let tmp = common::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let home = root.join("home");
    let project = root.join("analysis");
    let state = root.join("state");
    for dir in [&home, &project] {
        std::fs::create_dir(dir).unwrap();
    }
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    let r = std::env::split_paths(&path)
        .map(|dir| dir.join("R"))
        .chain([PathBuf::from(
            "/Library/Frameworks/R.framework/Resources/bin/R",
        )])
        .find(|r| r.is_file());
    let Some(r) = r else {
        eprintln!("Skipping the real renv restore: R isn't installed.");
        return;
    };
    let probe = r_in(&home, &path, &r).arg("cat(find.package('renv', quiet=TRUE)); quit(status=if (requireNamespace('renv', quietly=TRUE)) 0 else 1)").output().unwrap();
    if !probe.status.success() {
        eprintln!(
            "Skipping the real renv restore: renv isn't installed in the available R ({}) with an isolated home.",
            r.display()
        );
        return;
    }
    let library = root.join("restore-library");
    std::fs::create_dir(&library).unwrap();
    let copy = r_in(&home, &path, &r)
        .env("SANDHILL_TEST_LIBRARY", &library)
        .arg("stopifnot(file.copy(find.package('MASS'), Sys.getenv('SANDHILL_TEST_LIBRARY'), recursive=TRUE))")
        .output().unwrap();
    assert!(
        copy.status.success(),
        "{}",
        String::from_utf8_lossy(&copy.stderr)
    );
    std::fs::write(
        project.join("analysis.R"),
        "library(MASS)\nbase::print(1)\n",
    )
    .unwrap();
    let kernels = home.join("jupyter/kernels/project-test-r");
    std::fs::create_dir_all(&kernels).unwrap();
    std::fs::write(kernels.join("kernel.json"), serde_json::to_vec(&serde_json::json!({
        "argv": [r, PathBuf::from("--no-save"), PathBuf::from("-e"), PathBuf::from("IRkernel::main()"), PathBuf::from("--args"), PathBuf::from("{connection_file}")],
        "language": "R", "display_name": "Project test R"
    })).unwrap()).unwrap();
    let _serve = Command::new(DAEMON)
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", &home)
        .env("PATH", &path)
        .env("SHELL", "/bin/bash")
        .env("JUPYTER_DATA_DIR", home.join("jupyter"))
        .env("RENV_PATHS_ROOT", home.join("renv"))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let client = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(c) = Client::connect_unix(&state.join("sandhilld.sock")).await {
                break c;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let project = project.to_string_lossy().into_owned();
    let restricted: ProjectSetupPlan = client
        .call(Call::PlanProjectSetup {
            project: project.clone(),
            file: "renv.lock".into(),
        })
        .await
        .unwrap();
    assert!(restricted.refused.unwrap().contains("Trust this project"));
    client
        .call::<serde_json::Value>(Call::TrustFolder {
            path: project.clone(),
        })
        .await
        .unwrap();
    let plan: ProjectFilePlan = client
        .call(Call::PlanProjectFile {
            project: project.clone(),
            language: "r".into(),
        })
        .await
        .unwrap();
    assert!(plan.refused.is_none(), "{plan:?}");
    assert!(plan.packages.iter().any(|p| p.name == "MASS"), "{plan:?}");
    assert!(!plan.packages.iter().any(|p| p.name == "base"));
    assert!(!Path::new(&plan.path).exists());
    std::fs::write(Path::new(&project).join("analysis.R"), "base::print(1)\n").unwrap();
    let saved: ProjectFilePlan = client
        .call(Call::WriteProjectFile {
            project: project.clone(),
            language: "r".into(),
        })
        .await
        .unwrap();
    assert_eq!(saved.contents, plan.contents);
    assert_eq!(std::fs::read_to_string(&plan.path).unwrap(), plan.contents);
    let files: Vec<ProjectPackageFile> = client
        .call(Call::ProjectPackageFiles {
            project: project.clone(),
        })
        .await
        .unwrap();
    assert_eq!(files.len(), 1);
    assert!(!files[0].set_up);
    let setup: ProjectSetupPlan = client
        .call(Call::PlanProjectSetup {
            project: project.clone(),
            file: "renv.lock".into(),
        })
        .await
        .unwrap();
    assert!(setup.refused.is_none(), "{setup:?}");
    assert!(Path::new(&setup.folder).ends_with("analysis/renv/library"));
    assert!(
        setup
            .steps
            .iter()
            .any(|step| step.command.contains("renv::restore"))
    );
    let restore = r_in(&home, &path, &r)
        .env("SANDHILL_TEST_PROJECT", &project).env("SANDHILL_TEST_LIBRARY", &library)
        .arg("options(renv.consent=TRUE); renv::restore(project=Sys.getenv('SANDHILL_TEST_PROJECT'), prompt=FALSE, library=Sys.getenv('SANDHILL_TEST_LIBRARY'), clean=FALSE)").output().unwrap();
    assert!(
        restore.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );

    // A queued worker must not run a package file that changed after Review.
    std::fs::write(&plan.path, format!("{}\n", saved.contents)).unwrap();
    let started: QuickInstalling = client
        .call(Call::RunProjectSetup {
            project,
            file: "renv.lock".into(),
        })
        .await
        .unwrap();
    let exit = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let terminals: Vec<TerminalInfo> = client.call(Call::ListTerminals).await.unwrap();
            if let Some(done) = terminals
                .into_iter()
                .find(|term| term.id == started.job.id && !term.running)
            {
                break done.exit.and_then(|exit| exit.code);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(exit, Some(0));
    assert!(
        std::fs::read_to_string(started.log)
            .unwrap()
            .contains("The package file changed.")
    );
    assert!(!Path::new(&setup.folder).exists());
}
