//! The daemon's own log, which the app shows from the status bar.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};
use sandhill_proto::LogTail;

/// The last `bytes` of the log at `path`, starting at a whole line.
pub fn tail(path: &Path, bytes: u64) -> Result<LogTail> {
    let mut file = File::open(path).with_context(|| format!("no log at {}", path.display()))?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.take(bytes).read_to_end(&mut buf)?;
    // Unless it starts at the top, drop the line cut in half.
    let from = match start {
        0 => 0,
        _ => buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |at| at + 1),
    };
    Ok(LogTail {
        path: path.display().to_string(),
        text: String::from_utf8_lossy(&buf[from..]).into_owned(),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_starts_at_a_whole_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sandhilld.log");
        std::fs::write(&path, "first line\nsecond line\nthird\n").unwrap();
        let all = tail(&path, 1024).unwrap();
        assert_eq!(all.text, "first line\nsecond line\nthird\n");
        assert_eq!(all.size, 29);
        let end = tail(&path, 15).unwrap();
        assert_eq!(end.text, "third\n");
        assert!(tail(&dir.path().join("none.log"), 10).is_err());
    }
}
