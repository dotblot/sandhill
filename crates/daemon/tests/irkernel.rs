//! Offering IRkernel to an R that has none, with a real R. The daemon runs
//! as a separate process so it has its own home and Jupyter folders. The
//! install itself downloads from CRAN, so it isn't run here.

mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::client::Client;
use sandhill_proto::{Call, RWithoutIrkernel, TerminalInfo};
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

/// The first `R` on `PATH`, if there is one.
fn real_r() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("R"))
        .find(|r| r.is_file())
}

async fn listed(client: &Client) -> Vec<RWithoutIrkernel> {
    client
        .call(Call::ListRWithoutIrkernel { project: None })
        .await
        .unwrap()
}

fn register_irkernel(jupyter: &Path, r: &Path) {
    let dir = jupyter.join("kernels/ir");
    std::fs::create_dir_all(&dir).unwrap();
    let spec = serde_json::json!({
        "argv": [r, "--slave", "-e", "IRkernel::main()", "--args", "{connection_file}"],
        "display_name": "R",
        "language": "R",
    });
    std::fs::write(dir.join("kernel.json"), spec.to_string()).unwrap();
}

#[tokio::test]
async fn r_without_irkernel_is_offered_until_one_runs_it() {
    let Some(r) = real_r() else {
        eprintln!("skipping: no R on PATH");
        return;
    };
    // The daemon's own home and Jupyter folders are empty, but the system's
    // kernels can't be hidden from it: with an IRkernel there (as on CI's
    // Linux), this R already has one and is rightly not offered.
    let system_wide = [
        "/usr/local/share/jupyter",
        "/usr/share/jupyter",
        "/opt/homebrew/share/jupyter",
    ]
    .iter()
    .filter_map(|dir| std::fs::read_dir(Path::new(dir).join("kernels")).ok())
    .flatten()
    .flatten()
    .filter_map(|kernel| std::fs::read_to_string(kernel.path().join("kernel.json")).ok())
    .any(|spec| spec.contains("IRkernel"));
    if system_wide {
        eprintln!("skipping: an IRkernel is installed system-wide");
        return;
    }
    let tmp = common::tempdir().unwrap();
    let home = tmp.path().join("home");
    let jupyter = tmp.path().join("jupyter");
    std::fs::create_dir_all(&home).unwrap();
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
        .env("JUPYTER_DATA_DIR", &jupyter)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in [
        "JUPYTER_PATH",
        "XDG_DATA_HOME",
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

    // Offered with its version, registering `ir` by R's full path here.
    let shown = r.to_string_lossy().into_owned();
    let found = listed(&client).await;
    let offer = found
        .iter()
        .find(|f| f.r == shown)
        .unwrap_or_else(|| panic!("{shown} not offered: {found:?}"));
    assert!(
        offer
            .version
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit()),
        "{offer:?}"
    );
    assert_eq!(offer.env, None);
    assert!(offer.needs_library, "{offer:?}");

    // Its libraries, as R lists them; installing asks for one and never
    // creates one.
    let libraries = client
        .call::<Vec<sandhill_proto::RLibrary>>(Call::ListRLibraries { r: shown.clone() })
        .await
        .unwrap();
    assert!(!libraries.is_empty(), "no libraries listed");
    assert!(
        libraries.iter().all(|l| Path::new(&l.path).is_dir()),
        "{libraries:?}"
    );
    let refused = client
        .call::<TerminalInfo>(Call::InstallIrkernel {
            r: shown.clone(),
            project: None,
            library: None,
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("choose which of R's libraries"),
        "{refused}"
    );
    assert!(offer.plan.contains("as ir in"), "{}", offer.plan);
    assert!(
        offer
            .plan
            .contains(&jupyter.join("kernels").display().to_string()),
        "{}",
        offer.plan
    );

    // An R that isn't offered can't be installed to.
    let refused = client
        .call::<TerminalInfo>(Call::InstallIrkernel {
            r: "/nowhere/bin/R".into(),
            project: None,
            library: None,
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("isn't an R without IRkernel"), "{refused}");

    // Once an IRkernel runs that R, it is no longer offered.
    register_irkernel(&jupyter, &r);
    assert!(
        listed(&client).await.iter().all(|f| f.r != shown),
        "still offered after registering"
    );
}
