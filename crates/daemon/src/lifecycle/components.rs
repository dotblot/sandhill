use super::*;

pub(super) fn component_paths() -> [(&'static str, &'static str); 6] {
    [
        ("sandhilld", ".local/bin/sandhilld"),
        ("ark", ".local/share/sandhill/bin/ark"),
        ("ty", sandhill_proto::TY_PATH),
        ("ruff", sandhill_proto::RUFF_PATH),
        ("systemd", ".config/systemd/user/sandhilld.service"),
        (
            "launchd",
            "Library/LaunchAgents/dev.sandhill.sandhilld.plist",
        ),
    ]
}

pub(super) const AGY_CONFIG: &str = ".gemini/config/mcp_config.json";
pub(super) const AGY_MANIFEST: &str = ".local/share/sandhill/install/antigravity-mcp.json";

fn antigravity_server_name(home: &Path) -> Result<String> {
    let manifest = home.join(AGY_MANIFEST);
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
    for name in ["sandhill", "sandhill-managed"] {
        let Some(entry) = config.get("mcpServers").and_then(|v| v.get(name)) else {
            return Ok((name.into(), false));
        };
        if entry["command"] == executable.to_string_lossy().as_ref()
            && entry["args"] == json!(["mcp"])
            && entry.get("disabled").and_then(Value::as_bool) != Some(true)
        {
            return Ok((name.into(), true));
        }
        if name == owned_name && antigravity_owned(home, entry).is_ok() {
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
        &read_safe(&home.join(AGY_MANIFEST))
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

pub(super) fn remove_antigravity_mcp_at(home: &Path) -> Result<()> {
    let path = home.join(AGY_CONFIG);
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
    remove_safe(&home.join(AGY_MANIFEST))?;
    Ok(())
}

pub(super) fn uninstall_one(home: &Path, id: &str, relative: &str, manifest: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let bytes = read_safe(manifest)
        .with_context(|| format!("{id}: no readable ownership manifest; preserved"))?;
    let record: Value = serde_json::from_slice(&bytes)?;
    if record["version"] != 1 || record["component"] != id || record["path"] != relative {
        bail!("{id}: invalid ownership manifest; preserved");
    }
    let path = home.join(relative);
    if path_exists(&path)? {
        let parent = SafeDir::open(path.parent().context("no installed file parent")?)?;
        let mut file = parent.open_file(path.file_name().context("no installed filename")?)?;
        let installed = file.metadata()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let digest = sandhill_proto::hex(&hasher.finalize());
        if record["sha256"] != digest {
            bail!("{id}: installed file was modified; preserved");
        }
        // Never stop this running service synchronously: StopDaemon follows the
        // delivered response. Disable its future launch before removing files.
        if id == "systemd" {
            let status = std::process::Command::new("systemctl")
                .args(["--user", "disable", "sandhilld.service"])
                .status()?;
            if !status.success() {
                bail!("could not disable owned systemd service");
            }
            if std::process::Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", "sandhilld.service"])
                .status()?
                .success()
            {
                bail!(
                    "owned systemd service is disabled but still loaded/running; stop it outside the daemon before removing its service file"
                );
            }
        } else if id == "launchd" {
            let target = format!("gui/{}/dev.sandhill.sandhilld", unsafe { libc::getuid() });
            let status = std::process::Command::new("launchctl")
                .args(["disable", &target])
                .status()?;
            if !status.success() {
                bail!("could not disable owned launchd service");
            }
            if std::process::Command::new("launchctl")
                .args(["print", &target])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?
                .success()
            {
                bail!(
                    "owned launchd service is disabled but still loaded; unload it outside the daemon before removing its service file"
                );
            }
        }
        let current = parent
            .open_file(path.file_name().context("no installed filename")?)?
            .metadata()?;
        if (
            installed.dev(),
            installed.ino(),
            installed.len(),
            installed.mtime(),
            installed.mtime_nsec(),
        ) != (
            current.dev(),
            current.ino(),
            current.len(),
            current.mtime(),
            current.mtime_nsec(),
        ) {
            bail!("{id}: installed file changed during removal; preserved");
        }
        parent.remove(path.file_name().context("no installed filename")?)?;
    }
    remove_safe(manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_requires_manifest_and_matching_contents() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let relative = ".local/bin/sandhilld";
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"owned").unwrap();
        let manifest = home.join("manifest.json");
        assert!(uninstall_one(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&manifest, serde_json::to_vec(&json!({"version":1,"component":"sandhilld","path":relative,"sha256":sandhill_proto::hex(&Sha256::digest(b"owned"))})).unwrap()).unwrap();
        std::fs::write(&path, b"modified").unwrap();
        assert!(uninstall_one(&home, "sandhilld", relative, &manifest).is_err());
        assert!(path.exists());
        std::fs::write(&path, b"owned").unwrap();
        uninstall_one(&home, "sandhilld", relative, &manifest).unwrap();
        assert!(!path.exists());
        assert!(!manifest.exists());
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
        let manifest = home.join(AGY_MANIFEST);
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
        let manifest = home.join(AGY_MANIFEST);
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
