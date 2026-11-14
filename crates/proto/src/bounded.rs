//! Bounded text reads using the original, chunked `read_bytes` wire API.
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::client::{Client, ClientError, Result};
use crate::{Call, FileBytes, FileText, MAX_READ_BYTES};

fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::InvalidFile(message.into())
}

fn unchanged(before: &FileBytes, after: &FileBytes, offset: u64) -> Result<()> {
    if after.path != before.path || after.offset != offset {
        return Err(invalid("file read returned a different path or offset"));
    }
    if after.size != before.size || after.mtime_ms != before.mtime_ms {
        return Err(invalid("file changed while it was being read; try again"));
    }
    Ok(())
}

impl Client {
    /// Read exact UTF-8 text without requiring a new daemon RPC. Metadata and
    /// every chunk must agree; a final EOF probe detects growth even when an
    /// older daemon reports metadata captured before reading that chunk.
    /// The text buffer never exceeds `max_bytes`; exactly that size is allowed.
    pub async fn read_text_bounded(&self, path: &str, max_bytes: u64) -> Result<FileText> {
        let before: FileBytes = self
            .call(Call::ReadBytes {
                path: path.into(),
                offset: 0,
                limit: Some(0),
            })
            .await?;
        if before.offset != 0 || !before.data.is_empty() {
            return Err(invalid(
                "file metadata response contained unexpected bytes or offset",
            ));
        }
        if before.size > max_bytes {
            return Err(invalid(format!(
                "{} is too large to open as text ({} bytes; limit {max_bytes} bytes)",
                before.path, before.size
            )));
        }
        let size = usize::try_from(before.size)
            .map_err(|_| invalid("file is too large for this machine"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|err| invalid(format!("cannot allocate text buffer: {err}")))?;
        bytes.resize(size, 0);
        let mut offset = 0;
        while offset < before.size {
            let count = (before.size - offset).min(MAX_READ_BYTES);
            let chunk: FileBytes = self
                .call(Call::ReadBytes {
                    path: before.path.clone(),
                    offset,
                    limit: Some(count),
                })
                .await?;
            unchanged(&before, &chunk, offset)?;
            // Reject oversized encodings before decoding into the bounded
            // destination. Interpret UTF-8 only after assembling bytes: a
            // codepoint may cross the daemon's chunk boundary.
            if chunk.data.len() as u64 != count.div_ceil(3) * 4 {
                return Err(invalid(
                    "file read was truncated or exceeded its requested length",
                ));
            }
            let end = offset + count;
            let decoded = STANDARD
                .decode_slice(&chunk.data, &mut bytes[offset as usize..end as usize])
                .map_err(|err| invalid(format!("invalid file bytes: {err}")))?;
            if decoded as u64 != count {
                return Err(invalid(
                    "file read was truncated or exceeded its requested length",
                ));
            }
            offset = end;
        }
        let eof: FileBytes = self
            .call(Call::ReadBytes {
                path: before.path.clone(),
                offset,
                limit: Some(1),
            })
            .await?;
        unchanged(&before, &eof, offset)?;
        if !eof.data.is_empty() {
            return Err(invalid("file grew while it was being read; try again"));
        }
        let after: FileBytes = self
            .call(Call::ReadBytes {
                path: before.path.clone(),
                offset: 0,
                limit: Some(0),
            })
            .await?;
        unchanged(&before, &after, 0)?;
        if !after.data.is_empty() {
            return Err(invalid("file metadata response contained unexpected bytes"));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| invalid(format!("{} is not a UTF-8 text file", before.path)))?;
        Ok(FileText {
            path: before.path,
            text,
            mtime_ms: before.mtime_ms,
        })
    }
}
