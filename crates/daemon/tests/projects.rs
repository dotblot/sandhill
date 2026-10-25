//! Project settings against real kernels: the environment a project's files
//! point to, `.sandhill/project.toml`, env files, and R starting in the
//! project's folder. Each test skips (and says so) when its kernel isn't
//! installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::mcp::Tools;
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, DocumentsRunIn, EVENT_RUN_FINISHED, KernelspecInfo, ProjectSettings, ProjectSetup,
    SessionInfo,
};
use serde_json::{Value, json};
use tokio::net::UnixListener;

const TIMEOUT: Duration = Duration::from_secs(90);

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
        let socket = state.join("test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        TestDaemon {
            _daemon: daemon,
            socket,
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}

async fn have_kernel(name: &str) -> bool {
    let found = sandhill_daemon::kernel::find_kernelspec(name, None)
        .await
        .is_ok();
    if !found {
        eprintln!("skipping: Jupyter kernel {name:?} is not installed");
    }
    found
}

fn python(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("python3")
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Makes `<prefix>` a venv that sees the system's ipykernel, as `uv sync`
/// would make `.venv` with ipykernel among a project's dependencies.
fn venv_with_ipykernel(prefix: &Path) -> bool {
    let Some(launcher) = python(&["-c", "import ipykernel_launcher as l; print(l.__file__)"])
    else {
        return false;
    };
    let made = std::process::Command::new("python3")
        .args(["-m", "venv", "--without-pip", "--system-site-packages"])
        .arg(prefix)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        return false;
    }
    let output = std::process::Command::new(prefix.join("bin/python"))
        .args([
            "-c",
            "import sysconfig; print(sysconfig.get_paths()['purelib'])",
        ])
        .output()
        .unwrap();
    let site = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    std::fs::create_dir_all(&site).unwrap();
    // What tells Sandhill (and Jupyter) that ipykernel is installed here.
    std::os::unix::fs::symlink(&launcher, site.join("ipykernel_launcher.py")).unwrap();
    true
}

async fn setup(client: &Client, project: &Path) -> ProjectSetup {
    client
        .call(Call::GetProjectSettings {
            path: project.to_string_lossy().into(),
        })
        .await
        .unwrap()
}

async fn start(client: &Client, kernel: &str, cwd: &Path, project: &Path) -> SessionInfo {
    client
        .call(Call::StartSession {
            kernel: kernel.into(),
            cwd: Some(cwd.to_string_lossy().into()),
            project: Some(project.to_string_lossy().into()),
        })
        .await
        .unwrap()
}

/// Runs code and returns its stdout.
async fn run(client: &mut Client, session: &str, code: &str) -> String {
    client.subscribe(session, None).await.unwrap();
    let run = client.execute(session, code, None).await.unwrap().run;
    let mut out = String::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_event())
            .await
            .expect("timed out waiting for the run")
            .expect("connection closed");
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == "stream" {
            out.push_str(event.content["text"].as_str().unwrap_or_default());
        }
        if event.msg_type == "error" {
            panic!("the run failed: {}", event.content);
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            return out;
        }
    }
}

