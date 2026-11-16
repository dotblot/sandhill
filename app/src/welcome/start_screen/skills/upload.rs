use super::*;
use base64::Engine;
use sandhill_proto::SkillFile;
use std::io::Read;
use std::path::Path;

const MAX_FILES: usize = 2_048;
const MAX_BYTES: usize = 20 * 1024 * 1024;

impl SkillsView {
    pub(super) fn choose_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_card(Card::Busy("Choosing files…".into()), window, cx);
        self.generation += 1;
        let generation = self.generation;
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Choose a skill folder or zip".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let Some(paths) = picked.await?? else {
                    return Ok(None);
                };
                let Some(path) = paths.into_iter().next() else {
                    return Ok(None);
                };
                let fetch = cx
                    .background_spawn(async move { local_fetch(&path) })
                    .await?;
                Ok::<_, anyhow::Error>(Some(fetch))
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(Some(fetch)) => this.preview(fetch, window, cx),
                    Ok(None) => this.dismiss(cx),
                    Err(error) => this.fail(error, cx),
                }
            })
            .ok();
        })
        .detach();
    }
}

fn local_fetch(path: &Path) -> anyhow::Result<SkillFetch> {
    let metadata = std::fs::symlink_metadata(path)?;
    anyhow::ensure!(
        !metadata.file_type().is_symlink(),
        "Choose a folder or zip, not a symbolic link."
    );
    let label = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Choose a named skill folder or zip."))?
        .to_string_lossy()
        .into_owned();
    if metadata.is_dir() {
        let mut files = Vec::new();
        let mut bytes = 0;
        walk(path, path, &mut files, &mut bytes)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(SkillFetch::Files {
            label,
            files,
            source: None,
        })
    } else {
        anyhow::ensure!(
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("zip")),
            "Choose a skill folder or a .zip file."
        );
        let bytes = read_bounded(path, MAX_BYTES)?;
        Ok(SkillFetch::Zip {
            label,
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        })
    }
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SkillFile>,
    bytes: &mut usize,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            walk(root, &path, files, bytes)?;
        } else if metadata.is_file() {
            anyhow::ensure!(
                files.len() < MAX_FILES,
                "A skill upload can contain at most 2,048 files."
            );
            let data = read_bounded(&path, MAX_BYTES - *bytes)?;
            *bytes += data.len();
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let executable = false;
            let relative = path
                .strip_prefix(root)?
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Skill file names must be valid Unicode."))?
                .replace(std::path::MAIN_SEPARATOR, "/");
            files.push(SkillFile {
                path: relative,
                base64: base64::engine::general_purpose::STANDARD.encode(data),
                executable,
            });
        }
    }
    Ok(())
}

fn read_bounded(path: &Path, remaining: usize) -> anyhow::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    anyhow::ensure!(
        file.metadata()?.len() <= remaining as u64,
        "A skill upload can contain at most 20 MB."
    );
    let mut data = Vec::new();
    file.take(remaining as u64 + 1).read_to_end(&mut data)?;
    anyhow::ensure!(
        data.len() <= remaining,
        "A skill upload can contain at most 20 MB."
    );
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::{SkillFetch, local_fetch, read_bounded};
    use base64::Engine;

    #[test]
    fn folder_upload_preserves_paths_and_skips_metadata() {
        let root = std::env::temp_dir().join(format!(
            "sandhill-skill-upload-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("SKILL.md"), "instructions").unwrap();
        std::fs::write(root.join(".git/config"), "ignored").unwrap();
        std::fs::write(root.join(".DS_Store"), "ignored").unwrap();
        std::fs::write(root.join("scripts/run.py"), "print(1)").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};
            std::fs::set_permissions(
                root.join("scripts/run.py"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            symlink(root.join("SKILL.md"), root.join("link")).unwrap();
        }
        let SkillFetch::Files { files, source, .. } = local_fetch(&root).unwrap() else {
            panic!("not a folder");
        };
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["SKILL.md", "scripts/run.py"]
        );
        assert!(source.is_none());
        #[cfg(unix)]
        assert!(files[1].executable);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&files[0].base64)
                .unwrap(),
            b"instructions"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn upload_rejects_wrong_format_and_excess_bytes() {
        let path = std::env::temp_dir().join(format!(
            "sandhill-skill-upload-{}.txt",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, "12345").unwrap();
        assert!(local_fetch(&path).is_err());
        assert!(read_bounded(&path, 4).is_err());
        assert_eq!(read_bounded(&path, 5).unwrap(), b"12345");
        std::fs::remove_file(path).unwrap();
    }
}
