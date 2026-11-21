use super::*;
use sandhill_proto::{
    ARK_PATH, BIN_PATH, DAEMON_PATH, INSTALL_PATH, OLD_DAEMON_PATH, RUFF_PATH, TY_PATH,
};

use crate::paths::{installed_path, legacy_path};

pub(super) fn component_paths() -> [(&'static str, &'static str); 6] {
    [
        ("sandhilld", DAEMON_PATH),
        ("ark", ARK_PATH),
        ("ty", TY_PATH),
        ("ruff", RUFF_PATH),
        ("systemd", ".config/systemd/user/sandhilld.service"),
        (
            "launchd",
            "Library/LaunchAgents/dev.sandhill.sandhilld.plist",
        ),
    ]
}
pub(super) fn legacy_components() -> impl Iterator<Item = (&'static str, String)> {
    std::iter::once(("sandhilld", OLD_DAEMON_PATH.into())).chain(
        ["ark", "ty", "ruff"]
            .into_iter()
            .map(|name| (name, format!("{}/{name}", legacy_path(BIN_PATH)))),
    )
}

/// Find the record for this exact path, whether its folder has moved or not.
pub(super) fn component_manifest(home: &Path, id: &str, relative: &str) -> Option<PathBuf> {
    let name = format!("{id}.json");
    [INSTALL_PATH, legacy_path(INSTALL_PATH)]
        .into_iter()
        .map(|dir| home.join(dir).join(&name))
        .find(|path| {
            read_safe(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| value["path"] == relative)
        })
}

pub(super) const AGY_CONFIG: &str = ".gemini/config/mcp_config.json";
pub(super) fn antigravity_manifest(home: &Path) -> PathBuf {
    installed_path(home, INSTALL_PATH, "antigravity-mcp.json")
}

fn antigravity_server_name(home: &Path) -> Result<String> {
    let manifest = antigravity_manifest(home);
    if !path_exists(&manifest)? {
        return Ok("sandhill".into());
    }
    let value: Value = serde_json::from_slice(&read_safe(&manifest)?)?;
    match value
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("sandhill")
    {
        name @ ("sandhill" | "sandhill-managed") => Ok(name.into()),
        _ => bail!("unknown Sandhill MCP registration name; preserved"),
    }
}

pub(super) fn antigravity_registration_at(
    home: &Path,
    executable: &Path,
) -> Result<(String, bool)> {
    let path = home.join(AGY_CONFIG);
    let config: Value = if path_exists(&path)? {
        serde_json::from_slice(&read_safe(&path)?)?
    } else {
        json!({})
    };
    let owned_name = antigravity_server_name(home)?;
    let servers = &config["mcpServers"];
    if let Some(entry) = servers.get(&owned_name)
        && antigravity_owned(home, entry).is_ok()
    {
        let ready = entry["command"] == executable.to_string_lossy().as_ref()
            && entry["args"] == json!(["mcp"])
            && entry.get("disabled").and_then(Value::as_bool) != Some(true);
        return Ok((owned_name, ready));
    }
    for name in ["sandhill", "sandhill-managed"] {
        if servers.get(name).is_none() {
            return Ok((name.into(), false));
        }
    }
    bail!("both Sandhill MCP registration names are unowned or modified; preserved")
}

pub(super) fn antigravity_entry(home: &Path) -> Result<Option<Value>> {
    let path = home.join(AGY_CONFIG);
    if !path_exists(&path)? {
        return Ok(None);
    }
    let config: Value = serde_json::from_slice(&read_safe(&path)?)?;
    let server = antigravity_server_name(home)?;
    Ok(config
        .get("mcpServers")
        .and_then(|s| s.get(&server))
        .cloned())
}

pub(super) fn antigravity_owned(home: &Path, entry: &Value) -> Result<()> {
    let manifest: Value = serde_json::from_slice(
        &read_safe(&antigravity_manifest(home))
            .context("Antigravity MCP entry has no ownership manifest; preserved")?,
    )?;
    if manifest["version"] != 1
        || manifest["component"] != "antigravity-mcp"
        || manifest["path"] != AGY_CONFIG
        || manifest["entry"] != *entry
    {
        bail!("Antigravity MCP registration is unowned or modified; preserved");
    }
    Ok(())
}

pub(super) fn antigravity_tools_allowed_at(home: &Path) -> Result<bool> {
    let manifest = antigravity_manifest(home);
    if !path_exists(&manifest)? {
        return Ok(false);
    }
    let manifest: Value = serde_json::from_slice(&read_safe(&manifest)?)?;
    Ok(manifest["allowed"] == true
        && antigravity_entry(home)?.is_some_and(|entry| {
            entry["disabled"] != true && antigravity_owned(home, &entry).is_ok()
        }))
}

