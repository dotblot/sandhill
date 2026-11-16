use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};
use sandhill_proto::{SkillCandidate, SkillRefusal, SkillSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Index;

pub(super) const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub(super) const MAX_FILES: usize = 2048;

#[derive(Serialize, Deserialize)]
pub(super) struct Candidate {
    pub card: SkillCandidate,
    pub source: SkillSource,
    pub digest: String,
    pub licence_file: Option<PathBuf>,
}

pub(super) fn relative(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || !path.components().all(|c| matches!(c, Component::Normal(_)))
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("file paths must be relative, without . or ..: {value}");
    }
    Ok(path)
}

pub(super) fn name(value: &str) -> Result<String> {
    let safe: String = value
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    let safe = safe.trim_matches(['-', '.']);
    if safe.is_empty() || safe.starts_with('.') {
        bail!("skill name is empty or starts with a dot");
    }
    Ok(safe.to_owned())
}

pub(super) fn files(root: &Path, max_bytes: u64, max_files: usize) -> Result<(Vec<String>, u64)> {
    fn visit(
        root: &Path,
        dir: &Path,
        paths: &mut Vec<String>,
        bytes: &mut u64,
        limits: (u64, usize),
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                bail!("symlinks are not allowed: {}", path.display());
            }
            if meta.is_dir() {
                visit(root, &path, paths, bytes, limits)?;
            } else if meta.is_file() {
                *bytes = bytes
                    .checked_add(meta.len())
                    .ok_or_else(|| anyhow::anyhow!("files are too large"))?;
                if *bytes > limits.0 {
                    bail!("folder exceeds {} MB", limits.0 / (1024 * 1024));
                }
                paths.push(
                    path.strip_prefix(root)?
                        .to_str()
                        .ok_or_else(|| anyhow::anyhow!("file paths must be UTF-8"))?
                        .to_owned(),
                );
                if paths.len() > limits.1 {
                    bail!("folder exceeds {} files", limits.1);
                }
            } else {
                bail!("only ordinary files and folders are allowed");
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    let mut bytes = 0;
    visit(root, root, &mut paths, &mut bytes, (max_bytes, max_files))?;
    paths.sort();
    Ok((paths, bytes))
}

pub(super) fn digest(root: &Path, paths: &[String], licence: Option<&Path>) -> Result<String> {
    use std::io::Read;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    for path in paths {
        let original = if path == "LICENSE" {
            licence
                .map(Path::to_owned)
                .unwrap_or_else(|| root.join(path))
        } else {
            root.join(path)
        };
        let mut input = fs::File::open(&original)?;
        hash.update((path.len() as u64).to_le_bytes());
        hash.update(path.as_bytes());
        hash.update(input.metadata()?.len().to_le_bytes());
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        hash.update([u8::from(executable(&original))]);
    }
    Ok(sandhill_proto::hex(&hash.finalize()))
}

pub(super) fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn licence_name(path: &str) -> bool {
    let name = path.to_ascii_uppercase();
    ["LICENSE", "LICENCE", "COPYING"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn licence(root: &Path, folder: &Path, paths: &[String]) -> Result<Option<PathBuf>> {
    if let Some(path) = paths.iter().find(|p| !p.contains('/') && licence_name(p)) {
        return Ok(Some(folder.join(path)));
    }
    let mut parent = folder.parent();
    while let Some(dir) = parent.filter(|dir| dir.starts_with(root)) {
        let mut entries = fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if licence_name(&entry.file_name().to_string_lossy()) && entry.file_type()?.is_file() {
                return Ok(Some(entry.path()));
            }
        }
        parent = dir.parent();
    }
    Ok(None)
}

fn classify(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(16 * 1024)
        .read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
    Ok(
        if text.contains("apache license") && text.contains("version 2.0") {
            "Apache-2.0"
        } else if text.contains("mit license")
            || text.contains("permission is hereby granted, free of charge")
        {
            "MIT"
        } else if text.contains("bsd")
            || text.contains("redistribution and use in source and binary forms")
        {
            "BSD"
        } else if text.contains("gnu general public license") && text.contains("version 3") {
            "GPL-3.0"
        } else if text.contains("gnu general public license") && text.contains("version 2") {
            "GPL-2.0"
        } else {
            return Ok(path.file_name().unwrap().to_string_lossy().into_owned());
        }
        .to_owned(),
    )
}

fn candidate(
    root: &Path,
    folder: &Path,
    source: &SkillSource,
    index: &Index,
    preserve_source: bool,
) -> Result<Candidate> {
    let mut component = folder;
    while component != root {
        if fs::symlink_metadata(component)?.file_type().is_symlink() {
            bail!("symlinks are not allowed");
        }
        component = component
            .parent()
            .ok_or_else(|| anyhow::anyhow!("skill folder escapes the fetched tree"))?;
    }
    let (mut paths, mut bytes) = files(folder, MAX_BYTES, MAX_FILES)?;
    let text = fs::read_to_string(folder.join("SKILL.md"))?;
    let (metadata, _) = crate::agent_commands::folders::metadata(&text);
    let raw_name = metadata
        .name
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md needs a frontmatter name"))?;
    let name = name(&raw_name)?;
    let description = metadata
        .description
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("SKILL.md needs a frontmatter description"))?;
    let licence = licence(root, folder, &paths)?;
    let ancestor = licence
        .as_ref()
        .filter(|p| p.parent() != Some(folder))
        .cloned();
    if let Some(path) = ancestor.as_ref() {
        bytes += fs::metadata(path)?.len();
        paths.push("LICENSE".into());
        paths.sort();
        if bytes > MAX_BYTES || paths.len() > MAX_FILES {
            bail!("skill including its licence exceeds 20 MB or 2048 files");
        }
    }
    let scripts = paths
        .iter()
        .filter(|p| {
            matches!(
                Path::new(p).extension().and_then(|e| e.to_str()),
                Some("py" | "sh" | "bash" | "zsh" | "R" | "r" | "js" | "mjs" | "ts" | "pl" | "rb")
            ) || executable(&if p.as_str() == "LICENSE" {
                ancestor.clone().unwrap_or_else(|| folder.join(p))
            } else {
                folder.join(p)
            })
        })
        .cloned()
        .collect();
    let digest = digest(folder, &paths, ancestor.as_deref())?;
    let path = folder.strip_prefix(root)?.to_string_lossy().into_owned();
    let mut source = source.clone();
    if !preserve_source
        && let SkillSource::Git {
            path: source_path, ..
        } = &mut source
    {
        *source_path = path.clone();
    }
    let installed = index.get(&name);
    Ok(Candidate {
        card: SkillCandidate {
            name,
            description,
            path,
            files: paths,
            bytes,
            scripts,
            licence: licence.as_deref().map(classify).transpose()?,
            replaces: installed.is_some(),
            changed: installed.is_none_or(|record| record.digest != digest),
        },
        source,
        digest,
        licence_file: ancestor.map(|p| p.strip_prefix(root).unwrap().to_owned()),
    })
}

pub(super) fn scan(
    root: &Path,
    restriction: &str,
    source: &SkillSource,
    index: &Index,
    preserve_source: bool,
) -> Result<(Vec<Candidate>, Vec<SkillRefusal>)> {
    fn find(dir: &Path, depth: usize, folders: &mut Vec<PathBuf>) -> Result<()> {
        if fs::symlink_metadata(dir.join("SKILL.md")).is_ok() {
            folders.push(dir.to_owned());
        }
        if depth == 6 {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() != ".git" && entry.file_type()?.is_dir() {
                find(&entry.path(), depth + 1, folders)?;
            }
        }
        Ok(())
    }
    let start = if restriction.is_empty() {
        root.to_owned()
    } else {
        root.join(relative(restriction)?)
    };
    if !start.canonicalize()?.starts_with(root.canonicalize()?) {
        bail!("skill folder escapes the fetched repository");
    }
    let mut folders = Vec::new();
    find(&start, 0, &mut folders)?;
    folders.sort();
    let mut candidates = Vec::new();
    let mut refused = Vec::new();
    let mut names = BTreeSet::new();
    for folder in folders {
        let result =
            candidate(root, &folder, source, index, preserve_source).and_then(|candidate| {
                if !names.insert(candidate.card.name.clone()) {
                    bail!("another skill has the same name");
                }
                Ok(candidate)
            });
        match result {
            Ok(candidate) => candidates.push(candidate),
            Err(error) => refused.push(SkillRefusal {
                path: folder.strip_prefix(root)?.to_string_lossy().into_owned(),
                reason: error.to_string(),
            }),
        }
    }
    if candidates.is_empty() && refused.is_empty() {
        refused.push(SkillRefusal {
            path: restriction.into(),
            reason: "no SKILL.md found (searched up to six folders deep)".into(),
        });
    }
    Ok((candidates, refused))
}
