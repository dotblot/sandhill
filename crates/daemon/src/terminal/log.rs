//! A terminal's output, kept on disk so it can be replayed to a client that
//! connects later, even after the program ended. The output is numbered by
//! byte offset from the start and stored in 1 MiB segment files
//! (`out.0`, `out.1`, ...); only the last few are kept.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const SEGMENT: u64 = 1 << 20;
/// Segments kept: the newest output, up to 4 MiB of it.
pub const KEEP: u64 = 4;

fn segment_path(dir: &Path, n: u64) -> PathBuf {
    dir.join(format!("out.{n}"))
}

/// Appends a running program's output. Only the terminal host writes.
pub struct LogWriter {
    dir: PathBuf,
    end: u64,
    file: Option<File>,
}

impl LogWriter {
    /// Starts an empty log in `dir`.
    pub fn create(dir: &Path) -> io::Result<LogWriter> {
        Ok(LogWriter {
            dir: dir.to_path_buf(),
            end: 0,
            file: None,
        })
    }

    /// Offset just past the last byte written.
    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn append(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let at = self.end % SEGMENT;
            if at == 0 || self.file.is_none() {
                let n = self.end / SEGMENT;
                self.file = Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(segment_path(&self.dir, n))?,
                );
                if n >= KEEP {
                    let _ = std::fs::remove_file(segment_path(&self.dir, n - KEEP));
                }
            }
            let take = data.len().min((SEGMENT - at) as usize);
            self.file
                .as_mut()
                .expect("opened above")
                .write_all(&data[..take])?;
            self.end += take as u64;
            data = &data[take..];
        }
        Ok(())
    }
}

/// Where the kept output starts and ends, from the segment files.
pub fn bounds(dir: &Path) -> (u64, u64) {
    let mut segments: Vec<u64> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_prefix("out.")?.parse().ok())
        .collect();
    segments.sort_unstable();
    let (Some(&first), Some(&last)) = (segments.first(), segments.last()) else {
        return (0, 0);
    };
    let len = std::fs::metadata(segment_path(dir, last)).map_or(0, |m| m.len());
    (first * SEGMENT, last * SEGMENT + len)
}

/// Reads the output between `from` and `to`. Returns where the bytes
/// actually start: later than `from` when that part is no longer kept.
pub fn read(dir: &Path, from: u64, to: u64) -> io::Result<(u64, Vec<u8>)> {
    let mut start = from;
    let mut out = Vec::new();
    while start + (out.len() as u64) < to {
        let at = start + out.len() as u64;
        let n = at / SEGMENT;
        let mut file = match File::open(segment_path(dir, n)) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound && out.is_empty() => {
                // Dropped already: start at the next segment instead.
                start = (n + 1) * SEGMENT;
                if start >= to {
                    return Ok((to, Vec::new()));
                }
                continue;
            }
            Err(err) => return Err(err),
        };
        file.seek(SeekFrom::Start(at % SEGMENT))?;
        let want = (to - at).min(SEGMENT - at % SEGMENT);
        let before = out.len();
        file.take(want).read_to_end(&mut out)?;
        if out.len() == before {
            break;
        }
    }
    Ok((start, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_spans_segments_and_old_ones_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = LogWriter::create(dir.path()).unwrap();
        let chunk: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        for _ in 0..20 {
            log.append(&chunk).unwrap();
        }
        let end = 20 * 300_000;
        assert_eq!(log.end(), end);
        let (first, last) = bounds(dir.path());
        assert_eq!(last, end);
        assert_eq!(first, (end / SEGMENT - (KEEP - 1)) * SEGMENT);

        // Reading from the start begins where the kept output does.
        let (start, bytes) = read(dir.path(), 0, end).unwrap();
        assert_eq!(start, first);
        assert_eq!(bytes.len() as u64, end - first);
        assert_eq!(bytes[0], (first % 300_000 % 256) as u8);

        // A range across a segment boundary comes back whole.
        let from = last - SEGMENT / 2 - 10;
        let (start, bytes) = read(dir.path(), from, last).unwrap();
        assert_eq!(start, from);
        assert_eq!(bytes.len() as u64, last - from);
        assert_eq!(bytes[0], (from % 300_000 % 256) as u8);
    }

    #[test]
    fn an_empty_log_reads_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(bounds(dir.path()), (0, 0));
        assert_eq!(read(dir.path(), 0, 0).unwrap(), (0, vec![]));
    }
}