#[tokio::test]
async fn a_project_runs_python_in_its_own_venv_with_its_env_file() {
    if !have_kernel("python3").await {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // Python reports paths with symlinks resolved (/var is /private/var on
    // macOS), so start resolved.
    let base = tmp.path().canonicalize().unwrap();
    let project = base.join("atlas");
    let venv = project.join(".venv");
    std::fs::create_dir_all(project.join("analysis")).unwrap();
    if !venv_with_ipykernel(&venv) {
        eprintln!("skipping: can't make a venv with ipykernel (python3 -m venv)");
        return;
    }
    std::fs::write(project.join("uv.lock"), "").unwrap();
    std::fs::write(
        project.join(".env"),
        "# Not committed\nSANDHILL_TOKEN=\"abc def\"\nSANDHILL_DATA=${SANDHILL_TOKEN}/counts\n",
    )
    .unwrap();
    std::fs::write(project.join("local.env"), "SANDHILL_OTHER=1\n").unwrap();
    let daemon = TestDaemon::start(&base.join("state")).await;
    let mut client = daemon.client().await;

    // The folder points to its .venv, which runs Python unasked.
    let found = setup(&client, &project).await;
    assert!(!found.saved, "{found:?}");
    assert_eq!(
        found.python.as_deref(),
        Some("project-venv-py"),
        "{found:?}"
    );
    assert_eq!(found.found[0].file, ".venv");
    assert_eq!(found.found[0].hint, None);
    let listed: Vec<KernelspecInfo> = client
        .call(Call::ListProjectKernelspecs {
            path: project.to_string_lossy().into(),
        })
        .await
        .unwrap();
    let venv_kernel = listed.iter().find(|k| k.name == "project-venv-py");
    assert_eq!(
        venv_kernel.map(|k| k.display_name.as_str()),
        Some("Python (.venv)")
    );
    let everywhere = client.list_kernelspecs().await.unwrap();
    assert!(!everywhere.iter().any(|k| k.name == "project-venv-py"));

    // It runs in the venv, in the document's folder, with the .env's variables.
    let docs = project.join("analysis");
    let session = start(&client, "project-venv-py", &docs, &project).await;
    let code = "import os, sys\nprint(sys.prefix)\nprint(os.getcwd())\nprint(os.environ['SANDHILL_TOKEN'])\nprint(os.environ['SANDHILL_DATA'])\nprint(os.environ['VIRTUAL_ENV'])";
    let out = run(&mut client, &session.id, code).await;
    let venv_path = venv.to_string_lossy();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            &*venv_path,
            &*docs.to_string_lossy(),
            "abc def",
            "abc def/counts",
            &*venv_path,
        ]
    );
    client.shutdown(&session.id).await.unwrap();

    // Saving writes .sandhill/project.toml; other kernels and env files follow it.
    let settings = ProjectSettings {
        r: None,
        python: Some("python3".into()),
        documents: DocumentsRunIn::Project,
        env_files: vec!["local.env".into()],
    };
    let saved: ProjectSetup = client
        .call(Call::SaveProjectSettings {
            path: project.to_string_lossy().into(),
            settings: settings.clone(),
        })
        .await
        .unwrap();
    assert!(saved.saved);
    assert_eq!(saved.settings, settings);
    assert_eq!(saved.python.as_deref(), Some("python3"));
    let toml = std::fs::read_to_string(project.join(".sandhill/project.toml")).unwrap();
    assert!(toml.contains("python = \"python3\""), "{toml}");
    assert!(toml.contains("r = \"detect\""), "{toml}");
    assert!(toml.contains("documents = \"project\""), "{toml}");
    assert!(toml.contains("files = [\"local.env\"]"), "{toml}");
    let ignore = std::fs::read_to_string(project.join(".sandhill/.gitignore")).unwrap();
    assert!(ignore.contains("outputs/"), "{ignore}");
    assert_eq!(setup(&client, &project).await, saved);

    let session = start(&client, "python3", &project, &project).await;
    let code = "import os\nprint(os.environ.get('SANDHILL_TOKEN', '-'))\nprint(os.environ['SANDHILL_OTHER'])";
    let out = run(&mut client, &session.id, code).await;
    assert_eq!(out.lines().collect::<Vec<_>>(), ["-", "1"]);
    client.shutdown(&session.id).await.unwrap();

    // Agents' tools run the project's Python too: with "detect", the .venv
    // the folder points to.
    std::fs::write(
        project.join(".sandhill/project.toml"),
        toml.replace("python = \"python3\"", "python = \"detect\""),
    )
    .unwrap();
    let tools = Tools::new(
        daemon.socket.clone(),
        Some(project.to_string_lossy().into()),
    );
    let ran = tool(
        &tools,
        "run_code",
        json!({"code": "import sys\nprint(sys.prefix)", "language": "python"}),
    )
    .await;
    assert!(ran.contains(&*venv_path), "{ran}");
    for s in client.list_sessions().await.unwrap() {
        let _ = client.shutdown(&s.id).await;
    }
}

#[tokio::test]
async fn r_starts_in_the_project_folder_then_moves_to_the_documents() {
    let mut kernel = None;
    for name in ["ir", "ark"] {
        if sandhill_daemon::kernel::find_kernelspec(name, None)
            .await
            .is_ok()
        {
            kernel = Some(name);
            break;
        }
    }
    let Some(kernel) = kernel else {
        eprintln!("skipping: no R kernel (IRkernel or Ark) is installed");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let project = base.join("pbmc");
    let docs = project.join("reports");
    std::fs::create_dir_all(&docs).unwrap();
    // What renv's .Rprofile does, in short: set the project up as R starts.
    std::fs::write(
        project.join(".Rprofile"),
        "options(sandhill.profile = getwd())\n",
    )
    .unwrap();
    std::fs::write(project.join(".Renviron"), "SANDHILL_RENVIRON=yes\n").unwrap();
    std::fs::write(project.join(".env"), "SANDHILL_DOTENV=1\n").unwrap();
    std::fs::write(project.join("renv.lock"), "{}").unwrap();
    let daemon = TestDaemon::start(&base.join("state")).await;
    let mut client = daemon.client().await;

    let found = setup(&client, &project).await;
    assert_eq!(found.found[0].file, "renv.lock", "{found:?}");
    assert!(found.r.is_some(), "{found:?}");

    let session = start(&client, kernel, &docs, &project).await;
    assert_eq!(session.cwd, docs.to_string_lossy());
    let code = r#"cat(getOption("sandhill.profile"), Sys.getenv("SANDHILL_RENVIRON"), Sys.getenv("SANDHILL_DOTENV"), getwd(), sep = "\n")"#;
    let out = run(&mut client, &session.id, code).await;
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            &*project.to_string_lossy(),
            "yes",
            "1",
            &*docs.to_string_lossy(),
        ]
    );
    client.shutdown(&session.id).await.unwrap();
}

/// The text of a tool's reply.
async fn tool(tools: &Tools, name: &str, args: Value) -> String {
    let items = tokio::time::timeout(TIMEOUT, tools.call(name, &args))
        .await
        .expect("tool timed out")
        .unwrap_or_else(|e| panic!("{name} failed: {e:#}"));
    items
        .iter()
        .filter_map(|i| i["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}
