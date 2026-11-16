//! Device-local skills, shared by every agent (Huan, 2026-11-16).
mod fetch;
mod scan;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sandhill_proto::{
    Call, SkillExport, SkillFile, SkillInfo, SkillList, SkillPreview, SuggestedSkill,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    #[serde(flatten)]
    info: SkillInfo,
    digest: String,
}
pub(super) type Index = BTreeMap<String, Record>;

#[derive(Serialize, Deserialize)]
struct Incoming {
    created_ms: i64,
    candidates: Vec<scan::Candidate>,
}

pub(super) fn catalog() -> Result<Vec<SuggestedSkill>> {
    let mut catalog: Value =
        serde_json::from_str(include_str!("../../assets/skills/catalog.json"))?;
    for item in catalog["skills"]
        .as_array_mut()
        .context("invalid skill catalog")?
    {
        item["installed"] = json!(false);
    }
    Ok(serde_json::from_value(catalog["skills"].take())?)
}

fn index(root: &Path) -> Result<Index> {
    match fs::read(root.join("skills/index.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("cannot read skills/index.json"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Index::new()),
        Err(error) => Err(error.into()),
    }
}

fn save(root: &Path, index: &Index) -> Result<()> {
    let folder = root.join("skills");
    fs::create_dir_all(&folder)?;
    let temporary = folder.join(format!(".index-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        fs::write(&temporary, serde_json::to_vec_pretty(index)?)?;
        fs::rename(&temporary, folder.join("index.json"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn enabled(root: &Path) -> Result<Vec<SkillInfo>> {
    Ok(index(root)?
        .into_values()
        .filter(|r| r.info.enabled)
        .map(|r| r.info)
        .collect())
}

fn list(root: &Path) -> Result<SkillList> {
    let installed: Vec<_> = index(root)?.into_values().map(|r| r.info).collect();
    let mut suggested = catalog()?;
    for item in &mut suggested {
        item.installed = installed.iter().any(|skill| skill.name == item.id || skill.name == item.name
            || matches!(&skill.source, sandhill_proto::SkillSource::Git { suggested: Some(id), .. } if id == &item.id));
    }
    Ok(SkillList {
        installed,
        suggested,
        built_in: crate::bio::guides::skills().collect(),
    })
}

fn incoming(root: &Path, token: &str) -> Result<PathBuf> {
    let id = uuid::Uuid::parse_str(token)
        .context("unknown or expired skill preview; preview the skill again")?;
    if id.to_string() != token {
        bail!("unknown or expired skill preview; preview the skill again");
    }
    let folder = root.join("skills/.incoming").join(token);
    let created = fs::metadata(folder.join("preview.json"))
        .and_then(|meta| meta.modified())
        .context("unknown or expired skill preview; preview the skill again")?;
    if SystemTime::now()
        .duration_since(created)
        .unwrap_or_default()
        > Duration::from_secs(3600)
    {
        fs::remove_dir_all(&folder)?;
        bail!("unknown or expired skill preview; preview the skill again");
    }
    Ok(folder)
}

async fn preview(root: &Path, request: sandhill_proto::SkillFetch) -> Result<SkillPreview> {
    let parent = root.join("skills/.incoming");
    fs::create_dir_all(&parent)?;
    for entry in fs::read_dir(&parent)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && SystemTime::now()
                .duration_since(entry.metadata()?.modified()?)
                .unwrap_or_default()
                > Duration::from_secs(3600)
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    let token = uuid::Uuid::new_v4().to_string();
    let folder = parent.join(&token);
    let tree = folder.join("tree");
    fs::create_dir_all(&tree)?;
    let result = async {
        let index = index(root)?;
        let fetched = fetch::fetch(&tree, request, &index).await?;
        let (candidates, refused) = scan::scan(
            &tree,
            &fetched.restriction,
            &fetched.source,
            &index,
            fetched.preserve_source,
        )?;
        let skills = candidates
            .iter()
            .map(|candidate| candidate.card.clone())
            .collect();
        fs::write(
            folder.join("preview.json"),
            serde_json::to_vec(&Incoming {
                created_ms: crate::journal::now_ms(),
                candidates,
            })?,
        )?;
        Ok(SkillPreview {
            token,
            source: fetched.label,
            skills,
            refused,
        })
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_dir_all(folder);
    }
    result
}

fn install(root: &Path, token: &str, names: &[String]) -> Result<Vec<SkillInfo>> {
    let folder = incoming(root, token)?;
    let result = (|| {
        let manifest: Incoming = serde_json::from_slice(&fs::read(folder.join("preview.json"))?)?;
        let mut unique = BTreeSet::new();
        for name in names {
            if !unique.insert(name) {
                bail!("skill selected more than once: {name}");
            }
            if !manifest.candidates.iter().any(|c| &c.card.name == name) {
                bail!("no skill named {name} in this preview");
            }
        }
        let mut index = index(root)?;
        let mut installed = Vec::new();
        for name in names {
            let candidate = manifest
                .candidates
                .iter()
                .find(|c| &c.card.name == name)
                .unwrap();
            let card = &candidate.card;
            let source = folder.join("tree").join(&card.path);
            let staged = folder.join(format!("install-{name}"));
            fs::create_dir(&staged)?;
            // Copy rather than move: selected candidates can contain other selected skills.
            for file in &card.files {
                let original = if file == "LICENSE" {
                    candidate
                        .licence_file
                        .as_ref()
                        .map(|p| folder.join("tree").join(p))
                        .unwrap_or_else(|| source.join(file))
                } else {
                    source.join(file)
                };
                let target = staged.join(scan::relative(file)?);
                fs::create_dir_all(target.parent().unwrap())?;
                if !fs::symlink_metadata(&original)?.is_file() {
                    bail!("skill files changed after preview; preview again");
                }
                fs::copy(original, target)?;
            }
            let (files, _) = scan::files(&staged, scan::MAX_BYTES, scan::MAX_FILES)?;
            if scan::digest(&staged, &files, None)? != candidate.digest {
                bail!("skill files changed after preview; preview again");
            }
            let destination = root.join("skills").join(name);
            let old = folder.join(format!("old-{name}"));
            let replaces = destination.exists();
            if replaces {
                fs::rename(&destination, &old)?;
            }
            if let Err(error) = fs::rename(&staged, &destination) {
                if replaces {
                    fs::rename(&old, &destination)?;
                }
                return Err(error.into());
            }
            let info = SkillInfo {
                name: name.clone(),
                description: card.description.clone(),
                source: candidate.source.clone(),
                licence: card.licence.clone(),
                enabled: index.get(name).is_none_or(|record| record.info.enabled),
                installed_ms: crate::journal::now_ms(),
                files: card.files.len() as u32,
                bytes: card.bytes,
                scripts: card.scripts.clone(),
                path: destination.to_string_lossy().into_owned(),
            };
            index.insert(
                name.clone(),
                Record {
                    info: info.clone(),
                    digest: candidate.digest.clone(),
                },
            );
            if let Err(error) = save(root, &index) {
                fs::remove_dir_all(&destination)?;
                if replaces {
                    fs::rename(&old, &destination)?;
                }
                return Err(error);
            }
            if replaces {
                fs::remove_dir_all(old)?;
            }
            installed.push(info);
        }
        Ok(installed)
    })();
    let cleanup = fs::remove_dir_all(folder);
    match result {
        Ok(value) => {
            cleanup?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

fn export(root: &Path, name: &str) -> Result<SkillExport> {
    let index = index(root)?;
    let info = &index
        .get(name)
        .context("no installed skill with that name")?
        .info;
    let folder = root.join("skills").join(&info.name);
    let (paths, _) = scan::files(&folder, scan::MAX_BYTES, scan::MAX_FILES)?;
    let files = paths
        .into_iter()
        .map(|path| {
            Ok(SkillFile {
                base64: BASE64.encode(fs::read(folder.join(&path))?),
                executable: scan::executable(&folder.join(&path)),
                path,
            })
        })
        .collect::<Result<_>>()?;
    Ok(SkillExport {
        name: info.name.clone(),
        source: info.source.clone(),
        files,
    })
}

fn get(root: &Path, name: Option<&str>, file: Option<&str>) -> Result<Value> {
    let Some(name) = name else {
        if file.is_some() {
            bail!("give a skill name with file");
        }
        return Ok(
            json!({"skills": enabled(root)?.iter().map(|s| json!({"name": s.name, "description": s.description})).collect::<Vec<_>>()}),
        );
    };
    let index = index(root)?;
    let info = &index
        .get(name)
        .filter(|r| r.info.enabled)
        .with_context(|| format!("no installed skill named {name} (get_skill lists them)"))?
        .info;
    let folder = root.join("skills").join(&info.name);
    let (files, _) = scan::files(&folder, scan::MAX_BYTES, scan::MAX_FILES)?;
    let file = file.unwrap_or("SKILL.md");
    let target = folder.join(scan::relative(file)?);
    if !target.canonicalize()?.starts_with(folder.canonicalize()?) {
        bail!("file escapes the skill folder");
    }
    let input = fs::File::open(target)?;
    if input.metadata()?.len() > 256 * 1024 {
        bail!("skill text files must be at most 256 KiB");
    }
    let mut text = String::new();
    input
        .take(256 * 1024 + 1)
        .read_to_string(&mut text)
        .context("skill file must be UTF-8 text")?;
    if text.len() > 256 * 1024 {
        bail!("skill text files must be at most 256 KiB");
    }
    Ok(
        json!({"name": info.name, "description": info.description, "path": folder, "files": files, "text": text}),
    )
}

/// Called under the daemon's skills gate (reads too, so none sees a
/// replacement half done), except previews, which only read the index.
pub(crate) async fn call(root: &Path, call: Call) -> Result<Value> {
    Ok(match call {
        Call::ListSkills => serde_json::to_value(list(root)?)?,
        Call::PreviewSkills { fetch } => serde_json::to_value(preview(root, fetch).await?)?,
        Call::InstallSkills { token, names } => {
            serde_json::to_value(install(root, &token, &names)?)?
        }
        Call::SetSkillEnabled { name, enabled } => {
            let mut index = index(root)?;
            index
                .get_mut(&name)
                .context("no installed skill with that name")?
                .info
                .enabled = enabled;
            save(root, &index)?;
            Value::Null
        }
        Call::RemoveSkill { name } => {
            let mut index = index(root)?;
            let record = index
                .remove(&name)
                .context("no installed skill with that name")?;
            let folder = root.join("skills").join(&record.info.name);
            let removed = root
                .join("skills")
                .join(format!(".removed-{}", uuid::Uuid::new_v4()));
            fs::rename(&folder, &removed)?;
            if let Err(error) = save(root, &index) {
                fs::rename(&removed, &folder)?;
                return Err(error);
            }
            fs::remove_dir_all(removed)?;
            Value::Null
        }
        Call::ExportSkill { name } => serde_json::to_value(export(root, &name)?)?,
        Call::GetSkill { name, file } => get(root, name.as_deref(), file.as_deref())?,
        _ => unreachable!("not a skills call"),
    })
}
