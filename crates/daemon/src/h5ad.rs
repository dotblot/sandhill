//! AnnData is read in a short-lived copy of sandhilld. A panic, stack overflow
//! or allocation failure in the HDF5 parser must not take down the daemon.

mod matrix;
mod table;

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use hdf5_pure::{AttrValue, Dataset, Error, File, FormatError, Group, Source};
use sandhill_proto::H5adSummary;
use serde_json::Value;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::Semaphore;

const OLD_FILE: &str = "This .h5ad was written by an older anndata that Sandhill can't read";
const NAME_BYTES: usize = 256;
const MAX_KEYS: usize = 256;
const MAX_READ: usize = 16 * 1024 * 1024;
const READ_BUDGET: u64 = 128 * 1024 * 1024;
const MAX_REPLY: usize = 24 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
static WORKERS: Semaphore = Semaphore::const_new(2);

type Attrs = HashMap<String, AttrValue>;

pub(crate) async fn describe(executable: &Path, path: String) -> Result<Value> {
    let _permit = WORKERS.acquire().await?;
    let mut child = Command::new(executable)
        .args(["describe-h5ad", "--", &path])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Could not start the AnnData reader")?;
    let mut stdout = child
        .stdout
        .take()
        .expect("piped stdout")
        .take(MAX_REPLY as u64 + 1);
    let mut stderr = child.stderr.take().expect("piped stderr").take(4097);
    let mut reply = Vec::new();
    let mut error = Vec::new();
    let (reply, error, status) = tokio::time::timeout(TIMEOUT, async {
        tokio::try_join!(
            async move { stdout.read_to_end(&mut reply).await.map(|_| reply) },
            async move { stderr.read_to_end(&mut error).await.map(|_| error) },
            child.wait(),
        )
    })
    .await
    .context("Reading this .h5ad took too long")??;
    if !status.success() {
        let error = String::from_utf8_lossy(&error);
        let error = error.trim().strip_prefix("Error: ").unwrap_or(error.trim());
        ensure!(
            !error.is_empty(),
            "The AnnData reader stopped while reading this .h5ad"
        );
        bail!("{}", clipped(error.to_owned(), 1024));
    }
    ensure!(reply.len() <= MAX_REPLY, "This .h5ad preview is too large");
    Ok(serde_json::from_slice(&reply)?)
}

/// Runs only in the hidden reader command, before opening the HDF5 file.
pub fn worker(path: &Path) -> Result<()> {
    limit_worker_memory()?;
    std::panic::set_hook(Box::new(|_| {}));
    let summary = std::panic::catch_unwind(|| read(path))
        .map_err(|_| anyhow!("The AnnData reader could not read this .h5ad"))??;
    serde_json::to_writer(io::stdout().lock(), &summary)?;
    Ok(())
}

fn limit_worker_memory() -> Result<()> {
    const MEMORY: libc::rlim_t = 256 * 1024 * 1024;
    let ceiling = already_mapped()?.saturating_add(MEMORY);
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes to the supplied rlimit struct.
    if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut limit) } != 0 {
        return Err(io::Error::last_os_error())
            .context("Could not get AnnData reader memory limit");
    }
    limit.rlim_cur = limit.rlim_cur.min(ceiling);
    limit.rlim_max = limit.rlim_max.min(ceiling);
    // SAFETY: setrlimit reads the initialized struct and limits only this worker.
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
        return Err(io::Error::last_os_error()).context("Could not limit AnnData reader memory");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn already_mapped() -> Result<libc::rlim_t> {
    // Darwin counts the shared system mappings and rejects any ceiling below
    // the existing address space. Limit new mappings instead.
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: info is writable for size bytes; the PID is this worker's.
    let read = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    ensure!(
        read == size,
        "Could not measure AnnData reader address space"
    );
    // SAFETY: proc_pidinfo reported writing the complete struct.
    Ok(unsafe { info.assume_init() }.pti_virtual_size)
}

#[cfg(not(target_os = "macos"))]
fn already_mapped() -> Result<libc::rlim_t> {
    Ok(0)
}

// Offset reads keep the input file off the heap even when it is many GB. Cap
// requests before the library allocates its metadata or compressed buffers.
struct FileSource {
    file: std::fs::File,
    len: u64,
    remaining: AtomicU64,
}

impl FileSource {
    fn open(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let stat = file.metadata()?;
        ensure!(stat.is_file(), "An .h5ad preview needs a regular file");
        Ok(Self {
            file,
            len: stat.len(),
            remaining: AtomicU64::new(READ_BUDGET),
        })
    }

    fn claim_read(&self, offset: u64, len: usize) -> Result<(), FormatError> {
        let end = offset
            .checked_add(len as u64)
            .ok_or(FormatError::OffsetOverflow {
                offset,
                length: len as u64,
            })?;
        if end > self.len {
            return Err(FormatError::UnexpectedSourceEof {
                expected: end,
                available: self.len,
            });
        }
        if len > MAX_READ
            || self
                .remaining
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    left.checked_sub(len as u64)
                })
                .is_err()
        {
            return Err(FormatError::Source(
                "This .h5ad exceeds the preview read limit".into(),
            ));
        }
        Ok(())
    }

    fn read_claimed(&self, offset: u64, buf: &mut [u8]) -> Result<(), FormatError> {
        self.file
            .read_exact_at(buf, offset)
            .map_err(|err| FormatError::Source(err.to_string()))
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), FormatError> {
        self.claim_read(offset, buf.len())?;
        self.read_claimed(offset, buf)
    }

    fn read_exact_at(&self, offset: u64, len: usize) -> Result<Vec<u8>, FormatError> {
        self.claim_read(offset, len)?;
        let mut bytes = vec![0; len];
        self.read_claimed(offset, &mut bytes)?;
        Ok(bytes)
    }
}

