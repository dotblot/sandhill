use anyhow::{Result, bail};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Project-local tools win, followed by the selected kernel's environment,
/// login PATH, then checksum-owned copies installed by Sandhill.
pub fn find_binary(project: &Path, interpreter: Option<&Path>, name: &str) -> Option<PathBuf> {
    let mut candidates = vec![
        project.join(".venv/bin").join(name),
        project.join("venv/bin").join(name),
    ];
    if let Some(bin) = interpreter.and_then(Path::parent) {
        candidates.push(bin.join(name));
    }
    if let Some(path) = crate::paths::program_path() {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join(name)));
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(
            PathBuf::from(home)
                .join(".local/share/sandhill/bin")
                .join(name),
        );
    }
    candidates.into_iter().find(|path| {
        std::fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

pub(super) fn command(project: &Path, interpreter: Option<&Path>, name: &str) -> Result<Command> {
    let Some(binary) = find_binary(project, interpreter, name) else {
        bail!(
            "lsp_binary_missing: {name} is not installed in the project environment, PATH, or Sandhill's bin directory"
        );
    };
    let mut command = Command::new(binary);
    command.arg("server").current_dir(project);
    if let Some(path) = crate::paths::program_path() {
        command.env("PATH", path);
    }
    // ty server does not accept --python; its editor configuration supplies
    // environment.python instead. No interpreter or environment is modified.
    Ok(command)
}

pub(super) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}
