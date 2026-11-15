//! Old-wire endpoint backed by real daemon file operations and real files.
//! Its closed request enum deliberately cannot deserialize newer RPCs.
mod common;

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use sandhill_proto::client::Client;
use sandhill_proto::{Call, FileBytes, MAX_READ_BYTES, Outcome, ServerMessage};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

const LIMIT: u64 = 5_000_000;

#[derive(Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
enum LegacyCall {
    ReadBytes {
        path: String,
        offset: u64,
        limit: Option<u64>,
    },
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    Grow,
    Shrink,
    Mtime,
    Offset,
    Path,
    Truncate,
    InvalidBase64,
    ExtraData,
    GrowAtEof,
}

type Reads = Arc<Mutex<Vec<(u64, u64)>>>;

fn legacy_endpoint(fault: Fault) -> (DuplexStream, Reads) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let reads = Arc::new(Mutex::new(Vec::new()));
    let recorded = reads.clone();
    tokio::spawn(async move {
        let (reader, mut writer) = tokio::io::split(server);
        let mut lines = BufReader::new(reader).lines();
        let mut index = 0;
        while let Some(line) = lines.next_line().await.unwrap() {
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            let id = request["id"].as_u64().unwrap();
            let outcome = match serde_json::from_value::<LegacyCall>(request) {
                Ok(LegacyCall::ReadBytes {
                    path,
                    offset,
                    limit,
                }) => {
                    let limit = limit.unwrap();
                    recorded.lock().unwrap().push((offset, limit));
                    // Use the actual historical implementation: its size is
                    // captured before the read and limit is capped at 4 MiB.
                    let mut reply =
                        sandhill_daemon::fileops::read_bytes(&path, offset, Some(limit)).unwrap();
                    if index == 1 {
                        match fault {
                            Fault::Grow => {
                                std::fs::OpenOptions::new()
                                    .append(true)
                                    .open(&path)
                                    .unwrap()
                                    .write_all(b"!")
                                    .unwrap();
                            }
                            Fault::Shrink => {
                                std::fs::OpenOptions::new()
                                    .write(true)
                                    .open(&path)
                                    .unwrap()
                                    .set_len(1)
                                    .unwrap();
                            }
                            Fault::Mtime => {
                                std::fs::File::open(&path)
                                    .unwrap()
                                    .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
                                    .unwrap();
                            }
                            Fault::Offset => reply.offset += 1,
                            Fault::Path => reply.path.push_str(".different"),
                            Fault::Truncate => reply.data.truncate(4),
                            Fault::InvalidBase64 => reply.data.replace_range(..1, "!"),
                            Fault::ExtraData => reply.data.push_str("eA=="),
                            _ => {}
                        }
                    }
                    if matches!(fault, Fault::GrowAtEof) && limit == 1 {
                        // Reproduce growth after this request captured its
                        // metadata, before it read the byte at the old EOF.
                        std::fs::OpenOptions::new()
                            .append(true)
                            .open(&path)
                            .unwrap()
                            .write_all(b"!")
                            .unwrap();
                        let grown =
                            sandhill_daemon::fileops::read_bytes(&path, offset, Some(1)).unwrap();
                        reply.data = grown.data;
                    }
                    index += 1;
                    Outcome::Result(serde_json::to_value(reply).unwrap())
                }
                Err(err) => Outcome::Error(err.to_string()),
            };
            let mut response =
                serde_json::to_vec(&ServerMessage::Response { id, outcome }).unwrap();
            response.push(b'\n');
            if writer.write_all(&response).await.is_err() {
                break;
            }
        }
    });
    (client, reads)
}

fn client(fault: Fault) -> (Client, Reads) {
    let (stream, reads) = legacy_endpoint(fault);
    let (reader, writer) = tokio::io::split(stream);
    (Client::new(reader, writer), reads)
}

async fn read_file(
    path: &Path,
    fault: Fault,
) -> (
    sandhill_proto::client::Result<sandhill_proto::FileText>,
    Reads,
) {
    let (client, reads) = client(fault);
    let result = client
        .read_text_bounded(path.to_str().unwrap(), LIMIT)
        .await;
    (result, reads)
}

#[tokio::test]
async fn legacy_wire_opens_small_and_empty_csv_tsv_losslessly() {
    let dir = common::tempdir().unwrap();
    for (name, raw) in [
        ("quoted.csv", "\u{feff}a,b\r\n\"雪,ice\",2\r\n"),
        ("quoted.tsv", "a\tb\r\n\"two\nlines\"\t三\r\n"),
        ("empty.csv", ""),
        ("empty.tsv", ""),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, raw).unwrap();
        let (client, reads) = client(Fault::None);
        let metadata: FileBytes = client
            .call(Call::ReadBytes {
                path: path.to_str().unwrap().into(),
                offset: 0,
                limit: Some(0),
            })
            .await
            .unwrap();
        assert_eq!(metadata.size, raw.len() as u64);
        assert!(metadata.data.is_empty());
        let read = client
            .read_text_bounded(path.to_str().unwrap(), LIMIT)
            .await
            .unwrap();
        assert_eq!(read.text.as_bytes(), raw.as_bytes());
        assert_eq!(read.path, path.to_str().unwrap());
        assert_eq!(read.mtime_ms, metadata.mtime_ms);
        assert!(
            reads
                .lock()
                .unwrap()
                .iter()
                .all(|(_, count)| *count <= MAX_READ_BYTES)
        );
    }
}

#[tokio::test]
async fn legacy_wire_exact_limit_and_split_utf8_are_lossless_but_plus_one_is_refused() {
    let dir = common::tempdir().unwrap();
    let path = dir.path().join("boundary.csv");
    let mut raw = "x".repeat(MAX_READ_BYTES as usize - 1);
    raw.push('雪'); // First byte in one response, remaining bytes in the next.
    raw.extend(std::iter::repeat_n('y', LIMIT as usize - raw.len()));
    std::fs::write(&path, &raw).unwrap();
    let (read, reads) = read_file(&path, Fault::None).await;
    assert_eq!(read.unwrap().text, raw);
    assert!(
        reads
            .lock()
            .unwrap()
            .iter()
            .all(|(_, count)| *count <= MAX_READ_BYTES)
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"!")
        .unwrap();
    let (read, reads) = read_file(&path, Fault::None).await;
    assert!(read.unwrap_err().to_string().contains("too large"));
    assert!(reads.lock().unwrap().iter().all(|(_, count)| *count == 0));
}

#[tokio::test]
async fn legacy_wire_rejects_changes_and_malformed_chunks() {
    let dir = common::tempdir().unwrap();
    for (i, fault) in [
        Fault::Grow,
        Fault::Shrink,
        Fault::Mtime,
        Fault::Offset,
        Fault::Path,
        Fault::Truncate,
        Fault::InvalidBase64,
        Fault::ExtraData,
        Fault::GrowAtEof,
    ]
    .into_iter()
    .enumerate()
    {
        let path = dir.path().join(format!("changed-{i}.csv"));
        std::fs::write(&path, vec![b'x'; MAX_READ_BYTES as usize + 64]).unwrap();
        let (read, _) = read_file(&path, fault).await;
        assert!(
            read.is_err(),
            "accepted changed or malformed file (case {i})"
        );
    }
    let path = dir.path().join("invalid-utf8.tsv");
    std::fs::write(&path, [0xff]).unwrap();
    let (read, _) = read_file(&path, Fault::None).await;
    assert!(read.unwrap_err().to_string().contains("not a UTF-8"));
}
