//! Files and folders: reads, writes, listings and their error prefixes.

use super::*;

/// Prefix of the error returned when a write would overwrite changes made on
/// disk since the file was read.
pub const CONFLICT_ERROR: &str = "conflict:";

/// Prefix of the error returned when making, moving or copying something
/// would replace what is already there.
pub const EXISTS_ERROR: &str = "exists:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileText {
    pub path: String,
    pub text: String,
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileWritten {
    pub path: String,
    pub mtime_ms: i64,
}

/// What [`Call::DeletePath`] did.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Deleted {
    pub path: String,
    /// Where the Trash keeps it, when it was moved there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trashed_to: Option<String>,
}

/// How [`Call::CompressPath`] packs a file or folder.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    /// `name.zip`, for a file or a folder.
    Zip,
    /// `name.gz` for a file, `name.tar.gz` for a folder.
    Gzip,
}

/// The most [`Call::ReadBytes`] returns at once.
pub const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

/// Part of a file, from [`Call::ReadBytes`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileBytes {
    pub path: String,
    pub offset: u64,
    /// The bytes, in base64.
    pub data: String,
    /// The whole file's size.
    pub size: u64,
    pub mtime_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Dir,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mtime_ms: i64,
    /// A directory holding `.git`; reported only when `ListDir` asks for marks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub git: bool,
    /// A directory holding `.sandhill`; reported only when asked for marks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandhill: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileStat {
    pub path: String,
    /// `None` when the file doesn't exist (or can't be read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime_ms: Option<i64>,
    #[serde(default)]
    pub size: u64,
}