fn read(path: &Path) -> Result<H5adSummary> {
    let source = FileSource::open(path).context("Could not open this .h5ad")?;
    let file = File::from_source(source).context("Could not read the .h5ad file header")?;
    let root = file.root();
    let attrs = root.attrs()?;
    if attrs.contains_key("encoding-type") {
        encoding(&attrs, &[("anndata", "0.1.0")])?;
    }
    let (obs, n_obs) = table::read(&table::open(&root, "obs")?).context("Reading obs")?;
    let (var, n_vars) = table::read(&table::open(&root, "var")?).context("Reading var")?;
    let x = matrix::read(&root, "X", [n_obs, n_vars]).context("Reading X")?;
    let layers = mapping(&root, "layers", |g, key| {
        matrix::read(g, key, [n_obs, n_vars])?.context("Missing layer matrix")
    })?;
    let obsm = mapping(&root, "obsm", matrix::shape)?;
    let varm = mapping(&root, "varm", matrix::shape)?;
    let obsp = mapping(&root, "obsp", matrix::shape)?;
    let varp = mapping(&root, "varp", matrix::shape)?;
    let uns = match group(&root, "uns")? {
        Some(g) => keys(&g)?
            .into_iter()
            .map(|name| clipped(name, NAME_BYTES))
            .collect(),
        None => Vec::new(),
    };
    let raw_n_vars = match group(&root, "raw") {
        Ok(Some(g)) => {
            let attrs = g.attrs()?;
            if attrs.contains_key("encoding-type") {
                encoding(&attrs, &[("raw", "0.1.0")])?;
            }
            Some(table::dimensions(&table::open(&g, "var")?)?.0)
        }
        Ok(None) => {
            // Pre-0.7 files used dotted root names for raw.
            if dataset(&root, "raw.var")?.is_some() {
                bail!(OLD_FILE);
            }
            None
        }
        Err(Error::NotAGroup(_)) if is_null(&root.dataset("raw")?.attrs()?) => None,
        Err(err) => return Err(err.into()),
    };
    Ok(H5adSummary {
        n_obs,
        n_vars,
        x,
        layers,
        obs,
        var,
        obsm,
        varm,
        obsp,
        varp,
        uns,
        raw_n_vars,
    })
}

fn mapping<T>(
    root: &Group,
    name: &str,
    read: impl Fn(&Group, &str) -> Result<T>,
) -> Result<Vec<(String, T)>> {
    let Some(group) = group(root, name)? else {
        return Ok(Vec::new());
    };
    let attrs = group.attrs()?;
    if attrs.contains_key("encoding-type") {
        encoding(&attrs, &[("dict", "0.1.0")])?;
    }
    keys(&group)?
        .into_iter()
        .map(|key| {
            let value = read(&group, &key)
                .with_context(|| format!("Reading {name}/{}", clipped(key.clone(), NAME_BYTES)))?;
            Ok((clipped(key, NAME_BYTES), value))
        })
        .collect()
}

fn keys(group: &Group) -> Result<Vec<String>> {
    let mut keys = group.datasets()?;
    keys.extend(group.groups()?);
    ensure!(
        keys.len() <= MAX_KEYS,
        "This .h5ad section has more than {MAX_KEYS} entries"
    );
    keys.sort_unstable();
    Ok(keys)
}

fn group(parent: &Group, name: &str) -> Result<Option<Group>, Error> {
    match parent.group(name) {
        Ok(g) => Ok(Some(g)),
        Err(Error::Format(FormatError::PathNotFound(_))) => Ok(None),
        Err(err) => Err(err),
    }
}

fn dataset(parent: &Group, name: &str) -> Result<Option<Dataset>, Error> {
    match parent.dataset(name) {
        Ok(d) => Ok(Some(d)),
        Err(Error::Format(FormatError::PathNotFound(_))) => Ok(None),
        Err(err) => Err(err),
    }
}

fn attr<'a>(attrs: &'a Attrs, name: &str) -> Option<&'a str> {
    match attrs.get(name)? {
        AttrValue::String(s)
        | AttrValue::AsciiString(s)
        | AttrValue::VarLenString(s)
        | AttrValue::VarLenAsciiString(s)
        | AttrValue::StringSized { value: s, .. }
        | AttrValue::AsciiStringSized { value: s, .. } => Some(s),
        _ => None,
    }
}

fn encoding(attrs: &Attrs, supported: &[(&str, &str)]) -> Result<()> {
    let ty = attr(attrs, "encoding-type").unwrap_or("");
    let version = attr(attrs, "encoding-version").unwrap_or("");
    ensure!(
        supported.contains(&(ty, version)),
        "Unsupported AnnData encoding: {} {}",
        clipped(ty.to_owned(), NAME_BYTES),
        clipped(version.to_owned(), NAME_BYTES)
    );
    Ok(())
}

fn is_null(attrs: &Attrs) -> bool {
    attr(attrs, "encoding-type") == Some("null") && attr(attrs, "encoding-version") == Some("0.1.0")
}

fn clipped(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        value.truncate(value.floor_char_boundary(limit - '…'.len_utf8()));
        value.push('…');
    }
    value
}
