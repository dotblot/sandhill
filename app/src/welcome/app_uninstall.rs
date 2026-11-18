//! The explicit, final uninstall step for the currently running packaged app.
//! Development executables are never interpreted as removable applications.

#[cfg(any(target_os = "macos", test))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(target_os = "macos", test))]
use anyhow::ensure;
use anyhow::{Context as _, Result};

#[cfg(any(target_os = "macos", test))]
fn bundle_for_executable(executable: &Path) -> Result<Option<PathBuf>> {
    let Some(macos) = executable.parent() else {
        return Ok(None);
    };
    let Some(contents) = macos.parent() else {
        return Ok(None);
    };
    let Some(bundle) = contents.parent() else {
        return Ok(None);
    };
    if macos.file_name().is_none_or(|name| name != "MacOS")
        || contents.file_name().is_none_or(|name| name != "Contents")
        || bundle
            .extension()
            .is_none_or(|extension| extension != "app")
        || executable.file_name().is_none_or(|name| name != "sandhill")
    {
        return Ok(None);
    }
    ensure!(
        bundle.is_absolute(),
        "the running application path is not absolute"
    );
    let canonical = bundle
        .canonicalize()
        .context("cannot resolve the running application bundle")?;
    ensure!(
        canonical == bundle,
        "refusing a symlinked application bundle path"
    );
    for path in [
        bundle.to_path_buf(),
        contents.to_path_buf(),
        macos.to_path_buf(),
        executable.to_path_buf(),
        contents.join("Info.plist"),
    ] {
        let metadata = std::fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "refusing a symlink in the running application bundle"
        );
        if path == executable || path.file_name().is_some_and(|name| name == "Info.plist") {
            ensure!(
                metadata.is_file(),
                "the running application bundle is incomplete"
            );
        } else {
            ensure!(
                metadata.is_dir(),
                "the running application bundle is incomplete"
            );
        }
    }
    ensure!(
        executable.canonicalize()?.starts_with(&canonical),
        "the running executable is outside its application bundle"
    );
    #[cfg(target_os = "macos")]
    if !bundle_identity_matches(&canonical)? {
        return Ok(None);
    }
    Ok(Some(canonical))
}

