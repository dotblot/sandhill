//! The user's shell, how a terminal starts it, and the environment it gives
//! the program, resolved as Ghostty does.
//!
//! These need no pty and no feature, so an application that starts its own
//! processes, runs its own pty, or starts a shell on a remote machine gets
//! the same shell as the terminal's `LocalPty`. `terminal_env`, which names
//! Ghostty's terminfo, needs the `ghostty` feature.
//!
//! ```
//! use gpui_cn::shell;
//!
//! // The user's shell, such as /bin/zsh.
//! println!("{}", shell::default_shell().display());
//! // How a terminal starts it as a login shell, as one line.
//! let login = shell::login_shell();
//! println!("{login}");
//! // The same as a process to spawn.
//! let command: std::process::Command = login.command();
//! # let _ = command;
//! ```
//!
//! The rules come from Ghostty's `src/config/Config.zig` (the default
//! command), `src/os/passwd.zig` (the passwd entry) and
//! `src/termio/Exec.zig` (the login shell on macOS).

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// Variables other terminals set that a program in this terminal must not
/// see, since they describe that other terminal. The terminal's `LocalPty`
/// removes them.
pub const FOREIGN_TERMINAL_ENV: &[&str] = &[
    "VTE_VERSION",
    "WT_SESSION",
    "TERM_SESSION_ID",
    "TERMINFO",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_SHELL_FEATURES",
    "GHOSTTY_SHELL_INTEGRATION_XDG_DIR",
    "GHOSTTY_BASH_INJECT",
    "GHOSTTY_BASH_ENV",
    "GHOSTTY_ZSH_ZDOTDIR",
];

/// The user's default shell, as Ghostty picks it with no `command` set.
///
/// In a command-line environment, one with `TERM_PROGRAM` set, it is
/// `$SHELL`. Otherwise, such as an application opened from the Dock or
/// the Finder, `$SHELL` is whatever the launcher had, so the shell comes
/// from the user's passwd entry instead. Then `/bin/sh`.
///
/// Windows has no `$SHELL`, and Ghostty does not run there, so it follows
/// herdr: the first `pwsh.exe` on `PATH`, as a full path, which is what
/// other Windows terminals prefer, else `powershell.exe`, which every
/// Windows has.
pub fn default_shell() -> PathBuf {
    if cfg!(windows) {
        return windows_shell(std::env::var_os("PATH"));
    }
    let cli = std::env::var_os("TERM_PROGRAM").is_some_and(|value| !value.is_empty());
    resolve_shell(
        std::env::var_os("SHELL"),
        cli,
        passwd().and_then(|entry| entry.shell),
    )
}

/// herdr's Windows choice: the first `pwsh.exe` in an absolute `PATH`
/// directory that is a Windows executable, else `powershell.exe`.
fn windows_shell(path: Option<OsString>) -> PathBuf {
    path.as_deref()
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("pwsh.exe"))
        .find(|candidate| is_executable_image(candidate))
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// Whether `path` is a file that starts as a Windows executable does, with
/// the `MZ` header, so a stray file of that name is passed over.
fn is_executable_image(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 2];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok_and(|()| &magic == b"MZ")
}

/// Ghostty's choice between `$SHELL`, the passwd entry and `/bin/sh`.
fn resolve_shell(env: Option<OsString>, cli: bool, passwd: Option<PathBuf>) -> PathBuf {
    let env = env.filter(|value| !value.is_empty()).map(PathBuf::from);
    if cli && let Some(shell) = env.clone() {
        return shell;
    }
    passwd
        .filter(|shell| !shell.as_os_str().is_empty())
        .or(env)
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// A program and its arguments, such as [`login_command`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShellCommand {
    program: PathBuf,
    args: Vec<String>,
}

impl ShellCommand {
    /// `program` with `args`.
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    /// The program to run.
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// The arguments, without the program.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// The program and the arguments.
    pub fn into_parts(self) -> (PathBuf, Vec<String>) {
        (self.program, self.args)
    }

    /// A process that runs it, for the caller to give an environment, a
    /// working directory and stdio, and to spawn.
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

/// The command as a POSIX shell line, each word quoted when it needs it.
impl fmt::Display for ShellCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&word(&self.program.to_string_lossy()))?;
        for arg in &self.args {
            write!(f, " {}", word(arg))?;
        }
        Ok(())
    }
}

/// `value` as one shell word: as it is when it is plain, quoted otherwise.
fn word(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-=+:,@%".contains(c));
    if plain {
        value.to_owned()
    } else {
        quote(value)
    }
}

/// The user's default shell as a login shell, the way a terminal starts
/// it: [`login_command`] for [`default_shell`] with no arguments.
pub fn login_shell() -> ShellCommand {
    login_command(&default_shell(), &[])
}

