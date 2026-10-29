//! Jobs: scripts and command lines run in the background, each in a
//! terminal of its own, with their log, the CPU and memory they use, how
//! they ended, running again and stopping. Shell scripts run everywhere; a
//! Python script runs with the Python of the project's kernel, and an R
//! script with Rscript, when they are installed, and the test says when
//! they aren't.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use sandhill_daemon::kernelspecs::{self, Search};
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, JobSpec, TERMINAL_JOB, TerminalExit, TerminalInfo};

const TIMEOUT: Duration = Duration::from_secs(60);

/// Busy for a few seconds, so its CPU can be measured, then fails.
const BUSY: &str = r#"echo "start $1 in $(pwd)"
end=$(( $(date +%s) + 3 ))
while [ "$(date +%s)" -lt "$end" ]; do :; done
echo "done"
exit 3
"#;

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

fn job(path: Option<&Path>, command: Option<&str>) -> JobSpec {
    JobSpec {
        path: path.map(|p| p.to_string_lossy().into_owned()),
        command: command.map(String::from),
    }
}

/// A job's whole log and how it ended.
async fn finish(client: &mut Client, terminal: &str) -> (String, TerminalExit) {
    client.subscribe_terminal(terminal, Some(0)).await.unwrap();
    let mut out = Vec::new();
    loop {
        let event = tokio::time::timeout(TIMEOUT, client.next_terminal_event())
            .await
            .expect("the job did not end")
            .unwrap();
        if event.terminal != terminal {
            continue;
        }
        if let Some(data) = event.data {
            out.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .unwrap(),
            );
        }
        if let Some(exit) = event.exit {
            client.unsubscribe_terminal(terminal).await.unwrap();
            return (String::from_utf8_lossy(&out).into_owned(), exit);
        }
    }
}

async fn info(client: &Client, terminal: &str) -> TerminalInfo {
    let list = client.list_terminals().await.unwrap();
    list.into_iter().find(|t| t.id == terminal).unwrap()
}

#[tokio::test]
async fn jobs_run_in_the_background() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    let (_daemon, mut client) = start(&root.join("state")).await;
    let project_path = project.to_string_lossy().into_owned();

    let script = project.join("busy.sh");
    std::fs::write(&script, BUSY).unwrap();
    let spec = job(Some(&script), Some("lane1"));
    let busy = client
        .start_job(spec.clone(), Some(&project_path))
        .await
        .unwrap();
    assert_eq!(busy.kind, TERMINAL_JOB);
    assert_eq!(busy.title, "busy.sh");
    assert_eq!(busy.job, Some(spec));
    assert!(busy.running);
    // On a Mac, the Mac stays awake while it runs.
    if cfg!(target_os = "macos") {
        let ps = std::process::Command::new("ps")
            .args(["-ax", "-o", "command="])
            .output()
            .unwrap();
        let ps = String::from_utf8_lossy(&ps.stdout);
        assert!(
            ps.lines()
                .any(|l| l.starts_with("/usr/bin/caffeinate -i -s -w ")),
            "{ps}"
        );
    }

    // Its CPU and memory, the CPU once it has been measured twice.
    let mut usage = None;
    for _ in 0..20 {
        let stats = client.system_stats(None).await.unwrap();
        usage = stats.jobs.into_iter().find(|j| j.terminal == busy.id);
        if usage.as_ref().is_some_and(|u| u.cpu.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let usage = usage.expect("system_stats did not list the job");
    assert!(usage.rss > 0, "{usage:?}");
    assert!(usage.cpu.is_some_and(|cpu| cpu >= 0.0), "{usage:?}");

    let (log, exit) = finish(&mut client, &busy.id).await;
    assert!(
        log.contains(&format!("start lane1 in {}", project.display())),
        "{log}"
    );
    assert!(log.contains("done"), "{log}");
    assert_eq!(exit.code, Some(3), "{log}");
    let ended = info(&client, &busy.id).await;
    assert!(!ended.running);
    let stats = client.system_stats(None).await.unwrap();
    assert!(!stats.jobs.iter().any(|j| j.terminal == busy.id));

    // Running it again keeps its log; stopping it keeps the job.
    let again: TerminalInfo = client
        .call(Call::RestartTerminal {
            terminal: busy.id.clone(),
            options: None,
            cols: None,
            rows: None,
            prompt: None,
        })
        .await
        .unwrap();
    assert!(again.running);
    assert!(again.started_ms >= ended.started_ms);
    client.stop_terminal(&busy.id).await.unwrap();
    let stopped = tokio::time::timeout(TIMEOUT, async {
        loop {
            let now = info(&client, &busy.id).await;
            if !now.running {
                return now;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the job did not stop");
    let exit = stopped.exit.unwrap();
    assert!(exit.code != Some(3), "{exit:?}");
    assert!(exit.ended_ms >= again.started_ms);

    // A command line runs in the user's shell.
    let shell = client
        .start_job(
            job(None, Some("echo $((6 * 7)) from the shell")),
            Some(&project_path),
        )
        .await
        .unwrap();
    assert_eq!(shell.title, "echo $((6 * 7)) from the shell");
    let (log, exit) = finish(&mut client, &shell.id).await;
    assert!(log.contains("42 from the shell"), "{log}");
    assert_eq!(exit.code, Some(0));

    // A Python script runs with the Python of the project's kernel.
    let search = Search::from_env();
    let python = kernelspecs::discover(&search)
        .into_iter()
        .map(|k| k.dir)
        .find(|k| {
            k.kernelspec.language.eq_ignore_ascii_case("python")
                && k.kernelspec
                    .argv
                    .first()
                    .is_some_and(|p| Path::new(p).is_absolute())
        });
    match python {
        Some(kernel) => {
            let dir = project.join(".sandhill");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("project.toml"),
                format!("[kernels]\npython = \"{}\"\n", kernel.kernel_name),
            )
            .unwrap();
            let script = project.join("which.py");
            std::fs::write(&script, "import sys\nprint('python is', sys.executable)\n").unwrap();
            let py = client
                .start_job(job(Some(&script), None), Some(&project_path))
                .await
                .unwrap();
            let (log, exit) = finish(&mut client, &py.id).await;
            assert_eq!(exit.code, Some(0), "{log}");
            let want = std::fs::canonicalize(&kernel.kernelspec.argv[0]).unwrap();
            let line = log
                .lines()
                .find(|l| l.starts_with("python is "))
                .expect(&log);
            let ran = std::fs::canonicalize(line.trim_start_matches("python is ").trim()).unwrap();
            assert_eq!(ran, want, "{log}");
        }
        None => eprintln!("skipping the Python job: no Python kernel installed"),
    }

    // An R script runs with Rscript.
    let has_r = std::process::Command::new("sh")
        .args(["-c", "command -v Rscript"])
        .output()
        .is_ok_and(|o| o.status.success());
    if has_r {
        let script = project.join("de.R");
        std::fs::write(&script, "cat('R says', commandArgs(TRUE), '\\n')\n").unwrap();
        let r = client
            .start_job(job(Some(&script), Some("hello")), Some(&project_path))
            .await
            .unwrap();
        let (log, exit) = finish(&mut client, &r.id).await;
        assert!(log.contains("R says hello"), "{log}");
        assert_eq!(exit.code, Some(0), "{log}");
    } else {
        eprintln!("skipping the R job: Rscript is not installed");
    }
}
