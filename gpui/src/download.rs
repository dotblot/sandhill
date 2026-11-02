//! "Download to this Mac" for a server's file, as the Tauri app has it: the
//! file comes through the daemon in parts into the Downloads folder, as a
//! browser would save it (a `.download` file renamed when done), and is
//! then shown in Finder.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use gpui_kit::*;
use sandhill_proto::{Call, FileBytes, MAX_READ_BYTES};

use crate::files::base_name;
use crate::workbench::Workbench;

/// `name`, or "name 2.ext", "name 3.ext"… if the folder has one.
pub(crate) fn free_name(dir: &Path, name: &str) -> PathBuf {
    let (stem, ext) = match name.rfind('.').filter(|&dot| dot > 0) {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    };
    (1..)
        .map(|n| match n {
            1 => dir.join(name),
            n => dir.join(format!("{stem} {n}{ext}")),
        })
        .find(|path| std::fs::symlink_metadata(path).is_err())
        .unwrap()
}

fn downloads() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let dir = PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

impl Workbench {
    /// Copies a file of the server into ~/Downloads, then shows it.
    pub(crate) fn download(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let name = base_name(&path).to_string();
        let target = match downloads() {
            Ok(dir) => free_name(&dir, &name),
            Err(err) => {
                self.problem = Some(format!("Could not download {name}: {err}"));
                cx.notify();
                return;
            }
        };
        self.notice = Some(format!("Downloading {name}…"));
        cx.notify();
        let partial = target.with_file_name(format!(
            "{}.download",
            target.file_name().unwrap_or_default().to_string_lossy()
        ));
        cx.spawn(async move |this, cx| {
            let result = async {
                let mut file = std::fs::File::create(&partial).map_err(|err| err.to_string())?;
                let mut offset = 0;
                loop {
                    let part: FileBytes = daemon
                        .call(Call::ReadBytes {
                            path: path.clone(),
                            offset,
                            limit: Some(MAX_READ_BYTES),
                        })
                        .await
                        .map_err(|err| format!("{err:#}"))?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(part.data.trim())
                        .map_err(|err| err.to_string())?;
                    file.write_all(&bytes).map_err(|err| err.to_string())?;
                    offset += bytes.len() as u64;
                    if bytes.is_empty() || offset >= part.size {
                        break;
                    }
                }
                file.sync_all().map_err(|err| err.to_string())?;
                std::fs::rename(&partial, &target).map_err(|err| err.to_string())
            }
            .await;
            if result.is_err() {
                let _ = std::fs::remove_file(&partial);
            }
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        let folder = target.parent().map(Path::to_path_buf).unwrap_or_default();
                        this.notice = Some(format!("Downloaded {name} to {}", folder.display()));
                        cx.reveal_path(&target);
                    }
                    Err(err) => {
                        this.notice = None;
                        this.problem = Some(format!("Could not download {name}: {err}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::free_name;

    #[test]
    fn downloads_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a.bam"));
        std::fs::write(dir.path().join("a.bam"), "").unwrap();
        std::fs::write(dir.path().join("a 2.bam"), "").unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a 3.bam"));
        assert_eq!(free_name(dir.path(), ".env"), dir.path().join(".env"));
    }
}
