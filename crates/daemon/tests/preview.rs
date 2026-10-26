//! Quarto Preview: `quarto preview` in a terminal of its own, its page
//! reached once it answers, one preview per document, and what is said
//! when Quarto fails or is missing. A stand-in for Quarto (a script that
//! serves a page) runs everywhere; the real Quarto runs too when it is
//! installed, and the test says when it isn't.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::TERMINAL_PREVIEW;
use sandhill_proto::client::Client;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};

const TIMEOUT: Duration = Duration::from_secs(120);

/// Does what `quarto preview FILE --no-browser --port N` does, roughly:
/// fails on a document named "broken", else serves a page naming it.
const FAKE_QUARTO: &str = r#"#!/bin/sh
file="$2"
port=""
while [ $# -gt 0 ]; do
  if [ "$1" = "--port" ]; then port="$2"; fi
  shift
done
case "$file" in
  *broken*) echo "ERROR: YAML parse error in $file"; exit 1 ;;
esac
dir=$(mktemp -d)
echo "<h1>$(basename "$file")</h1>" > "$dir/index.html"
echo "Browse at http://localhost:$port/"
exec python3 -m http.server "$port" --bind 127.0.0.1 --directory "$dir"
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
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let client = Client::connect_unix(&socket).await.unwrap();
    (daemon, client)
}

/// The body of `GET /` at `url`, or none if nothing answers.
async fn get(url: &str) -> Option<String> {
    let port: u16 = url
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches('/')
        .parse()
        .unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    let request = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = String::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_string(&mut response))
        .await
        .ok()?
        .ok()?;
    Some(response)
}

#[tokio::test]
async fn documents_are_previewed_with_quarto() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    // Looked for before QUARTO_PATH points to the stand-in.
    let real =
        sandhill_daemon::preview::find_quarto(&sandhill_daemon::kernelspecs::Search::from_env());
    let fake = root.join("fake-quarto");
    std::fs::write(&fake, FAKE_QUARTO).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // SAFETY: this is the only test in this binary, and the daemon reads
    // the variable only while answering this test's calls.
    unsafe { std::env::set_var("QUARTO_PATH", &fake) };
    let (_daemon, client) = start(&root.join("state")).await;

    let doc = root.join("report.qmd");
    std::fs::write(
        &doc,
        "---\ntitle: Sandhill cranes\n---\n\nAlways in pairs.\n",
    )
    .unwrap();
    let doc = doc.to_string_lossy().into_owned();
    let preview = tokio::time::timeout(TIMEOUT, client.preview_document(&doc))
        .await
        .unwrap()
        .unwrap();
    assert!(preview.url.starts_with("http://127.0.0.1:"), "{preview:?}");
    assert_eq!(preview.terminal.kind, TERMINAL_PREVIEW);
    assert_eq!(preview.terminal.title, "Preview report.qmd");
    let page = get(&preview.url).await.unwrap();
    assert!(page.contains("<h1>report.qmd</h1>"), "{page}");

    // The same document's preview is used again.
    let again = client.preview_document(&doc).await.unwrap();
    assert_eq!(again.url, preview.url);
    assert_eq!(again.terminal.id, preview.terminal.id);

    // What Quarto said, when it stops first.
    let broken = root.join("broken.qmd");
    std::fs::write(&broken, "---\ntitle: [\n---\n").unwrap();
    let err = client
        .preview_document(&broken.to_string_lossy())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("Quarto stopped before showing the page"),
        "{err}"
    );
    assert!(err.contains("YAML parse error"), "{err}");

    let notes = root.join("notes.txt");
    std::fs::write(&notes, "hi").unwrap();
    let err = client
        .preview_document(&notes.to_string_lossy())
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a Quarto document"), "{err}");

    // Closing its terminal ends the preview.
    client.close_terminal(&preview.terminal.id).await.unwrap();
    let mut gone = false;
    for _ in 0..50 {
        if get(&preview.url).await.is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(gone, "the preview still answers");

    // Quarto itself, when it is installed: the page, then again once the
    // document is saved.
    let Some(quarto) = real else {
        eprintln!("skipping the real Quarto: it is not installed");
        return;
    };
    // SAFETY: as above.
    unsafe { std::env::set_var("QUARTO_PATH", &quarto) };
    let preview = tokio::time::timeout(TIMEOUT, client.preview_document(&doc))
        .await
        .unwrap()
        .unwrap();
    let page = get(&preview.url).await.unwrap();
    assert!(page.contains("Sandhill cranes"), "{page}");
    std::fs::write(
        &doc,
        "---\ntitle: Sandhill cranes\n---\n\n## Seen in Wisconsin\n",
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if get(&preview.url)
            .await
            .is_some_and(|page| page.contains("Seen in Wisconsin"))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the preview didn't render the saved document"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    client.close_terminal(&preview.terminal.id).await.unwrap();
}
