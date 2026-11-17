//! Connector children belong to this daemon, unlike independent kernels and jobs.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

use super::catalog::Program;

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

pub(super) struct Child {
    process: tokio::process::Child,
    input: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: Pending,
    reader: tokio::task::JoinHandle<()>,
    stderr: tokio::task::JoinHandle<()>,
}

impl Child {
    pub async fn start(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self> {
        // As with jobs, execute through the user's login shell so uvx and
        // environment modules are found. Quote every argument, never interpret it.
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
        let line = env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .chain(std::iter::once(command.to_owned()))
            .chain(args.iter().cloned())
            .map(|word| crate::agents::shell_quote(&word))
            .collect::<Vec<_>>()
            .join(" ");
        let mut cmd = tokio::process::Command::new(shell);
        cmd.args(["-l", "-i", "-c", &format!("exec /usr/bin/env {line}")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        if let Some(path) = crate::paths::program_path() {
            cmd.env("PATH", path);
        }
        // Explicit connector overrides win over the daemon's fallback PATH.
        cmd.envs(env);
        let mut process = cmd.spawn().context("could not start connector program")?;
        let input = tokio::sync::Mutex::new(process.stdin.take().unwrap());
        let output = process.stdout.take().unwrap();
        let mut errors = process.stderr.take().unwrap();
        let pending: Pending = Arc::default();
        let replies = pending.clone();
        let reader = tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                match output.read_until(b'\n', &mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.last() != Some(&b'\n') => break,
                    Ok(_) => {}
                }
                if let Ok(value) = serde_json::from_slice::<Value>(&line)
                    && let Some(id) = value["id"].as_u64()
                    && let Some(reply) = replies.lock().unwrap().remove(&id)
                {
                    let _ = reply.send(Ok(value));
                }
            }
            for (_, reply) in replies.lock().unwrap().drain() {
                let _ = reply.send(Err(anyhow::anyhow!("connector program closed its output")));
            }
        });
        let stderr = tokio::spawn(async move {
            // Keep only the last 8 KiB in memory. Never put untrusted stderr in
            // daemon logs or errors: a server could print a supplied secret.
            let mut log = VecDeque::with_capacity(8192);
            let mut bytes = [0; 2048];
            while let Ok(n) = errors.read(&mut bytes).await {
                if n == 0 {
                    break;
                }
                for byte in &bytes[..n] {
                    if log.len() == 8192 {
                        log.pop_front();
                    }
                    log.push_back(*byte);
                }
            }
        });
        Ok(Self {
            process,
            input,
            pending,
            reader,
            stderr,
        })
    }
    pub fn stop(&self) {
        if let Some(pid) = self.process.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        self.reader.abort();
        self.stderr.abort();
        for (_, reply) in self.pending.lock().unwrap().drain() {
            let _ = reply.send(Err(anyhow::anyhow!("connector program stopped")));
        }
    }

    pub async fn send(&self, message: &Value) -> Result<Value> {
        let id = message["id"].as_u64();
        let receiver = id.map(|id| {
            let (send, recv) = oneshot::channel();
            self.pending.lock().unwrap().insert(id, send);
            recv
        });
        // Remove an unanswered id on timeout/cancellation too.
        struct PendingCall<'a>(&'a Pending, Option<u64>);
        impl Drop for PendingCall<'_> {
            fn drop(&mut self) {
                if let Some(id) = self.1 {
                    self.0.lock().unwrap().remove(&id);
                }
            }
        }
        let _pending = PendingCall(&self.pending, id);
        let mut bytes = serde_json::to_vec(message)?;
        bytes.push(b'\n');
        {
            let mut input = self.input.lock().await;
            input
                .write_all(&bytes)
                .await
                .context("writing to connector program")?;
            input.flush().await?;
        }
        match receiver {
            Some(receiver) => receiver.await.context("connector program stopped")?,
            None => Ok(Value::Null),
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Some(pid) = self.process.id() {
            // The child has its own group; stop any subprocesses as well.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.process.start_kill();
        self.reader.abort();
        self.stderr.abort();
    }
}

pub(super) fn available(command: &str) -> bool {
    let path = crate::paths::program_path().unwrap_or_default();
    // The same login shell used for connector children, not a second PATH convention.
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
    std::process::Command::new(shell)
        .args([
            "-l",
            "-i",
            "-c",
            &format!(
                "command -v {} >/dev/null 2>&1",
                crate::agents::shell_quote(command)
            ),
        ])
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(super) fn download(root: &Path, program: &Program) -> Result<(PathBuf, bool)> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let platform = format!("{os}-{arch}");
    let checksum = program
        .sha256
        .get(&platform)
        .with_context(|| format!("BioMCP does not support this platform ({platform})"))?;
    let bin = root.join("connectors/bin");
    std::fs::create_dir_all(&bin)?;
    let bin = std::fs::canonicalize(bin)?;
    let folder = bin.join(format!("biomcp-{}", program.version));
    let target = folder.join(&program.binary);
    if target.is_file() {
        return Ok((target, false));
    }
    let temporary = bin.join(format!(".download-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&temporary)?;
    let result = (|| -> Result<()> {
        // Test daemons can replace the URL, but never the pinned checksum.
        let url = std::env::var("SANDHILL_BIOMCP_URL")
            .unwrap_or_else(|_| program.url.replace("{platform}", &platform));
        let archive = temporary.join("archive.tar.gz");
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&archive)?;
        let mut reader: Box<dyn Read> = if let Some(file) = url.strip_prefix("file://") {
            Box::new(std::fs::File::open(file)?)
        } else {
            let response = ureq::Agent::config_builder()
                .timeout_global(Some(super::client::timeout()))
                .build()
                .new_agent()
                .get(&url)
                .call()
                .map_err(|_| anyhow::anyhow!("could not download BioMCP"))?;
            Box::new(response.into_body().into_reader())
        };
        let mut digest = Sha256::new();
        let mut size = 0usize;
        let mut bytes = [0; 64 * 1024];
        loop {
            let n = reader.read(&mut bytes)?;
            if n == 0 {
                break;
            }
            size += n;
            if size > 100 << 20 {
                bail!("BioMCP download exceeds 100 MB");
            }
            digest.update(&bytes[..n]);
            output.write_all(&bytes[..n])?;
        }
        if sandhill_proto::hex(&digest.finalize()) != *checksum {
            bail!("the download did not match its checksum");
        }
        drop(output);
        let mut tar =
            tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(archive)?));
        let mut found = false;
        for entry in tar.entries()? {
            let mut entry = entry?;
            if entry.path()?.file_name() != Some(std::ffi::OsStr::new(&program.binary)) {
                continue;
            }
            if found || !entry.header().entry_type().is_file() || entry.size() > 100 << 20 {
                bail!("BioMCP archive contains an invalid binary");
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o755)
                .open(temporary.join(&program.binary))?;
            std::io::copy(&mut entry, &mut output)?;
            output.set_permissions(std::fs::Permissions::from_mode(0o755))?;
            found = true;
        }
        if !found {
            bail!("BioMCP archive contains no biomcp binary");
        }
        std::fs::create_dir_all(&folder)?;
        std::fs::rename(temporary.join(&program.binary), &target)?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&temporary);
    result?;
    Ok((target, true))
}
