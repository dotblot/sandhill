//! The file tree's "Reveal in Finder" for this Mac's files, and "Download to
//! this Mac" for a server's: the file comes through the daemon in parts, into
//! the Downloads folder, as a browser would save it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use sandhill_proto::{Call, FileBytes, MAX_READ_BYTES};
use tauri::State;

use crate::link::Links;

/// Shows a file of this Mac in Finder, selected.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    if !Path::new(&path).is_absolute() {
        return Err(format!("not a full path: {path}"));
    }
    reveal(Path::new(&path))
}

fn reveal(path: &Path) -> Result<(), String> {
    let mut command = if cfg!(target_os = "macos") {
        let mut open = std::process::Command::new("open");
        open.arg("-R").arg(path);
        open
    } else {
        let mut open = std::process::Command::new("xdg-open");
        open.arg(path.parent().unwrap_or(path));
        open
    };
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot show {}: {err}", path.display()))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Copies a file from `host` into ~/Downloads, shows it in Finder and
/// returns where it went.
#[tauri::command]
pub async fn download_file(
    links: State<'_, Arc<Links>>,
    host: String,
    path: String,
) -> Result<String, String> {
    let client = links.client(&host).ok_or("not connected to sandhilld")?;
    let name = Path::new(&path)
        .file_name()
        .ok_or_else(|| format!("not a file: {path}"))?
        .to_string_lossy()
        .into_owned();
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let downloads = PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&downloads).map_err(|err| err.to_string())?;
    let target = free_name(&downloads, &name);
    let partial = target.with_file_name(format!(
        "{}.download",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = async {
        let mut file = std::fs::File::create(&partial).map_err(|err| err.to_string())?;
        let mut offset = 0;
        loop {
            let part: FileBytes = client
                .call(Call::ReadBytes {
                    path: path.clone(),
                    offset,
                    limit: Some(MAX_READ_BYTES),
                })
                .await
                .map_err(|err| err.to_string())?;
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
    if let Err(err) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(format!("cannot download {path}: {err}"));
    }
    let _ = reveal(&target);
    Ok(target.to_string_lossy().into_owned())
}

/// `name`, or "name 2.ext", "name 3.ext"… if the folder has one.
fn free_name(dir: &Path, name: &str) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_never_replace_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a.bam"));
        std::fs::write(dir.path().join("a.bam"), "").unwrap();
        std::fs::write(dir.path().join("a 2.bam"), "").unwrap();
        assert_eq!(free_name(dir.path(), "a.bam"), dir.path().join("a 3.bam"));
    }
}
