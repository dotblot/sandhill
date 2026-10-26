//! The Viewer's pages: files on the daemon's machine served over HTTP on
//! `127.0.0.1`, so the app's web view (through a tunnel, from a Mac) or a
//! browser can show an HTML report, or the page an htmlwidget was saved
//! to, with the pictures, scripts and styles beside it.
//!
//! A page's address is `/<signature>/<folder>/<file>`: the folder it may
//! read from, signed with a key kept in the state folder. A page can't read
//! files outside its folder, nobody can make an address without the key,
//! and addresses outlive the daemon (its port doesn't).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use sandhill_proto::PageUrl;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::OnceCell;

/// Pages of outputs, newest kept.
const KEEP_HTML: usize = 50;
/// The most a request's head may hold.
const MAX_HEAD: usize = 64 * 1024;

pub struct Pages {
    key: Arc<String>,
    /// Where output HTML is written to be served.
    dir: PathBuf,
    port: OnceCell<u16>,
}

impl Pages {
    /// Reads the key from `state`, making one the first time.
    pub fn open(state: &Path) -> Result<Pages> {
        let path = state.join("pages.key");
        let key = match std::fs::read_to_string(&path) {
            Ok(key) if key.trim().len() >= 32 => key.trim().to_string(),
            _ => {
                let key = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                write_private(&path, &key)?;
                key
            }
        };
        Ok(Pages {
            key: Arc::new(key),
            dir: state.join("pages"),
            port: OnceCell::new(),
        })
    }

    /// Serves a file, or a folder's `index.html`, with the folder it is in.
    pub async fn serve(&self, path: &Path) -> Result<PageUrl> {
        let path = std::fs::canonicalize(path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        let file = if path.is_dir() {
            path.join("index.html")
        } else {
            path
        };
        if !file.is_file() {
            bail!("{} isn't a file", file.display());
        }
        let folder = file.parent().context("no folder")?;
        let port = self.port().await?;
        Ok(PageUrl {
            port,
            path: format!("/{}{}", sign(&self.key, folder), encode(&file)),
        })
    }

    /// Writes HTML to a page of its own and serves it.
    pub async fn serve_html(&self, html: &str) -> Result<PageUrl> {
        let hash = format!("{:x}", Sha256::digest(html.as_bytes()));
        let folder = self.dir.join(&hash[..16]);
        std::fs::create_dir_all(&folder)?;
        let file = folder.join("index.html");
        if !file.is_file() {
            std::fs::write(&file, html)?;
            prune(&self.dir, KEEP_HTML);
        }
        self.serve(&file).await
    }

    /// The server's port, started the first time.
    async fn port(&self) -> Result<u16> {
        let key = self.key.clone();
        self.port
            .get_or_try_init(|| async move {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
                let port = listener.local_addr()?.port();
                tokio::spawn(accept(listener, key));
                Ok::<u16, anyhow::Error>(port)
            })
            .await
            .copied()
    }
}

async fn accept(listener: TcpListener, key: Arc<String>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(connection(stream, key.clone()));
    }
}

/// Answers a connection's requests in turn (HTTP/1.1 keep-alive).
async fn connection(stream: TcpStream, key: Arc<String>) {
    let _ = stream.set_nodelay(true);
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let Some(request) = read_head(&mut reader).await else {
            return;
        };
        let keep = request.keep_alive;
        if answer(&mut writer, &request, &key).await.is_err() || !keep {
            return;
        }
    }
}

struct RequestHead {
    method: String,
    target: String,
    keep_alive: bool,
}

async fn read_head<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Option<RequestHead> {
    let mut line = String::new();
    let mut total = 0;
    // Browsers may send a blank line between requests.
    loop {
        line.clear();
        total += reader.read_line(&mut line).await.ok()?;
        if line.is_empty() {
            return None;
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let version = parts.next().unwrap_or("HTTP/1.0").to_string();
    let mut keep_alive = version == "HTTP/1.1";
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).await.ok()?;
        total += n;
        if n == 0 || total > MAX_HEAD {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        let value = value.trim().to_ascii_lowercase();
        match name.trim().to_ascii_lowercase().as_str() {
            "connection" if value == "close" => keep_alive = false,
            "connection" if value == "keep-alive" => keep_alive = true,
            "content-length" => length = value.parse().unwrap_or(0),
            _ => {}
        }
    }
    // Pages only read, but a body sent anyway is skipped.
    if length > 0 {
        let mut body = reader.take(length as u64);
        tokio::io::copy(&mut body, &mut tokio::io::sink())
            .await
            .ok()?;
    }
    Some(RequestHead {
        method,
        target,
        keep_alive,
    })
}

async fn answer<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    request: &RequestHead,
    key: &str,
) -> std::io::Result<()> {
    if request.method != "GET" && request.method != "HEAD" {
        return plain(writer, "405 Method Not Allowed", "Pages can only be read.").await;
    }
    let file = match resolve(key, &request.target) {
        Ok(file) => file,
        Err(status) => return plain(writer, status, "There is no such page.").await,
    };
    let Ok(mut opened) = tokio::fs::File::open(&file).await else {
        return plain(writer, "404 Not Found", "There is no such page.").await;
    };
    let length = opened.metadata().await.map(|m| m.len()).unwrap_or(0);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        content_type(&file)
    );
    writer.write_all(head.as_bytes()).await?;
    if request.method == "GET" {
        tokio::io::copy(&mut opened, writer).await?;
    }
    writer.flush().await
}

