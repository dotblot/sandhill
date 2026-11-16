use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sandhill_proto::{SkillFetch, SkillSource};

use super::{Index, catalog, scan};

const MAX_FETCH_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub(super) struct Fetched {
    pub label: String,
    pub source: SkillSource,
    pub restriction: String,
    pub preserve_source: bool,
}

struct Repository {
    url: String,
    reference: String,
    path: String,
    suggested: Option<String>,
}

fn repository(input: &str, commit: Option<String>) -> Result<Repository> {
    let input = input.trim();
    if input.is_empty() || input.starts_with('-') {
        bail!("give a git repository address");
    }
    let url = if !input.contains(':') && !input.starts_with('/') && input.split('/').count() == 2 {
        format!("https://github.com/{input}")
    } else {
        input.to_owned()
    };
    let mut repo = Repository {
        url: url.clone(),
        reference: commit.clone().unwrap_or_else(|| "HEAD".into()),
        path: String::new(),
        suggested: None,
    };
    if let Some(path) = url.strip_prefix("https://github.com/") {
        let parts: Vec<_> = path.trim_end_matches('/').split('/').collect();
        if parts.len() < 2 || parts[..2].iter().any(|s| s.is_empty()) {
            bail!("GitHub addresses need an owner and repository");
        }
        repo.url = format!(
            "https://github.com/{}/{}",
            parts[0],
            parts[1].trim_end_matches(".git")
        );
        if parts.len() > 2 {
            if parts.len() < 4 || parts[2] != "tree" {
                bail!("use a GitHub repository or /tree/<ref>/<folder> address");
            }
            if commit.is_none() {
                repo.reference = parts[3].to_owned();
            }
            repo.path = parts[4..].join("/");
            if !repo.path.is_empty() {
                scan::relative(&repo.path)?;
            }
        }
    }
    if repo.reference.is_empty() || repo.reference.starts_with('-') {
        bail!("invalid git commit or ref");
    }
    Ok(repo)
}

async fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = tokio::process::Command::new(crate::git::program()?);
    command
        .args([
            "-c",
            "core.quotepath=off",
            "-c",
            "color.ui=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_EDITOR")
        .env_remove("GIT_SEQUENCE_EDITOR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // No controlling terminal: private repositories use the device's existing credentials.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let output = command.output().await.context("cannot run git")?;
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn tree_bytes(root: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path())?;
        bytes = bytes.saturating_add(if entry.file_type()?.is_dir() {
            tree_bytes(&entry.path())?
        } else {
            meta.len()
        });
        if bytes > MAX_FETCH_BYTES {
            bail!("repository exceeds 100 MB");
        }
    }
    Ok(bytes)
}

fn ignored(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "__MACOSX")
        || path.file_name().is_some_and(|name| name == ".DS_Store")
}

fn write_file(root: &Path, path: &str, bytes: &[u8], executable: bool) -> Result<()> {
    let target = root.join(scan::relative(path)?);
    fs::create_dir_all(target.parent().unwrap())?;
    fs::write(&target, bytes)?;
    fs::set_permissions(
        target,
        fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
    )?;
    Ok(())
}

fn zip(root: &Path, encoded: &str) -> Result<()> {
    let bytes = BASE64.decode(encoded).context("invalid zip base64")?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("cannot read zip")?;
    if archive.len() > MAX_ENTRIES {
        bail!("zip exceeds 10,000 entries");
    }
    let mut size = 0u64;
    let mut names = BTreeSet::new();
    // Validate every entry before writing anything, including ignored Mac metadata.
    for n in 0..archive.len() {
        let entry = archive.by_index(n)?;
        let name = entry.name()?;
        let path = name.trim_end_matches('/');
        scan::relative(path)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("zip symlinks are not allowed");
        }
        size = size.saturating_add(entry.size());
        if size > MAX_FETCH_BYTES {
            bail!("zip exceeds 100 MB uncompressed");
        }
        if !names.insert(path.to_owned()) {
            bail!("zip contains duplicate paths");
        }
    }
    for n in 0..archive.len() {
        let mut entry = archive.by_index(n)?;
        let name = entry.name()?.into_owned();
        if entry.is_dir() || ignored(Path::new(&name)) {
            continue;
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_FETCH_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != entry.size() {
            bail!("zip entry size does not match its metadata");
        }
        write_file(
            root,
            &name,
            &bytes,
            entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0),
        )?;
    }
    Ok(())
}

pub(super) async fn fetch(root: &Path, request: SkillFetch, index: &Index) -> Result<Fetched> {
    let repo = match request {
        SkillFetch::Git { url, commit } => repository(&url, commit)?,
        SkillFetch::Suggested { id } => {
            let item = catalog()?
                .into_iter()
                .find(|s| s.id == id)
                .context("unknown suggested skill")?;
            let mut repo = repository(&item.url, Some(item.commit))?;
            repo.path = item.path;
            repo.suggested = Some(id);
            repo
        }
        SkillFetch::Update { name } => {
            let record = index
                .get(&name)
                .context("no installed skill with that name")?;
            let SkillSource::Git {
                url,
                path,
                suggested,
                ..
            } = &record.info.source
            else {
                bail!("only skills from GitHub can be checked for updates");
            };
            let mut repo = repository(url, None)?;
            repo.path = path.clone();
            repo.suggested = suggested.clone();
            repo
        }
        SkillFetch::Zip { label, base64 } => {
            zip(root, &base64)?;
            return Ok(Fetched {
                source: SkillSource::Upload {
                    label: label.clone(),
                },
                label,
                restriction: String::new(),
                preserve_source: false,
            });
        }
        SkillFetch::Files {
            label,
            files,
            source,
        } => {
            if files.len() > MAX_ENTRIES {
                bail!("upload exceeds 10,000 files");
            }
            let mut size = 0u64;
            let mut names = BTreeSet::new();
            for file in files {
                let path = scan::relative(&file.path)?;
                if !names.insert(file.path.clone()) {
                    bail!("upload contains duplicate paths");
                }
                let bytes = BASE64.decode(&file.base64).context("invalid file base64")?;
                size = size.saturating_add(bytes.len() as u64);
                if size > MAX_FETCH_BYTES {
                    bail!("upload exceeds 100 MB");
                }
                if !ignored(path) {
                    write_file(root, &file.path, &bytes, file.executable)?;
                }
            }
            let preserve_source = source.is_some();
            return Ok(Fetched {
                source: source.unwrap_or_else(|| SkillSource::Upload {
                    label: label.clone(),
                }),
                label,
                restriction: String::new(),
                preserve_source,
            });
        }
    };
    let commit = tokio::time::timeout(Duration::from_secs(120), async {
        git(root, &["init", "--quiet"]).await?;
        git(root, &["remote", "add", "origin", &repo.url]).await?;
        git(
            root,
            &["fetch", "--depth", "1", "--", "origin", &repo.reference],
        )
        .await?;
        git(root, &["checkout", "--quiet", "--detach", "FETCH_HEAD"]).await?;
        git(root, &["rev-parse", "HEAD"]).await
    })
    .await
    .context("fetching the skill repository took more than 120 seconds")??;
    tree_bytes(root)?;
    let label = if let Some(github) = repo.url.strip_prefix("https://github.com/") {
        format!("github.com/{github} @ {}", &commit[..7.min(commit.len())])
    } else {
        repo.url.clone()
    };
    Ok(Fetched {
        label,
        restriction: repo.path,
        preserve_source: false,
        source: SkillSource::Git {
            url: repo.url,
            path: String::new(),
            commit,
            suggested: repo.suggested,
        },
    })
}