/// The program and arguments that start `shell` with `args` as a login
/// shell, as Ghostty does.
///
/// On macOS that is `login(1)`, so `getlogin`, `SHELL` and the rest of a
/// login behave as a user expects there: `/usr/bin/login -flp <user>
/// /bin/bash --noprofile --norc -c "exec -l <shell> <args>"`, with `-q`
/// when `~/.hushlogin` exists. `-p` keeps the environment, so shell
/// integration still reaches the shell. Elsewhere, and on macOS when the
/// user is unknown, it is the shell with `-l` first. Windows has no login
/// shell, so there it is the shell and `args` as they are.
pub fn login_command(shell: &Path, args: &[String]) -> ShellCommand {
    if cfg!(windows) {
        return ShellCommand::new(shell, args.iter().cloned());
    }
    let entry = if cfg!(target_os = "macos") {
        passwd()
    } else {
        None
    };
    let hush = entry
        .as_ref()
        .and_then(|entry| entry.home.as_ref())
        .is_some_and(|home| home.join(".hushlogin").exists());
    login_command_for(
        cfg!(target_os = "macos"),
        shell,
        args,
        entry.and_then(|entry| entry.name).as_deref(),
        hush,
    )
}

fn login_command_for(
    macos: bool,
    shell: &Path,
    args: &[String],
    user: Option<&str>,
    hush: bool,
) -> ShellCommand {
    match user.filter(|_| macos) {
        Some(user) => {
            let mut line = format!("exec -l {}", quote(&shell.to_string_lossy()));
            for arg in args {
                line.push(' ');
                line.push_str(&quote(arg));
            }
            let mut argv = Vec::new();
            if hush {
                argv.push("-q".to_owned());
            }
            argv.extend(
                ["-flp", user, "/bin/bash", "--noprofile", "--norc", "-c"].map(str::to_owned),
            );
            argv.push(line);
            ShellCommand::new("/usr/bin/login", argv)
        }
        None => {
            let mut argv = vec!["-l".to_owned()];
            argv.extend(args.iter().cloned());
            ShellCommand::new(shell, argv)
        }
    }
}

/// `value` in single quotes for a POSIX shell, with its own single quotes
/// escaped.
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The variables a terminal sets for its program, as Ghostty sets them:
/// `TERM=xterm-ghostty` with `TERMINFO` pointing at the shipped database,
/// `COLORTERM=truecolor`, `TERM_PROGRAM=ghostty` with its version, and
/// `GHOSTTY_RESOURCES_DIR` for the shell integration scripts.
///
/// The database and the scripts are written to a cache directory on first
/// use; see `ghostty_vt::terminfo::dir`. A remote program needs only
/// `TERM`, `COLORTERM` and `TERM_PROGRAM`, plus the terminfo entry on the
/// remote machine.
#[cfg(feature = "ghostty")]
pub fn terminal_env() -> std::io::Result<Vec<(&'static str, OsString)>> {
    Ok(vec![
        ("TERM", ghostty_vt::terminfo::TERM.into()),
        ("TERMINFO", ghostty_vt::terminfo::dir()?.into_os_string()),
        ("COLORTERM", "truecolor".into()),
        ("TERM_PROGRAM", "ghostty".into()),
        (
            "TERM_PROGRAM_VERSION",
            ghostty_vt::build_info::pinned_version().into(),
        ),
        (
            "GHOSTTY_RESOURCES_DIR",
            ghostty_vt::shell_integration::resources_dir()?.into_os_string(),
        ),
    ])
}

/// The parts of the user's passwd entry a terminal needs.
#[derive(Debug, Default)]
struct Passwd {
    name: Option<String>,
    home: Option<PathBuf>,
    shell: Option<PathBuf>,
}

/// The current user's passwd entry, through `getpwuid_r`.
#[cfg(unix)]
fn passwd() -> Option<Passwd> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt as _;

    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buf = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: every pointer is valid for the call and `buf` outlives the
    // strings `entry` points into, which are copied out before it drops.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &raw mut entry,
            buf.as_mut_ptr(),
            buf.len(),
            &raw mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let text = |pointer: *const libc::c_char| {
        // SAFETY: a non-null field of a filled entry is a C string in `buf`.
        (!pointer.is_null())
            .then(|| unsafe { CStr::from_ptr(pointer) }.to_bytes().to_vec())
            .filter(|bytes| !bytes.is_empty())
    };
    let path = |bytes: Vec<u8>| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    Some(Passwd {
        name: text(entry.pw_name).and_then(|bytes| String::from_utf8(bytes).ok()),
        home: text(entry.pw_dir).map(path),
        shell: text(entry.pw_shell).map(path),
    })
}

