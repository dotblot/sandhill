//! Real R installs, Undo, local package files and Check and Repair.

mod common;

use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, EnvAction, EnvActionPlan, EnvCard, InstallPrefs, InstallTarget, InstallTool,
    InstalledKey, PackageAction, PackageChange, PackageHit, PackagePlan, QuickInstalling,
    TerminalInfo,
};
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");
/// A tiny CRAN package written in R alone, with no dependencies.
const PACKAGE: &str = "praise";
const REASON: &str = "The install test needs it";

fn real_r() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("R"))
        .find(|r| r.is_file() && r.with_file_name("Rscript").is_file())
}

fn cran_reachable() -> bool {
    ("cloud.r-project.org", 443)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .is_some_and(|address| TcpStream::connect_timeout(&address, Duration::from_secs(5)).is_ok())
}

async fn finished(client: &Client, started: &QuickInstalling) -> TerminalInfo {
    for _ in 0..1200 {
        let terminals: Vec<TerminalInfo> = client.call(Call::ListTerminals).await.unwrap();
        if let Some(job) = terminals
            .into_iter()
            .find(|t| t.id == started.job.id && !t.running)
        {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!(
        "the job didn't finish: {}",
        std::fs::read_to_string(&started.log).unwrap_or_default()
    );
}

fn succeeded(job: &TerminalInfo, started: &QuickInstalling) {
    assert_eq!(
        job.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "{}",
        std::fs::read_to_string(&started.log).unwrap_or_default()
    );
}

#[tokio::test]
async fn r_package_change_can_be_undone_and_local_package_checked() {
    let Some(r) = real_r() else {
        eprintln!("skipping: no R with Rscript on PATH");
        return;
    };
    if !cran_reachable() {
        eprintln!("skipping: CRAN (cloud.r-project.org) can't be reached");
        return;
    }
    let tmp = common::tempdir().unwrap();
    let home = tmp.path().join("home");
    let library = tmp.path().join("library");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&library).unwrap();
    let library = library.canonicalize().unwrap();
    let state = home.join("state");
    let path = std::env::join_paths([
        r.parent().unwrap(),
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .unwrap();
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env("HOME", &home)
        .env("PATH", &path)
        .env("SHELL", "/bin/bash")
        .env("JUPYTER_DATA_DIR", tmp.path().join("jupyter"))
        .env("R_LIBS_USER", &library)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in [
        "ZDOTDIR",
        "R_LIBS",
        "R_LIBS_SITE",
        "R_PROFILE_USER",
        "R_ENVIRON_USER",
        "CONDA_EXE",
        "CONDA_PREFIX",
        "MAMBA_ROOT_PREFIX",
    ] {
        command.env_remove(var);
    }
    let _serve = command.spawn().unwrap();
    let socket = state.join("sandhilld.sock");
    let mut client = None;
    for _ in 0..100 {
        if let Ok(c) = Client::connect_unix(&socket).await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let client = client.expect("sandhilld did not start");

    let target = InstallTarget::R {
        r: r.to_string_lossy().into_owned(),
        library: library.to_string_lossy().into_owned(),
    };
    let change = PackageChange {
        action: PackageAction::Install,
        target: target.clone(),
        packages: vec![PACKAGE.into()],
        tool: None,
        reason: REASON.into(),
    };
    let plan: PackagePlan = client
        .call(Call::PlanPackages {
            change: change.clone(),
        })
        .await
        .unwrap();
    assert!(plan.refused.is_none(), "{plan:?}");
    assert_eq!(plan.change.tool, Some(InstallTool::R));
    assert_eq!(plan.folder, library.to_string_lossy());
    assert!(
        plan.command.contains("Rscript") && plan.command.contains("install.packages"),
        "{}",
        plan.command
    );
    // With BiocManager in R's libraries, BiocManager installs CRAN packages too.
    assert!(
        ["CRAN", "CRAN or Bioconductor"].contains(&plan.packages[0].source.as_str()),
        "{plan:?}"
    );

    let started: QuickInstalling = client.call(Call::RunPackages { change }).await.unwrap();
    succeeded(&finished(&client, &started).await, &started);
    assert!(library.join(PACKAGE).join("DESCRIPTION").is_file());
    let record = std::fs::read_to_string(state.join("installed-by-sandhill.tsv")).unwrap();
    let line = record
        .lines()
        .find(|line| line.starts_with(&format!("r-package\t{PACKAGE}\t")))
        .unwrap_or_else(|| panic!("not recorded: {record}"));
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields[3], library.to_string_lossy());
    assert_eq!(&fields[5..], ["r", REASON]);
    let prefs: InstallPrefs = client.call(Call::InstallPrefs).await.unwrap();
    assert_eq!(
        prefs.last_for("r").map(|choice| &choice.target),
        Some(&target)
    );

    let cards: Vec<EnvCard> = client.call(Call::ListEnvironments).await.unwrap();
    let env = cards
        .into_iter()
        .find(|card| Path::new(&card.program).canonicalize().ok() == r.canonicalize().ok())
        .expect("R is listed")
        .id;
    let undo = EnvAction::Undo { env: env.clone() };
    let plan: EnvActionPlan = client
        .call(Call::PlanEnvAction {
            action: undo.clone(),
        })
        .await
        .unwrap();
    assert!(plan.refused.is_none(), "{plan:?}");
    assert!(
        plan.notes.iter().any(|note| note.contains("Remove praise")),
        "{plan:?}"
    );
    let started: QuickInstalling = client
        .call(Call::RunEnvAction {
            action: undo.clone(),
        })
        .await
        .unwrap();
    succeeded(&finished(&client, &started).await, &started);
    assert!(!library.join(PACKAGE).exists());
    let plan: EnvActionPlan = client
        .call(Call::PlanEnvAction { action: undo })
        .await
        .unwrap();
    assert!(plan.refused.is_some(), "{plan:?}");
    let record = std::fs::read_to_string(state.join("installed-by-sandhill.tsv")).unwrap();
    assert!(!record.contains(&format!("\t{PACKAGE}\t")), "{record}");

    const LOCAL: &str = "sandhilllocalcheck";
    let pkg = tmp.path().join(LOCAL);
    std::fs::create_dir_all(pkg.join("R")).unwrap();
    std::fs::write(pkg.join("DESCRIPTION"), format!(
        "Package: {LOCAL}\nVersion: 0.1.0\nTitle: Sandhill Local Package Test\nDescription: A tiny package for an installation test.\nAuthors@R: person('Sandhill', 'Test', email='test@example.com', role=c('aut','cre'))\nLicense: MIT\n"
    )).unwrap();
    std::fs::write(pkg.join("NAMESPACE"), "export(example_value)\n").unwrap();
    std::fs::write(pkg.join("R/example.R"), "example_value <- function() 1L\n").unwrap();
    let output = Command::new(&r)
        .args(["CMD", "build", "--no-build-vignettes", "--no-manual", LOCAL])
        .current_dir(tmp.path())
        .env("HOME", &home)
        .env("PATH", &path)
        .env("R_LIBS_USER", &library)
        .env_remove("R_LIBS")
        .env_remove("R_PROFILE_USER")
        .env_remove("R_ENVIRON_USER")
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let file = tmp
        .path()
        .join(format!("{LOCAL}_0.1.0.tar.gz"))
        .to_string_lossy()
        .into_owned();
    let hits: Vec<PackageHit> = client
        .call(Call::SearchPackages {
            env: env.clone(),
            query: file.clone(),
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, "This file");
    let change = PackageChange {
        action: PackageAction::Install,
        target,
        packages: vec![file.clone()],
        tool: Some(InstallTool::R),
        reason: REASON.into(),
    };
    let plan: PackagePlan = client
        .call(Call::PlanPackages {
            change: change.clone(),
        })
        .await
        .unwrap();
    assert!(plan.refused.is_none(), "{plan:?}");
    assert!(plan.command.contains("repos = NULL"), "{plan:?}");
    let started: QuickInstalling = client.call(Call::RunPackages { change }).await.unwrap();
    succeeded(&finished(&client, &started).await, &started);
    let description = library.join(LOCAL).join("DESCRIPTION");
    assert!(description.is_file());
    let text = std::fs::read_to_string(&description)
        .unwrap()
        .lines()
        .map(|line| {
            if line.starts_with("Built:") {
                "Built: R 3.6.0; ; ; unix"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&description, format!("{text}\n")).unwrap();
    let check = EnvAction::Check { env: env.clone() };
    let plan: EnvActionPlan = client
        .call(Call::PlanEnvAction {
            action: check.clone(),
        })
        .await
        .unwrap();
    assert!(plan.refused.is_none(), "{plan:?}");
    let started: QuickInstalling = client
        .call(Call::RunEnvAction { action: check })
        .await
        .unwrap();
    succeeded(&finished(&client, &started).await, &started);
    let cards: Vec<EnvCard> = client.call(Call::ListEnvironments).await.unwrap();
    let card = cards.into_iter().find(|card| card.id == env).unwrap();
    assert!(card.needs_repair.iter().any(|p| p == LOCAL), "{card:?}");
    let report = card.check.unwrap();
    let problem = report
        .problems
        .iter()
        .find(|p| p.package == LOCAL)
        .expect("Check found the deliberately broken package");
    assert!(problem.reason.contains("Built for R 3.6"), "{problem:?}");
    let fix = problem.fix.as_ref().expect("Check offered a fix");
    assert_eq!(fix.reason, "Check and Repair");
    assert_eq!(fix.action, PackageAction::Update);
    assert_eq!(fix.packages, [file]);
    let started: QuickInstalling = client
        .call(Call::RunPackages {
            change: fix.clone(),
        })
        .await
        .unwrap();
    succeeded(&finished(&client, &started).await, &started);
    assert!(
        !std::fs::read_to_string(&description)
            .unwrap()
            .contains("Built: R 3.6.0")
    );
    let key = InstalledKey {
        kind: "r-package".into(),
        name: LOCAL.into(),
        place: library.to_string_lossy().into_owned(),
    };
    let started: QuickInstalling = client
        .call(Call::RemoveInstalled { items: vec![key] })
        .await
        .unwrap();
    succeeded(&finished(&client, &started).await, &started);
    assert!(!library.join(LOCAL).exists());
    let record = std::fs::read_to_string(state.join("installed-by-sandhill.tsv")).unwrap();
    assert!(!record.contains(&format!("\t{LOCAL}\t")), "{record}");
    let log = std::fs::read_to_string(&started.log).unwrap();
    assert!(log.contains("Done: 1 removed"), "{log}");
}
