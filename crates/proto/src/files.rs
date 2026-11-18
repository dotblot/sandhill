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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileVersionReason {
    Opened,
    Saved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileVersionInfo {
    pub id: i64,
    pub time_ms: i64,
    pub reason: FileVersionReason,
    /// The change from the preceding distinct version.
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FileHistoryLimits {
    pub days: u32,
    pub per_file: u32,
    pub file_mb: u32,
    pub total_mb: u32,
}

impl Default for FileHistoryLimits {
    fn default() -> Self {
        Self {
            days: 30,
            per_file: 100,
            file_mb: 2,
            total_mb: 500,
        }
    }
}

impl From<FileHistoryLimits> for Call {
    fn from(limits: FileHistoryLimits) -> Self {
        let FileHistoryLimits {
            days,
            per_file,
            file_mb,
            total_mb,
        } = limits;
        Self::SetFileHistoryLimits {
            days,
            per_file,
            file_mb,
            total_mb,
        }
    }
}

/// An output-free Jupyter cell. The snapshot preserves the source lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookHistoryCell {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub cell_type: String,
    #[serde(deserialize_with = "notebook_source")]
    pub source: Vec<String>,
}

#[derive(Deserialize)]
pub struct NotebookHistory {
    pub cells: Vec<NotebookHistoryCell>,
}

impl NotebookHistory {
    pub fn render(mut self) -> serde_json::Result<String> {
        let mut seen = std::collections::HashSet::with_capacity(self.cells.len());
        for (i, cell) in self.cells.iter_mut().enumerate() {
            let id = cell
                .id
                .take()
                .filter(|id| !id.is_empty() && !seen.contains(id))
                .unwrap_or_else(|| notebook_cell_id(i, &seen));
            seen.insert(id.clone());
            cell.id = Some(id);
        }
        serde_json::to_string_pretty(&self.cells)
    }

    /// Reads the stored cell array, which has no notebook-level fields.
    pub fn from_version(version: &str) -> serde_json::Result<Self> {
        Ok(Self {
            cells: serde_json::from_str(version)?,
        })
    }

    /// Text shown, compared and copied in File History.
    pub fn readable_text(&self) -> String {
        notebook_readable_text(self.cells.iter().map(|cell| {
            (
                cell.cell_type.as_str(),
                cell.source.iter().map(String::as_str),
            )
        }))
    }
}

pub fn notebook_readable_text<'a>(
    cells: impl IntoIterator<Item = (&'a str, impl IntoIterator<Item = &'a str>)>,
) -> String {
    use std::fmt::Write as _;

    let mut text = String::new();
    for (index, (kind, source)) in cells.into_iter().enumerate() {
        let _ = writeln!(text, "Cell {} · {kind}", index + 1);
        for line in source {
            text.push_str(line);
        }
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text
}

pub fn notebook_cell_id(index: usize, seen: &std::collections::HashSet<String>) -> String {
    let mut n = index + 1;
    loop {
        let id = format!("cell-{n}");
        if !seen.contains(&id) {
            return id;
        }
        n += 1000;
    }
}

fn notebook_source<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Source {
        Text(String),
        Lines(Vec<String>),
    }
    match Source::deserialize(d)? {
        Source::Text(text) => Ok(text.split_inclusive('\n').map(str::to_owned).collect()),
        Source::Lines(lines)
            if lines.iter().enumerate().all(|(i, line)| {
                let (body, end) = line
                    .strip_suffix('\n')
                    .map_or((line.as_str(), false), |s| (s, true));
                !body.contains('\n') && (end || i + 1 == lines.len()) && !line.is_empty()
            }) =>
        {
            Ok(lines)
        }
        Source::Lines(lines) => Ok(lines
            .concat()
            .split_inclusive('\n')
            .map(str::to_owned)
            .collect()),
    }
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
    /// `name.gz` in bgzip's blocks, which tabix and samtools index; files
    /// only.
    Bgzip,
}

/// How far a compression or extraction is: bytes read of the input, of
/// `total` (the files packed, or the archive unpacked).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArchiveProgress {
    pub done: u64,
    pub total: u64,
}

/// Prefix of the error a cancelled compression or extraction returns.
pub const CANCELLED_ERROR: &str = "cancelled:";

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
