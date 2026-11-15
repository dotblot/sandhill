use super::*;

// All storage walking and deletion is descriptor-relative, including ancestors.
// A symlink can neither escape the state directory nor race a pathname check.
fn absolute_storage_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    // macOS publishes these root-owned aliases for /private. Spell their
    // destinations directly; do not traverse the links. Arbitrary user-created
    // symlinks, including a custom state root, are still rejected by openat.
    #[cfg(target_os = "macos")]
    for (alias, destination) in [
        ("/var", "/private/var"),
        ("/tmp", "/private/tmp"),
        ("/etc", "/private/etc"),
    ] {
        if let Ok(relative) = absolute.strip_prefix(alias) {
            return Ok(Path::new(destination).join(relative));
        }
    }
    Ok(absolute)
}
pub(super) struct SafeDir(File);
impl SafeDir {
    pub(super) fn open(path: &Path) -> Result<Self> {
        let absolute = absolute_storage_path(path)?;
        let mut dir = Self(File::open("/")?);
        for component in absolute.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => dir = dir.child(name)?,
                _ => bail!("unsafe storage path {}", path.display()),
            }
        }
        Ok(dir)
    }
    pub(super) fn ensure(path: &Path) -> Result<Self> {
        let absolute = absolute_storage_path(path)?;
        let mut dir = Self(File::open("/")?);
        for component in absolute.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(name) => {
                    let c = CString::new(name.as_bytes())?;
                    if unsafe { libc::mkdirat(dir.0.as_raw_fd(), c.as_ptr(), 0o700) } < 0
                        && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    dir = dir.child(name)?;
                }
                _ => bail!("unsafe directory path"),
            }
        }
        Ok(dir)
    }
    fn child(&self, name: &std::ffi::OsStr) -> Result<Self> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self(unsafe { File::from_raw_fd(fd) }))
    }
    fn names(&self) -> Result<Vec<OsString>> {
        let fd = unsafe { libc::dup(self.0.as_raw_fd()) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            unsafe { libc::close(fd) };
            return Err(std::io::Error::last_os_error().into());
        }
        // dup shares the directory offset; rewind before each independent walk.
        unsafe { libc::rewinddir(stream) };
        let mut names = Vec::new();
        loop {
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            unsafe { *errno = 0 };
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                let error = unsafe { *errno };
                if error != 0 {
                    unsafe { libc::closedir(stream) };
                    return Err(std::io::Error::from_raw_os_error(error).into());
                }
                break;
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                names.push(OsString::from_vec(bytes.to_vec()));
            }
        }
        unsafe { libc::closedir(stream) };
        Ok(names)
    }
    fn stat(&self, name: &std::ffi::OsStr) -> Result<libc::stat> {
        let name = CString::new(name.as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { stat.assume_init() })
    }
    pub(super) fn open_file(&self, name: &std::ffi::OsStr) -> Result<File> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        if !file.metadata()?.is_file() {
            bail!("not a regular owned file");
        }
        Ok(file)
    }
    fn read_file(&self, name: &std::ffi::OsStr) -> Result<Vec<u8>> {
        let mut file = self.open_file(name)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    pub(super) fn write_file(&self, name: &std::ffi::OsStr, bytes: &[u8]) -> Result<()> {
        let name = CString::new(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY
                    | libc::O_CREAT
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        use std::os::unix::fs::MetadataExt;
        let mut file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.nlink() != 1 {
            bail!("refusing to truncate linked or special storage");
        }
        file.set_len(0)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
    pub(super) fn replace_file(
        &self,
        name: &std::ffi::OsStr,
        before: &[u8],
        after: &[u8],
    ) -> Result<()> {
        let temp = OsString::from(format!(".sandhill-{}.tmp", uuid::Uuid::new_v4()));
        self.write_file(&temp, after)?;
        let result = (|| -> Result<()> {
            if self.read_file(name)? != before {
                bail!("configuration changed during removal; preserved");
            }
            let source = CString::new(temp.as_bytes())?;
            let target = CString::new(name.as_bytes())?;
            if unsafe {
                libc::renameat(
                    self.0.as_raw_fd(),
                    source.as_ptr(),
                    self.0.as_raw_fd(),
                    target.as_ptr(),
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = self.remove(&temp);
        }
        result
    }
    fn measure(&self) -> Result<(u64, bool)> {
        let mut total = 0u64;
        let mut links = false;
        for name in self.names()? {
            let stat = self.stat(&name)?;
            match stat.st_mode & libc::S_IFMT {
                libc::S_IFDIR => {
                    let (bytes, linked) = self.child(&name)?.measure()?;
                    total = total.saturating_add(bytes);
                    links |= linked;
                }
                libc::S_IFREG => total = total.saturating_add(stat.st_size.max(0) as u64),
                _ => links = true,
            }
        }
        Ok((total, links))
    }
    pub(super) fn remove(&self, name: &std::ffi::OsStr) -> Result<()> {
        let stat = self.stat(name)?;
        let directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
        if directory {
            let dir = self.child(name)?;
            for entry in dir.names()? {
                dir.remove(&entry)?;
            }
        } else if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            bail!("refusing to remove a symlink or special file");
        }
        let name = CString::new(name.as_bytes())?;
        if unsafe {
            libc::unlinkat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                if directory { libc::AT_REMOVEDIR } else { 0 },
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

pub(super) fn read_safe(path: &Path) -> Result<Vec<u8>> {
    SafeDir::open(path.parent().context("no file parent")?)?
        .read_file(path.file_name().context("no filename")?)
}
pub(super) fn remove_safe(path: &Path) -> Result<()> {
    SafeDir::open(path.parent().context("no file parent")?)?
        .remove(path.file_name().context("no filename")?)
}
pub(super) fn path_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}
pub(super) fn storage_entry(
    id: &str,
    category: &str,
    path: &Path,
    removable: bool,
) -> Result<Option<Value>> {
    if !path_exists(path)? {
        return Ok(None);
    }
    let measured = (|| -> Result<(u64, bool)> {
        let parent = SafeDir::open(path.parent().context("no parent")?)?;
        let name = path.file_name().context("no filename")?;
        let stat = parent.stat(name)?;
        match stat.st_mode & libc::S_IFMT {
            libc::S_IFDIR => parent.child(name)?.measure(),
            libc::S_IFREG => Ok((
                stat.st_size.max(0) as u64,
                matches!(id, "journal" | "daemon_log") && stat.st_nlink > 1,
            )),
            _ => Ok((0, true)),
        }
    })();
    Ok(Some(match measured {
        Ok((bytes, special)) => {
            let mut value = json!({"id":id,"category":category,"path":path,"bytes":bytes,"removable":removable && !special});
            if special {
                value["reason"] =
                    "Contains symlinks, hard links or special files; never traversed or automatically mutated"
                        .into();
            }
            value
        }
        Err(err) => {
            json!({"id":id,"category":category,"path":path,"bytes":0,"removable":false,"reason":format!("Not safely readable: {err:#}")})
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symlinks_are_neither_counted_nor_followed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"private data").unwrap();
        let cache = root.join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("owned"), b"123").unwrap();
        std::os::unix::fs::symlink(&outside, cache.join("escape")).unwrap();
        let entry = storage_entry("cache", "cache", &cache, true)
            .unwrap()
            .unwrap();
        assert_eq!(entry["bytes"], 3);
        assert_eq!(entry["removable"], false);
        assert!(remove_safe(&cache).is_err());
        assert_eq!(
            std::fs::read(outside.join("keep")).unwrap(),
            b"private data"
        );
        assert!(read_safe(&cache.join("escape/keep")).is_err());
    }
    #[test]
    fn log_truncation_refuses_hardlinked_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("user-data");
        std::fs::write(&original, b"keep").unwrap();
        std::fs::hard_link(&original, root.join("daemon.log")).unwrap();
        assert!(
            SafeDir::open(&root)
                .unwrap()
                .write_file(std::ffi::OsStr::new("daemon.log"), b"")
                .is_err()
        );
        assert_eq!(std::fs::read(&original).unwrap(), b"keep");
    }
}