pub(super) fn remove_antigravity_mcp_at(home: &Path) -> Result<()> {
    let path = home.join(AGY_CONFIG);
    let manifest = antigravity_manifest(home);
    if !path_exists(&manifest)? {
        bail!("Antigravity's tool entry was not added by Sandhill.");
    }
    if antigravity_entry(home)?.is_none() {
        return remove_safe(&manifest);
    }
    let server = antigravity_server_name(home)?;
    let before = read_safe(&path)?;
    let mut config: Value = serde_json::from_slice(&before)?;
    let servers = config
        .get_mut("mcpServers")
        .and_then(Value::as_object_mut)
        .context("no MCP server map")?;
    let entry = servers
        .get(&server)
        .context("Sandhill MCP entry is absent")?;
    antigravity_owned(home, entry)?;
    servers.remove(&server);
    let parent = SafeDir::open(path.parent().context("no MCP config parent")?)?;
    parent.replace_file(
        path.file_name().context("no config filename")?,
        &before,
        &serde_json::to_vec_pretty(&config)?,
    )?;
    remove_safe(&manifest)?;
    Ok(())
}

pub(super) fn owned_component(
    home: &Path,
    id: &str,
    relative: &str,
    manifest: &Path,
) -> Result<String> {
    let bytes = read_safe(manifest).context("No readable ownership record.")?;
    let record: Value = serde_json::from_slice(&bytes)?;
    if record["version"] != 1 || record["component"] != id || record["path"] != relative {
        bail!("The ownership record doesn't match this path.");
    }
    let path = home.join(relative);
    let parent = SafeDir::open(path.parent().context("No installed file parent.")?)?;
    let mut file = parent.open_file(path.file_name().context("No installed file name.")?)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let bytes = file.read(&mut buffer)?;
        if bytes == 0 {
            break;
        }
        hasher.update(&buffer[..bytes]);
    }
    let digest = sandhill_proto::hex(&hasher.finalize());
    if record["sha256"] != digest {
        bail!("The installed file has changed.");
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_requires_manifest_and_matching_contents() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let relative = DAEMON_PATH;
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"owned").unwrap();
        let manifest = home.join("manifest.json");
        assert!(owned_component(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&manifest, serde_json::to_vec(&json!({"version":1,"component":"sandhilld","path":relative,"sha256":sandhill_proto::hex(&Sha256::digest(b"owned"))})).unwrap()).unwrap();
        std::fs::write(&path, b"modified").unwrap();
        assert!(owned_component(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&path, b"owned").unwrap();
        assert_eq!(
            owned_component(&home, "sandhilld", relative, &manifest).unwrap(),
            sandhill_proto::hex(&Sha256::digest(b"owned")),
        );
    }
    #[test]
    fn mcp_removal_preserves_unrelated_servers_and_auth_settings() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let config = home.join(AGY_CONFIG);
        SafeDir::ensure(config.parent().unwrap()).unwrap();
        let entry = json!({"command":"/owned/sandhilld","args":["mcp"]});
        let other = json!({"command":"/user/server","args":["--own"]});
        let document =
            json!({"mcpServers":{"sandhill":entry,"user":other},"auth":{"keep":"unchanged"}});
        std::fs::write(&config, serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(remove_antigravity_mcp_at(&home).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&config).unwrap()).unwrap(),
            document
        );
        let manifest = antigravity_manifest(&home);
        SafeDir::ensure(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            &manifest,
            serde_json::to_vec(
                &json!({"version":1,"component":"antigravity-mcp","path":AGY_CONFIG,"entry":entry}),
            )
            .unwrap(),
        )
        .unwrap();
        remove_antigravity_mcp_at(&home).unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert!(after["mcpServers"].get("sandhill").is_none());
        assert_eq!(after["mcpServers"]["user"], other);
        assert_eq!(after["auth"], document["auth"]);
        assert!(!manifest.exists());
    }

    #[test]
    fn tracked_mcp_replacement_preserves_legacy_entry_on_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let config = home.join(AGY_CONFIG);
        SafeDir::ensure(config.parent().unwrap()).unwrap();
        let legacy =
            json!({"command":"/expired/Sandhill.app/Contents/MacOS/sandhilld","args":["mcp"]});
        let current = json!({"command":"/current/sandhilld","args":["mcp"]});
        std::fs::write(
            &config,
            json!({"mcpServers":{"sandhill":legacy}}).to_string(),
        )
        .unwrap();
        assert_eq!(
            antigravity_registration_at(&home, Path::new("/current/sandhilld")).unwrap(),
            ("sandhill-managed".into(), false)
        );
        std::fs::write(
            &config,
            json!({"mcpServers":{"sandhill":legacy,"sandhill-managed":current}}).to_string(),
        )
        .unwrap();
        let manifest = antigravity_manifest(&home);
        SafeDir::ensure(manifest.parent().unwrap()).unwrap();
        std::fs::write(
            &manifest,
            json!({"version":1,"component":"antigravity-mcp",
            "path":AGY_CONFIG,"server":"sandhill-managed","entry":current})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            antigravity_registration_at(&home, Path::new("/upgraded/sandhilld")).unwrap(),
            ("sandhill-managed".into(), false)
        );
        remove_antigravity_mcp_at(&home).unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(after, json!({"mcpServers":{"sandhill":legacy}}));
        assert!(!manifest.exists());
    }
}
