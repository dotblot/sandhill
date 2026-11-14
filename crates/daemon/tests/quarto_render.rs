//! Real one-shot Quarto render jobs, including persisted results after a client
//! disconnects. No substitute renderer: missing Quarto/R dependencies are skipped
//! explicitly, while an installed renderer's failures fail the test.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use sandhill_daemon::kernelspecs::Search;
use sandhill_daemon::terminal::Terminals;
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, JobSpec, TERMINAL_JOB, TerminalEvent, TerminalInfo};

const TIMEOUT: Duration = Duration::from_secs(180);

async fn start(state: &Path) -> (Arc<Daemon>, Client) {
    let socket = state.join("test.sock");
    let options = DaemonOptions {
        executable: env!("CARGO_BIN_EXE_sandhilld").into(),
        socket: socket.clone(),
        started_by: String::new(),
    };
    let daemon = Daemon::open_with(StateDir::new(state), options)
        .await
        .unwrap();
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let client = Client::connect_unix(&socket).await.unwrap();
    (daemon, client)
}

fn quarto_available() -> bool {
    if sandhill_daemon::preview::find_quarto(&Search::from_env()).is_some() {
        true
    } else {
        eprintln!("skipping Quarto render integration test: Quarto is not installed");
        false
    }
}

fn rmarkdown_available() -> bool {
    let path = sandhill_daemon::paths::program_path();
    let mut command = std::process::Command::new("Rscript");
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let result = command
        .args([
            "--vanilla",
            "-e",
            "if (!requireNamespace('rmarkdown', quietly = TRUE) || !requireNamespace('knitr', quietly = TRUE)) quit(status = 42)",
        ])
        .output();
    match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping Quarto knitr integration test: Rscript is not installed");
            false
        }
        Ok(output) if output.status.code() == Some(42) => {
            eprintln!(
                "skipping Quarto knitr integration test: R packages rmarkdown and knitr are required"
            );
            false
        }
        Ok(output) => {
            assert!(
                output.status.success(),
                "Rscript dependency check failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            true
        }
        Err(error) => panic!("cannot check R render dependencies: {error}"),
    }
}

fn spec(file: &Path) -> JobSpec {
    JobSpec {
        path: Some(file.to_string_lossy().into_owned()),
        command: None,
    }
}

async fn info(client: &Client, id: &str) -> TerminalInfo {
    client
        .list_terminals()
        .await
        .unwrap()
        .into_iter()
        .find(|terminal| terminal.id == id)
        .expect("the render job disappeared")
}

