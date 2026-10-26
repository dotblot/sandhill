//! Temporary probe: what Ark and IRkernel send for htmlwidgets.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::EVENT_RUN_FINISHED;
use sandhill_proto::client::Client;
use tokio::process::Command;

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");

const CODE: &[&str] = &[
    "cat('interactive:', interactive(), '\\n'); cat('tempdir:', tempdir(), '\\n')",
    "print(getOption('viewer'))",
    "print(getOption('browser')); print(getOption('shiny.launch.browser')); print(getOption('page_viewer'))",
    "w <- DT::datatable(head(iris)); w",
    "print(w)",
    "htmltools::browsable(htmltools::tags$b('hi'))",
    "list.files(tempdir(), recursive = TRUE)",
];

#[tokio::test]
async fn probe() {
    let ark = std::env::var_os("SANDHILL_TEST_ARK").map(PathBuf::from);
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    if let Some(ark) = &ark {
        let installed = home.join(sandhill_proto::ARK_PATH);
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        std::fs::copy(ark, &installed).unwrap();
    }
    let state = home.join("state");
    let mut command = Command::new(DAEMON);
    command
        .arg("--state-dir")
        .arg(&state)
        .arg("serve")
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in ["JUPYTER_PATH", "JUPYTER_DATA_DIR", "XDG_DATA_HOME"] {
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
    let mut client = client.expect("sandhilld did not start");
    let specs = client.list_kernelspecs().await.unwrap();
    eprintln!("KERNELS {:?}", specs.iter().map(|k| &k.name).collect::<Vec<_>>());
    for kernel in ["ark", "ir"] {
        if !specs.iter().any(|k| k.name == kernel) {
            eprintln!("PROBE no {kernel}");
            continue;
        }
        let session = client.start_session(kernel, None).await.unwrap().id;
        client.subscribe(&session, None).await.unwrap();
        for code in CODE {
            eprintln!("\n===== {kernel}: {code}");
            let run = client.execute(&session, code, None).await.unwrap().run;
            loop {
                let Ok(Some(event)) =
                    tokio::time::timeout(Duration::from_secs(60), client.next_event()).await
                else {
                    eprintln!("TIMEOUT");
                    break;
                };
                if event.run.as_deref() != Some(run.as_str()) {
                    eprintln!("(other run) {} {}", event.msg_type, trunc(&event.content.to_string(), 600));
                    continue;
                }
                if event.msg_type == EVENT_RUN_FINISHED {
                    break;
                }
                let c = &event.content;
                match event.msg_type.as_str() {
                    "display_data" | "execute_result" | "update_display_data" => {
                        let data = &c["data"];
                        for (mime, value) in data.as_object().unwrap() {
                            let text = value.as_str().map(String::from).unwrap_or_else(|| value.to_string());
                            eprintln!(
                                "{} {mime} len={} scriptsrc={} : {}",
                                event.msg_type,
                                text.len(),
                                text.contains("<script src"),
                                trunc(&text, 1500)
                            );
                        }
                        eprintln!("  metadata: {}", trunc(&c["metadata"].to_string(), 400));
                    }
                    _ => eprintln!("{} {}", event.msg_type, trunc(&c.to_string(), 3000)),
                }
            }
        }
        client.shutdown(&session).await.unwrap();
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
