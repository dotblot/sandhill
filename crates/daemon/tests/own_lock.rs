//! The lock while Sandhill's own R and Python are chosen: a kernel from the
//! own folder can't write into the user's R library, which Sandhill found
//! through that R, while writing into the project works. Apple's sandbox,
//! so a Mac only; skips without a Python that has ipykernel. The only test
//! here: it sets `HOME` and `PATH` for the whole test binary.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{EVENT_RUN_FINISHED, EnvMode, InstallPrefs};
use tokio::net::UnixListener;

/// A Python with ipykernel, found without the daemon's code, which keeps
/// the first `PATH` it sees.
fn python_with_ipykernel() -> Option<PathBuf> {
    [
        "python3",
        "/opt/homebrew/bin/python3",
        "/usr/local/bin/python3",
    ]
    .into_iter()
    .find_map(|python| {
        let out = std::process::Command::new(python)
            .args(["-c", "import ipykernel, sys; print(sys.executable)"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let path = String::from_utf8(out.stdout).ok()?;
        out.status
            .success()
            .then(|| PathBuf::from(path.trim()))
            .filter(|p| p.is_absolute())
    })
}

fn executable(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn a_kernel_cant_write_into_a_locked_library_but_can_into_the_project() {
    if !cfg!(target_os = "macos") || !Path::new("/usr/bin/sandbox-exec").is_file() {
        eprintln!("skipping: the lock's test needs a Mac with Apple's sandbox");
        return;
    }
    let Some(python) = python_with_ipykernel() else {
        eprintln!("skipping: no Python with ipykernel is installed");
        return;
    };
    let dir = common::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let home = base.join("home");
    let library = home.join("Library/R/arm64/4.5/library");
    std::fs::create_dir_all(&library).unwrap();
    // The user's R, as the R and Python page's probe reads it.
    let r_home = home.join("fake-r");
    executable(
        &r_home.join("bin/R"),
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  *--version*) echo 'R version 4.5.1 (2025-06-13)' ;;\n  *) printf 'SANDHILL_HOME\\t%s\\n' '{}'; printf 'SANDHILL_LIBPATH\\t%s\\tyes\\t0\\n' '{}' ;;\nesac\n",
            r_home.display(),
            library.display()
        ),
    );
    // Sandhill's own folder with a Python kernel.
    let own = base.join("own");
    let env = own.join("conda/envs/sandhill-python");
    std::fs::create_dir_all(env.join("conda-meta")).unwrap();
    std::fs::write(
        own.join(".sandhill-environment"),
        "Sandhill's own R and Python\n",
    )
    .unwrap();
    executable(
        &env.join("bin/python"),
        &format!("#!/bin/sh\nexec '{}' \"$@\"\n", python.display()),
    );
    let spec = env.join("share/jupyter/kernels/python3");
    std::fs::create_dir_all(&spec).unwrap();
    std::fs::write(
        spec.join("kernel.json"),
        serde_json::json!({
            "argv": [env.join("bin/python"), "-m", "ipykernel_launcher", "-f", "{connection_file}"],
            "display_name": "Python 3",
            "language": "python",
        })
        .to_string(),
    )
    .unwrap();
    let project = base.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let state = base.join("state");
    std::fs::create_dir_all(&state).unwrap();
    let prefs = InstallPrefs {
        mode: Some(EnvMode::Own),
        own_folder: own.to_string_lossy().into_owned(),
        ..Default::default()
    };
    std::fs::write(
        state.join("install-prefs.json"),
        serde_json::to_vec(&prefs).unwrap(),
    )
    .unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(r_home.join("bin")).chain(std::env::split_paths(&path)),
    )
    .unwrap();
    // SAFETY: the only test in this binary, set before anything reads them.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", path);
    }

    let daemon = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon: Arc<Daemon> = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let mut client = Client::connect_unix(&socket).await.unwrap();
    let kernels = client.list_kernelspecs().await.unwrap();
    let kernel = kernels
        .iter()
        .find(|k| k.language.eq_ignore_ascii_case("python"))
        .unwrap_or_else(|| panic!("no kernel from the own folder: {kernels:?}"));
    let info = client
        .start_session(&kernel.name, Some(project.to_str().unwrap()))
        .await
        .unwrap();
    client.subscribe(&info.id, None).await.unwrap();
    let code = format!(
        r#"def attempt(path):
    try:
        with open(path, "w") as f:
            f.write("x")
        return "written"
    except OSError as e:
        return e.strerror
print(attempt({:?}))
print(attempt({:?}))
"#,
        library.join("sandhill-test.txt").display().to_string(),
        project.join("sandhill-test.txt").display().to_string(),
    );
    let run = client.execute(&info.id, &code, None).await.unwrap().run;
    let mut stdout = String::new();
    tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let event = client.next_event().await.expect("connection closed");
            if event.run.as_deref() != Some(run.as_str()) {
                continue;
            }
            if event.msg_type == EVENT_RUN_FINISHED {
                break;
            }
            if event.msg_type == "stream" && event.content["name"] == "stdout" {
                stdout.push_str(event.content["text"].as_str().unwrap_or_default());
            }
        }
    })
    .await
    .expect("the run never finished");
    assert_eq!(stdout, "Operation not permitted\nwritten\n");
    assert!(!library.join("sandhill-test.txt").exists());
    assert!(project.join("sandhill-test.txt").exists());
    let _ = client.shutdown(&info.id).await;
}