#[cfg(not(unix))]
fn passwd() -> Option<Passwd> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_is_from_the_environment_only_on_a_command_line() {
        let fish = Some(PathBuf::from("/opt/homebrew/bin/fish"));
        assert_eq!(
            resolve_shell(Some("/bin/zsh".into()), true, fish.clone()),
            PathBuf::from("/bin/zsh"),
            "a command line keeps its SHELL"
        );
        assert_eq!(
            resolve_shell(Some("/bin/zsh".into()), false, fish.clone()),
            PathBuf::from("/opt/homebrew/bin/fish"),
            "an app from the desktop takes the passwd entry"
        );
        assert_eq!(
            resolve_shell(Some("/bin/zsh".into()), false, None),
            PathBuf::from("/bin/zsh"),
            "with no passwd entry, SHELL"
        );
        assert_eq!(
            resolve_shell(Some("".into()), true, None),
            PathBuf::from("/bin/sh")
        );
        assert_eq!(resolve_shell(None, false, None), PathBuf::from("/bin/sh"));
    }

    #[test]
    fn macos_logs_in_through_login_and_bash_exec() {
        let args = ["--posix".to_owned(), "it's".to_owned()];
        let (program, argv) = login_command_for(
            true,
            Path::new("/opt/homebrew/bin/fish"),
            &args,
            Some("ada"),
            true,
        )
        .into_parts();
        assert_eq!(program, PathBuf::from("/usr/bin/login"));
        assert_eq!(
            argv,
            [
                "-q",
                "-flp",
                "ada",
                "/bin/bash",
                "--noprofile",
                "--norc",
                "-c",
                r"exec -l '/opt/homebrew/bin/fish' '--posix' 'it'\''s'",
            ]
        );
        let (_, quiet) =
            login_command_for(true, Path::new("/bin/zsh"), &[], Some("ada"), false).into_parts();
        assert_eq!(quiet[0], "-flp", "no -q without ~/.hushlogin");
    }

    #[test]
    fn elsewhere_the_shell_takes_a_login_flag() {
        let (program, argv) = login_command_for(
            false,
            Path::new("/bin/zsh"),
            &["-i".to_owned()],
            Some("ada"),
            false,
        )
        .into_parts();
        assert_eq!(program, PathBuf::from("/bin/zsh"));
        assert_eq!(argv, ["-l", "-i"]);
        let (program, _) =
            login_command_for(true, Path::new("/bin/zsh"), &[], None, false).into_parts();
        assert_eq!(program, PathBuf::from("/bin/zsh"), "no user, no login(1)");
    }

    #[cfg(unix)]
    #[test]
    fn the_passwd_entry_names_a_shell_and_a_home() {
        let entry = passwd().expect("the current user has an entry");
        assert!(entry.name.is_some_and(|name| !name.is_empty()));
        assert!(entry.home.is_some_and(|home| home.is_absolute()));
    }

    #[test]
    fn windows_prefers_pwsh_on_the_path_then_powershell() {
        let dir = std::env::temp_dir().join(format!("gpui-cn-pwsh-{}", std::process::id()));
        let (empty, fake, real) = (dir.join("empty"), dir.join("fake"), dir.join("real"));
        for d in [&empty, &fake, &real] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(fake.join("pwsh.exe"), b"#!/bin/sh").unwrap();
        std::fs::write(real.join("pwsh.exe"), b"MZ\x90\0").unwrap();
        let path = |dirs: &[&PathBuf]| Some(std::env::join_paths(dirs).unwrap());
        assert_eq!(
            windows_shell(path(&[&empty, &fake, &real])),
            real.join("pwsh.exe"),
            "the first real executable"
        );
        assert_eq!(
            windows_shell(path(&[&empty, &fake])),
            PathBuf::from("powershell.exe")
        );
        assert_eq!(windows_shell(None), PathBuf::from("powershell.exe"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_command_reads_as_a_shell_line_and_builds_a_process() {
        let command = login_command_for(
            false,
            Path::new("/bin/zsh"),
            &["-i".to_owned(), "a b".to_owned()],
            None,
            false,
        );
        assert_eq!(command.to_string(), "/bin/zsh -l -i 'a b'");
        let process = command.command();
        assert_eq!(process.get_program(), "/bin/zsh");
        assert_eq!(process.get_args().collect::<Vec<_>>(), ["-l", "-i", "a b"]);
    }

    #[cfg(unix)]
    #[test]
    fn the_default_shell_runs_a_command() {
        let shell = default_shell();
        assert!(shell.is_absolute(), "{}", shell.display());
        let status = ShellCommand::new(shell, ["-c", "exit 7"])
            .command()
            .status();
        assert_eq!(status.ok().and_then(|s| s.code()), Some(7));
    }

    #[cfg(feature = "ghostty")]
    #[test]
    fn the_terminal_environment_names_ghostty() {
        let env = terminal_env().unwrap();
        let get = |name: &str| {
            env.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string_lossy().into_owned())
        };
        assert_eq!(get("TERM").as_deref(), Some("xterm-ghostty"));
        assert_eq!(get("COLORTERM").as_deref(), Some("truecolor"));
        assert_eq!(get("TERM_PROGRAM").as_deref(), Some("ghostty"));
        assert!(get("TERMINFO").is_some_and(|dir| Path::new(&dir).is_dir()));
    }
}