/// NSBundle handles XML and binary plists; do not guess identity by scanning
/// text or trust a caller-supplied application path.
#[cfg(target_os = "macos")]
fn bundle_identity_matches(bundle: &Path) -> Result<bool> {
    use objc2::{
        msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::NSString;

    let bundle_class =
        AnyClass::get(c"NSBundle").context("the system bundle reader is unavailable")?;
    let string_class =
        AnyClass::get(c"NSString").context("the system string class is unavailable")?;
    let path = NSString::from_str(
        bundle
            .to_str()
            .context("application path is not valid UTF-8")?,
    );
    // SAFETY: Public Foundation selectors, with dynamically read plist values
    // checked to be NSString before using the NSString comparison selector.
    unsafe {
        let object: Option<Retained<AnyObject>> = msg_send![bundle_class, bundleWithPath: &*path];
        let Some(object) = object else {
            return Ok(false);
        };
        for (key, expected) in [
            ("CFBundleIdentifier", "dev.sandhill.ide"),
            ("CFBundleExecutable", "sandhill"),
            ("CFBundlePackageType", "APPL"),
        ] {
            let key = NSString::from_str(key);
            let value: Option<Retained<AnyObject>> =
                msg_send![&*object, objectForInfoDictionaryKey: &*key];
            let Some(value) = value else { return Ok(false) };
            let is_string: bool = msg_send![&*value, isKindOfClass: string_class];
            if !is_string {
                return Ok(false);
            }
            let expected = NSString::from_str(expected);
            let matches: bool = msg_send![&*value, isEqualToString: &*expected];
            if !matches {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// The only application that this process is allowed to offer for removal.
/// None is normal for `cargo run`, CLI binaries and other development layouts.
pub(crate) fn preview_path() -> Result<Option<PathBuf>> {
    #[cfg(target_os = "macos")]
    {
        bundle_for_executable(&std::env::current_exe()?)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// On Linux the package manager removes the app, not Sandhill: the command
/// for it when this is the installed package (`/usr/lib/sandhill`).
pub(crate) fn package_removal() -> Option<&'static str> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    exe.starts_with("/usr/lib/sandhill")
        .then_some("sudo apt remove sandhill")
}

/// Moves only the running .app to the user's Trash using NSFileManager.
/// Call only after explicit confirmation and successful device teardown. This
/// does not quit the process, delete preferences or touch any other app copy.
pub(crate) fn move_to_trash() -> Result<PathBuf> {
    let path = preview_path()?.context("this is not a packaged macOS application")?;
    #[cfg(target_os = "macos")]
    {
        use objc2::{
            msg_send,
            rc::Retained,
            runtime::{AnyClass, AnyObject},
        };
        use objc2_foundation::NSString;

        let class = |name: &'static std::ffi::CStr| -> Result<&'static AnyClass> {
            AnyClass::get(name)
                .with_context(|| format!("system class unavailable: {}", name.to_string_lossy()))
        };
        let string = NSString::from_str(
            path.to_str()
                .context("application path is not valid UTF-8")?,
        );
        // SAFETY: These are public Foundation selectors, with NSError** and
        // NSURL** out-parameters represented by pointers to Objective-C objects.
        // NSFileManager performs the actual Trash operation (not unlink/rm).
        unsafe {
            let manager: Option<Retained<AnyObject>> =
                msg_send![class(c"NSFileManager")?, defaultManager];
            let manager = manager.context("the system file manager is unavailable")?;
            let url: Option<Retained<AnyObject>> =
                msg_send![class(c"NSURL")?, fileURLWithPath: &*string];
            let url = url.context("the system could not resolve the application URL")?;
            // Recheck at the destructive boundary, not just when rendering the
            // confirmation UI, and never accept an arbitrary caller pathname.
            ensure!(
                preview_path()?.as_ref() == Some(&path),
                "the running application changed during uninstall"
            );
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let success: bool = msg_send![&*manager, trashItemAtURL: &*url, resultingItemURL: std::ptr::null_mut::<*mut AnyObject>(), error: &mut error];
            if !success {
                let detail = if let Some(error) = error.as_ref() {
                    let text: Retained<NSString> = msg_send![error, localizedDescription];
                    text.to_string()
                } else {
                    "the system refused to move the application to Trash".into()
                };
                anyhow::bail!("{detail}");
            }
        }
        ensure!(
            !path.try_exists()?,
            "the system reported success but the application remains at {}",
            path.display()
        );
        Ok(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        anyhow::bail!("application Trash is only available on macOS")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_executable_is_not_an_application() {
        assert!(
            bundle_for_executable(Path::new("/project/target/debug/sandhill"))
                .unwrap()
                .is_none()
        );
        assert!(
            bundle_for_executable(Path::new("/project.app/target/debug/sandhill"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn packaged_path_is_previewed_but_never_deleted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let bundle = root.join("Sandhill.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>dev.sandhill.ide</string><key>CFBundleExecutable</key><string>sandhill</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#).unwrap();
        let executable = macos.join("sandhill");
        std::fs::write(&executable, b"test fixture").unwrap();
        assert_eq!(
            bundle_for_executable(&executable).unwrap(),
            Some(bundle.clone())
        );
        assert!(executable.exists());
        let linked = root.join("Linked.app");
        std::os::unix::fs::symlink(&bundle, &linked).unwrap();
        assert!(bundle_for_executable(&linked.join("Contents/MacOS/sandhill")).is_err());
        let wrong_executable = macos.join("other-app");
        std::fs::write(&wrong_executable, b"not Sandhill").unwrap();
        assert!(bundle_for_executable(&wrong_executable).unwrap().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn other_bundle_identity_is_not_offered_for_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().canonicalize().unwrap().join("Other.app");
        let macos = bundle.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        let executable = macos.join("sandhill");
        std::fs::write(&executable, b"not Sandhill").unwrap();
        std::fs::write(bundle.join("Contents/Info.plist"), br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.example.other</string><key>CFBundleExecutable</key><string>sandhill</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#).unwrap();
        assert!(bundle_for_executable(&executable).unwrap().is_none());
    }
}