async fn plain<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    status: &str,
    text: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
        text.len()
    );
    writer.write_all(head.as_bytes()).await?;
    writer.write_all(text.as_bytes()).await?;
    writer.flush().await
}

/// The file a request's target names, if its signature lets it be read: the
/// signature covers the folder (or one it is in), and the file, links
/// followed, is inside that folder.
fn resolve(key: &str, target: &str) -> Result<PathBuf, &'static str> {
    const NOT_FOUND: &str = "404 Not Found";
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let path = path.strip_prefix('/').ok_or(NOT_FOUND)?;
    let (signature, rest) = path.split_once('/').ok_or(NOT_FOUND)?;
    let decoded = decode(rest).ok_or(NOT_FOUND)?;
    let mut file = PathBuf::from("/");
    for part in Path::new(&decoded).components() {
        match part {
            Component::Normal(name) => file.push(name),
            Component::RootDir | Component::CurDir => {}
            _ => return Err(NOT_FOUND),
        }
    }
    // A folder's own address reads its index.html.
    let folder = file
        .ancestors()
        .find(|folder| sign(key, folder) == signature)
        .ok_or("403 Forbidden")?;
    let mut file = std::fs::canonicalize(&file).map_err(|_| NOT_FOUND)?;
    let folder = std::fs::canonicalize(folder).map_err(|_| NOT_FOUND)?;
    if !file.starts_with(&folder) {
        return Err("403 Forbidden");
    }
    if file.is_dir() {
        file.push("index.html");
    }
    Ok(file)
}

fn sign(key: &str, folder: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(key.as_bytes());
    hash.update([0]);
    hash.update(folder.as_os_str().as_encoded_bytes());
    format!("{:x}", hash.finalize())[..32].to_string()
}

/// A path as a URL's, each byte other than letters, digits, `/` and
/// `-._~` percent-encoded.
fn encode(path: &Path) -> String {
    let mut out = String::new();
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub(crate) fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "txt" | "log" | "csv" | "tsv" | "md" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Deletes all but the newest `keep` pages of outputs.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut pages: Vec<_> = entries
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if pages.len() <= keep {
        return;
    }
    pages.sort();
    for (_, old) in &pages[..pages.len() - keep] {
        let _ = std::fs::remove_dir_all(old);
    }
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(port: u16, path: &str) -> (String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        (head.lines().next().unwrap().to_string(), body.to_string())
    }

    #[tokio::test]
    async fn a_page_reads_its_folder_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let report = root.join("qc report");
        std::fs::create_dir_all(report.join("lib")).unwrap();
        std::fs::write(
            report.join("multiqc.html"),
            "<script src=\"lib/a.js\"></script>",
        )
        .unwrap();
        std::fs::write(report.join("lib/a.js"), "let a = 1;").unwrap();
        std::fs::write(root.join("secret.txt"), "no").unwrap();
        let state = root.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let pages = Pages::open(&state).unwrap();

        let url = pages.serve(&report.join("multiqc.html")).await.unwrap();
        assert!(url.path.ends_with("/qc%20report/multiqc.html"), "{url:?}");
        let (status, body) = get(url.port, &url.path).await;
        assert_eq!(status, "HTTP/1.1 200 OK");
        assert!(body.contains("lib/a.js"));
        let script = url.path.replace("multiqc.html", "lib/a.js?v=2");
        assert_eq!(get(url.port, &script).await.1, "let a = 1;");

        // Outside the folder, and with a signature of the wrong folder.
        let signature = url.path.split('/').nth(1).unwrap();
        let outside = format!("/{signature}{}", encode(&root.join("secret.txt")));
        assert_eq!(get(url.port, &outside).await.0, "HTTP/1.1 403 Forbidden");
        let climbing = url.path.replace("multiqc.html", "../secret.txt");
        assert_ne!(get(url.port, &climbing).await.1, "no");
        let forged = url.path.replacen(signature, &"0".repeat(32), 1);
        assert_eq!(get(url.port, &forged).await.0, "HTTP/1.1 403 Forbidden");

        // A key kept in the state folder signs the same way after a restart.
        let again = Pages::open(&state).unwrap();
        let url2 = again.serve(&report.join("multiqc.html")).await.unwrap();
        assert_eq!(url2.path, url.path);
    }

    #[tokio::test]
    async fn output_html_gets_a_page_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let pages = Pages::open(tmp.path()).unwrap();
        let url = pages.serve_html("<b>DT</b>").await.unwrap();
        assert!(url.path.ends_with("/index.html"), "{url:?}");
        assert_eq!(get(url.port, &url.path).await.1, "<b>DT</b>");
        // A folder's address is its index.html.
        let folder = url.path.trim_end_matches("index.html");
        assert_eq!(get(url.port, folder).await.1, "<b>DT</b>");
    }

    #[test]
    fn paths_survive_encoding() {
        let path = Path::new("/data/Huan's run #3/ä.html");
        assert_eq!(
            decode(&encode(path)).as_deref(),
            Some("/data/Huan's run #3/ä.html")
        );
    }
}
