//! "Download to this Mac" for a server's file, as the Tauri app has it: the
//! file comes through the daemon in parts into the Downloads folder, as a
//! browser would save it (a `.download` file renamed when done), and is
//! then shown in Finder.

use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use gpui_kit::*;
use sandhill_proto::{Call, FileBytes, MAX_READ_BYTES};

use crate::panes::files::base_name;
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

/// Publishes a completed download without an overwrite window.
fn publish(partial: &Path, target: &Path) -> Result<PathBuf, String> {
    let dir = target.parent().ok_or("missing download folder")?;
    let name = target
        .file_name()
        .ok_or("missing download name")?
        .to_string_lossy();
    let mut destination = target.to_path_buf();
    loop {
        match std::fs::hard_link(partial, &destination) {
            Ok(()) => break,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                destination = free_name(dir, &name);
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    std::fs::remove_file(partial).map_err(|err| err.to_string())?;
    Ok(destination)
}

impl Workbench {
    /// Copies a file of the server into ~/Downloads, then shows it.
    pub(crate) fn download(&mut self, path: String, cx: &mut Context<Self>) {
        self.download_file(path, false, cx);
    }

    pub(crate) fn download_and_open(&mut self, path: String, cx: &mut Context<Self>) {
        self.download_file(path, true, cx);
    }

    fn download_file(&mut self, path: String, open: bool, cx: &mut Context<Self>) {
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
        let partial_name = format!(
            "{}.download",
            target.file_name().unwrap_or_default().to_string_lossy()
        );
        let partial = free_name(target.parent().unwrap(), &partial_name);
        cx.spawn(async move |this, cx| {
            let mut owns_partial = false;
            let result = async {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&partial)
                    .map_err(|err| err.to_string())?;
                owns_partial = true;
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
                publish(&partial, &target)
            }
            .await;
            if result.is_err() && owns_partial {
                let _ = std::fs::remove_file(&partial);
            }
            this.update(cx, |this, cx| {
                match result {
                    Ok(target) => {
                        let folder = target.parent().map(Path::to_path_buf).unwrap_or_default();
                        this.notice = Some(format!("Downloaded {name} to {}", folder.display()));
                        if open {
                            cx.open_with_system(&target);
                        } else {
                            cx.reveal_path(&target);
                        }
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
    use super::{free_name, publish};

    #[test]
    fn downloads_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a.bam"));
        std::fs::write(dir.path().join("a.bam"), "").unwrap();
        std::fs::write(dir.path().join("a 2.bam"), "").unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a 3.bam"));
        assert_eq!(free_name(dir.path(), ".env"), dir.path().join(".env"));
    }

    #[test]
    fn finishing_a_download_preserves_a_newly_created_destination() {
        let dir = tempfile::tempdir().unwrap();
        let target = free_name(dir.path(), "report.txt");
        let partial = dir.path().join("report.txt.download");
        std::fs::write(&partial, "downloaded").unwrap();
        std::fs::write(&target, "created while downloading").unwrap();
        let published = publish(&partial, &target).unwrap();
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "created while downloading"
        );
        assert_eq!(std::fs::read_to_string(&published).unwrap(), "downloaded");
        assert_ne!(published, target);
        assert!(!partial.exists());
    }
}
