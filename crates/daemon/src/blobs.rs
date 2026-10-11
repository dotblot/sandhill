//! Content-addressed storage for large or binary output values (plots,
//! big HTML tables). The journal keeps only a reference, so events stay small
//! and identical plots are stored once.

use std::path::{Path, PathBuf};

use sandhill_proto::BLOB_KEY;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// String values longer than this are moved out of the journal regardless of
/// MIME type.
const INLINE_LIMIT: usize = 64 * 1024;

/// Output message types whose `content.data` is a MIME bundle.
const MIME_BUNDLE_TYPES: &[&str] = &["display_data", "update_display_data", "execute_result"];

#[derive(Debug, Clone)]
pub struct BlobStore {
    dir: PathBuf,
}

impl BlobStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        BlobStore { dir: dir.into() }
    }

    /// Moves binary and oversized values in a MIME bundle into the store,
    /// replacing each with `{"sandhill_blob": hash, "size": bytes}`.
    pub fn externalize(&self, msg_type: &str, content: &mut Value) -> std::io::Result<()> {
        if !MIME_BUNDLE_TYPES.contains(&msg_type) {
            return Ok(());
        }
        let Some(data) = content.get_mut("data").and_then(Value::as_object_mut) else {
            return Ok(());
        };
        for (mime, value) in data.iter_mut() {
            let Some(text) = value.as_str() else { continue };
            if is_binary_mime(mime) || text.len() > INLINE_LIMIT {
                let hash = self.put(text.as_bytes())?;
                *value = json!({ BLOB_KEY: hash, "size": text.len() });
            }
        }
        Ok(())
    }

    pub fn put(&self, bytes: &[u8]) -> std::io::Result<String> {
        let hash = format!("{:x}", Sha256::digest(bytes));
        let path = self.path_for(&hash).expect("sha256 hex is a valid hash");
        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap())?;
            // Write to a temp name first so a crash never leaves a truncated blob.
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(hash)
    }

    pub fn get(&self, hash: &str) -> std::io::Result<Vec<u8>> {
        let path = self.path_for(hash).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a blob hash")
        })?;
        std::fs::read(path)
    }

    /// `None` unless `hash` is 64 lowercase hex characters, which also keeps
    /// client-supplied hashes from escaping the blob directory.
    fn path_for(&self, hash: &str) -> Option<PathBuf> {
        let valid = hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        valid.then(|| self.dir.join(&hash[..2]).join(&hash[2..]))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

fn is_binary_mime(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml" || mime == "application/pdf"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_moved_out_and_text_stays() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let mut content = json!({
            "data": {"image/png": "iVBORw0KGgo=", "text/plain": "<Figure>"},
            "metadata": {}
        });
        store.externalize("display_data", &mut content).unwrap();
        let png = &content["data"]["image/png"];
        let hash = png[BLOB_KEY].as_str().unwrap();
        assert_eq!(png["size"], 12);
        assert_eq!(content["data"]["text/plain"], "<Figure>");
        assert_eq!(store.get(hash).unwrap(), b"iVBORw0KGgo=");
    }

    #[test]
    fn stream_messages_are_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        let mut content = json!({"name": "stdout", "text": "x".repeat(INLINE_LIMIT + 1)});
        let before = content.clone();
        store.externalize("stream", &mut content).unwrap();
        assert_eq!(content, before);
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path());
        assert!(store.get("../../etc/passwd").is_err());
        assert!(store.get(&"A".repeat(64)).is_err());
    }
}