async fn wait_for_finish(client: &Client, id: &str) -> TerminalInfo {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let terminal = info(client, id).await;
            if !terminal.running {
                return terminal;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Quarto did not finish")
}

async fn finish_event(client: &mut Client, id: &str) -> (String, TerminalEvent) {
    client.subscribe_terminal(id, Some(0)).await.unwrap();
    let result = tokio::time::timeout(TIMEOUT, async {
        let mut log = Vec::new();
        loop {
            let event = client.next_terminal_event().await.unwrap();
            if event.terminal != id {
                continue;
            }
            if let Some(data) = &event.data {
                log.extend(
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap(),
                );
            }
            if event.exit.is_some() {
                return (String::from_utf8_lossy(&log).into_owned(), event);
            }
        }
    })
    .await
    .expect("Quarto did not send its finish event");
    client.unsubscribe_terminal(id).await.unwrap();
    result
}

fn assert_html_result(
    terminal: &TerminalInfo,
    event: &TerminalEvent,
    log: &str,
    marker: &str,
) -> PathBuf {
    assert_eq!(event.exit.as_ref().unwrap().code, Some(0), "{log}");
    assert_eq!(event.output_paths, terminal.output_paths, "{log}");
    assert_eq!(terminal.output_paths.len(), 1, "{terminal:?}\n{log}");
    let output = PathBuf::from(&terminal.output_paths[0]);
    assert!(output.is_absolute(), "{output:?}");
    assert_eq!(
        output.extension().and_then(|ext| ext.to_str()),
        Some("html")
    );
    let html = std::fs::read_to_string(&output).unwrap();
    assert!(
        html.contains(marker),
        "rendered HTML did not contain {marker}: {output:?}"
    );
    output
}

#[tokio::test]
async fn markdown_render_survives_disconnect_and_discovers_project_output_directory() {
    if !quarto_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    let documents = project.join("documents");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(
        project.join("_quarto.yml"),
        "project:\n  type: default\n  output-dir: _rendered\nformat: html\n",
    )
    .unwrap();
    let file = documents.join("tiny report.qmd");
    std::fs::write(
        &file,
        "---\ntitle: Sandhill render\n---\n\n# SANDHILL_MARKDOWN_RENDER\n\nOnly markdown; no kernel or session is needed.\n",
    )
    .unwrap();
    let state = root.join("state");
    let (_daemon, client) = start(&state).await;
    // An explicitly supplied cwd cannot replace the document directory.
    let launched: TerminalInfo = client
        .call(Call::StartJob {
            job: spec(&file),
            cwd: Some(root.to_string_lossy().into_owned()),
            project: Some(project.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
    assert_eq!(launched.kind, TERMINAL_JOB);
    assert_eq!(launched.cwd, documents.to_string_lossy());
    assert_eq!(launched.job, Some(spec(&file)));
    assert!(launched.running);
    assert!(launched.output_paths.is_empty());
    drop(client);

    // Nothing subscribes while Quarto runs: the independent host must save
    // output paths even without an app, rather than parsing only live events.
    let mut client = Client::connect_unix(&state.join("test.sock"))
        .await
        .unwrap();
    let ended = wait_for_finish(&client, &launched.id).await;
    let (log, event) = finish_event(&mut client, &launched.id).await;
    let output = assert_html_result(&ended, &event, &log, "SANDHILL_MARKDOWN_RENDER");
    assert!(
        output.starts_with(project.join("_rendered")),
        "{output:?}\n{log}"
    );

    // A newly constructed registry reads the results solely from disk, as
    // after a daemon restart. Even losing the retained terminal log cannot
    // erase the separately persisted render result.
    let terminal_dir = StateDir::new(&state).terminals_dir().join(&launched.id);
    for entry in std::fs::read_dir(&terminal_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with("out.") {
            std::fs::remove_file(entry.path()).unwrap();
        }
    }
    let registry = Terminals::new(
        StateDir::new(&state).terminals_dir(),
        env!("CARGO_BIN_EXE_sandhilld").into(),
    );
    assert_eq!(
        registry.info(&launched.id).unwrap().output_paths,
        ended.output_paths
    );
    let (_, replayed) = finish_event(&mut client, &launched.id).await;
    assert_eq!(replayed.output_paths, ended.output_paths);
}

#[tokio::test]
async fn rmarkdown_render_executes_a_fresh_knitr_process() {
    if !quarto_available() || !rmarkdown_available() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    let documents = project.join("analysis");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(documents.join("render-input.txt"), "42\n").unwrap();
    let file = documents.join("tiny.Rmd");
    std::fs::write(
        &file,
        "---\ntitle: Sandhill knitr render\nformat: html\nengine: knitr\n---\n\n```{r}\n#| echo: false\nvalue <- as.integer(readLines('render-input.txt'))\ncat('SANDHILL_R_RENDER_', value, sep = '')\n```\n",
    )
    .unwrap();
    let (_daemon, mut client) = start(&root.join("state")).await;
    let launched = client
        .start_job(spec(&file), Some(&project.to_string_lossy()))
        .await
        .unwrap();
    assert_eq!(launched.cwd, documents.to_string_lossy());
    let (log, event) = finish_event(&mut client, &launched.id).await;
    let ended = info(&client, &launched.id).await;
    assert_html_result(&ended, &event, &log, "SANDHILL_R_RENDER_42");

    // A failed rerun must not advertise the previous successful result, even
    // though Quarto leaves that HTML file on disk and the terminal keeps its log.
    std::fs::write(
        &file,
        "---\nformat: html\nengine: knitr\n---\n\n```{r}\nstop('SANDHILL_EXPECTED_RENDER_FAILURE')\n```\n",
    )
    .unwrap();
    let restarted: TerminalInfo = client
        .call(Call::RestartTerminal {
            terminal: launched.id.clone(),
            options: None,
            cols: None,
            rows: None,
            prompt: None,
        })
        .await
        .unwrap();
    assert!(restarted.output_paths.is_empty());
    let (log, failed) = finish_event(&mut client, &launched.id).await;
    assert_ne!(failed.exit.as_ref().unwrap().code, Some(0), "{log}");
    assert!(log.contains("SANDHILL_EXPECTED_RENDER_FAILURE"), "{log}");
    assert!(failed.output_paths.is_empty());
    assert!(info(&client, &launched.id).await.output_paths.is_empty());
}
