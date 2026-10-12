//! Which build of sandhilld is running.

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// A short hash of this executable, read the first time it is asked for
/// (the daemon asks as it starts, before an update can replace the file).
///
/// The app starts `sandhilld attach --start` from the build it ships with.
/// If the daemon already running is another build, such as one an older
/// version of the app left running, `attach` replaces it.
pub fn build_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| match hash_executable() {
        Ok(hash) => hash,
        Err(err) => {
            tracing::warn!("cannot read this executable to identify the build: {err}");
            String::new()
        }
    })
}

fn hash_executable() -> std::io::Result<String> {
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let hash = format!("{:x}", hasher.finalize());
    Ok(hash[..16].to_string())
}

/// Where this executable is, for running it again (terminal hosts, agents'
/// hooks and MCP server). After an update replaced the file, Linux names
/// the running one "<path> (deleted)"; the new build at `<path>` is the
/// one to run then.
pub fn executable() -> std::path::PathBuf {
    let path = std::env::current_exe().unwrap_or_else(|_| "sandhilld".into());
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(original) => original.into(),
        None => path,
    }
}
