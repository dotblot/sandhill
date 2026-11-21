use std::fmt::Write as _;
use std::os::unix::process::CommandExt as _;

use super::*;

pub(super) struct File {
    parent: String,
    name: String,
    identity: String,
    empty: bool,
    checksum: Option<String>,
}

impl File {
    pub(super) fn new(path: &Path, empty: bool) -> Result<Self> {
        let parent = path.parent().context("Missing parent folder.")?;
        // Refuse arbitrary symlink ancestors; the OS aliases for /tmp and
        // /var are handled by SafeDir on macOS.
        SafeDir::open(parent)?;
        let real_parent = parent.canonicalize()?;
        let meta = path.symlink_metadata()?;
        Ok(Self {
            parent: real_parent
                .to_str()
                .context("The parent path isn't text.")?
                .into(),
            name: path
                .file_name()
                .and_then(|n| n.to_str())
                .context("The file name isn't text.")?
                .into(),
            identity: format!("{}:{}", meta.dev(), meta.ino()),
            empty,
            checksum: None,
        })
    }

    pub(super) fn with_checksum(mut self, checksum: String) -> Self {
        self.checksum = Some(checksum);
        self
    }

    pub(super) fn command(&self) -> String {
        format!(
            "{} -- {}",
            if self.empty {
                "/bin/rmdir"
            } else {
                "/bin/rm -rf"
            },
            shell_quote(&format!("{}/{}", self.parent, self.name))
        )
    }

    fn script_args(&self) -> [String; 5] {
        [
            shell_quote(&self.parent),
            shell_quote(&format!("./{}", self.name)),
            shell_quote(&self.identity),
            shell_quote(if self.empty { "empty" } else { "tree" }),
            shell_quote(self.checksum.as_deref().unwrap_or_default()),
        ]
    }
}

#[derive(Clone, Copy)]
pub(super) enum Service {
    Launchd,
    Systemd,
}

impl Service {
    pub(super) fn for_platform(component: &str) -> Option<Self> {
        match (std::env::consts::OS, component) {
            ("macos", "launchd") => Some(Self::Launchd),
            ("linux", "systemd") => Some(Self::Systemd),
            _ => None,
        }
    }

    pub(super) fn command(self) -> String {
        match self {
            Self::Launchd => format!(
                "/bin/launchctl bootout gui/{}/dev.sandhill.sandhilld",
                unsafe { libc::getuid() }
            ),
            Self::Systemd => "systemctl --user disable --now sandhilld.service".into(),
        }
    }
}

pub(super) enum Task {
    Remove(File),
    Component {
        file: File,
        service: Option<Service>,
    },
    Defaults,
}

pub(super) fn app_pid(pid: Option<u32>) -> Result<Option<u32>> {
    if !cfg!(target_os = "macos") {
        return Ok(None);
    }
    let Some(pid) = pid else { return Ok(None) };
    let identity =
        ProcessIdentity::read(pid).context("Couldn't verify the Sandhill app process.")?;
    if identity.uid != unsafe { libc::getuid() }
        || !identity
            .program()
            .is_some_and(|path| path.file_name().is_some_and(|name| name == "sandhill"))
    {
        bail!("The process that asked isn't the Sandhill app on this Mac.");
    }
    Ok(Some(pid))
}

const TEMP: &str = "/tmp";

fn marker_prefix(home: &Path) -> String {
    let digest = sandhill_proto::hex(&Sha256::digest(home.as_os_str().as_bytes()));
    format!("sandhill-uninstall-{}-{digest}", unsafe { libc::getuid() })
}

fn active_removal(prefix: &str) -> Result<bool> {
    for entry in fs::read_dir(TEMP)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(prefix))
            .and_then(|name| name.strip_prefix('-'))
            .and_then(|pid| pid.parse::<u32>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        let path = entry.path();
        if !path
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file() && meta.uid() == unsafe { libc::getuid() })
        {
            continue;
        }
        let Ok(expected) = fs::read_to_string(&path) else {
            continue;
        };
        if process(pid).is_some_and(|process| !process.zombie)
            && birth(pid).is_ok_and(|current| current == expected)
        {
            return Ok(true);
        }
        let _ = fs::remove_file(&path);
    }
    Ok(false)
}

pub(in crate::lifecycle) async fn wait_for_removal(home: &Path) -> Result<()> {
    let prefix = marker_prefix(&home.canonicalize()?);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while active_removal(&prefix)? {
        if tokio::time::Instant::now() >= deadline {
            bail!("Sandhill is still being removed from this computer. Try again in a moment.");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
fn birth(pid: u32) -> Result<String> {
    let out = Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .output()?;
    let text = String::from_utf8(out.stdout)?;
    if !out.status.success() || text.trim_end().is_empty() {
        bail!("Couldn't identify the Sandhill process {pid}.");
    }
    Ok(text.trim_end_matches('\n').to_owned())
}

pub(super) fn start(
    home: &Path,
    mac: bool,
    app: Option<u32>,
    tasks: &[(String, Task)],
) -> Result<()> {
    let mut pids = vec![(std::process::id(), birth(std::process::id())?)];
    if let Some(pid) = app {
        pids.push((pid, birth(pid)?));
    }
    launch(home, mac, &pids, tasks)?;
    Ok(())
}

fn launch(
    home: &Path,
    mac: bool,
    pids: &[(u32, String)],
    tasks: &[(String, Task)],
) -> Result<std::process::Child> {
    let mut file = tempfile::Builder::new()
        .prefix("sandhill-uninstall-")
        .suffix(".sh")
        .tempfile_in(TEMP)?;
    file.write_all(script(home, mac, pids, tasks)?.as_bytes())?;
    file.as_file().sync_all()?;
    let marker = Path::new(TEMP).join(marker_prefix(home));
    let mut child = Command::new("/usr/bin/nohup")
        .arg("/bin/sh")
        .arg(file.path())
        .arg(&marker)
        .current_dir("/")
        .env("HOME", home)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Couldn't start the final removal.")?;
    let publish = (|| -> Result<()> {
        let pid = child.id();
        let mut staging = tempfile::NamedTempFile::new_in(TEMP)?;
        staging.write_all(birth(pid)?.as_bytes())?;
        staging.persist_noclobber(format!("{}-{pid}", marker.display()))?;
        Ok(())
    })();
    if let Err(err) = publish {
        // The script still waits for us and hasn't started removing anything.
        let _ = child.kill();
        let _ = child.wait();
        return Err(err.context("Couldn't mark removal in progress."));
    }
    // The script removes itself on exit, not when the daemon closes.
    file.disable_cleanup(true);
    Ok(child)
}

fn script(
    home: &Path,
    mac: bool,
    pids: &[(u32, String)],
    tasks: &[(String, Task)],
) -> Result<String> {
    let report = if mac {
        home.join("Desktop")
    } else {
        home.to_path_buf()
    }
    .join("sandhill-uninstall-leftovers.txt");
    let report = shell_quote(report.to_str().context("The report path isn't text.")?);
    let mut text = format!(
        "#!/bin/sh\numask 077\nmarker=${{1:+$1-$$}}\ntrap '/bin/rm -f -- \"$0\"; [ -z \"$marker\" ] || /bin/rm -f -- \"$marker\"' 0\nreport={report}\nfailures=''\nstat_identity() {{ {}; }}\nchecksum() {{ {}; }}\n",
        if mac {
            "/usr/bin/stat -f '%d:%i' \"$1\""
        } else {
            "/usr/bin/stat -c '%d:%i' -- \"$1\""
        },
        if mac {
            "/usr/bin/shasum -a 256 < \"$1\""
        } else {
            "/usr/bin/sha256sum < \"$1\""
        },
    );
    text.push_str(r#"fail() {
    failures="${failures}$1: $2
"
}
wait_for() {
    while [ "$(/bin/ps -o lstart= -p "$1")" = "$2" ]; do
        case "$(/bin/ps -o stat= -p "$1")" in *Z*|'') break;; esac
        /bin/sleep 0.1
    done
}
remove_path() (
    parent=$1 leaf=$2 identity=$3 mode=$4 expected=$5 service=$6
    [ -e "$parent" ] || [ -L "$parent" ] || exit 0
    cd -P -- "$parent" || exit 1
    [ "$(/bin/pwd -P)" = "$parent" ] || { echo 'The parent folder changed.'; exit 1; }
    [ -e "$leaf" ] || [ -L "$leaf" ] || exit 0
    [ "$(stat_identity "$leaf")" = "$identity" ] || { echo 'The file or folder changed.'; exit 1; }
    if [ -n "$expected" ]; then
        sum=$(checksum "$leaf") || exit 1
        [ "${sum%% *}" = "$expected" ] || { echo "The file no longer matches Sandhill's record."; exit 1; }
    fi
    case "$service" in
        launchd)
            target="gui/$(/usr/bin/id -u)/dev.sandhill.sandhilld"
            probe=$(/bin/launchctl print "$target" 2>&1) && loaded=yes || loaded=no
            if [ "$loaded" = yes ]; then
                case "$probe" in *"path = $parent/${leaf#./}"*) ;; *) echo 'This service belongs to another file.'; exit 1;; esac
                /bin/launchctl bootout "$target" || exit 1
            else
                case "$probe" in *'Could not find service'*) ;; *) printf '%s\n' "$probe"; exit 1;; esac
            fi
            ;;
        systemd)
            fragment=$(systemctl --user show --value -p FragmentPath sandhilld.service) || exit 1
            [ -z "$fragment" ] || [ "$fragment" = "$parent/${leaf#./}" ] || { echo 'This service belongs to another file.'; exit 1; }
            systemctl --user disable --now sandhilld.service || exit 1
            ;;
    esac
    case "$mode" in
        empty) /bin/rmdir -- "$leaf" ;;
        tree) /bin/rm -rf -- "$leaf" ;;
    esac
)
"#);
    for (pid, start) in pids {
        writeln!(text, "wait_for {pid} {}", shell_quote(start))?;
    }
    for (path, task) in tasks {
        let path = shell_quote(path);
        let (file, service) = match task {
            Task::Remove(file) => (file, ""),
            Task::Component { file, service } => (
                file,
                match service {
                    None => "",
                    Some(Service::Launchd) => "launchd",
                    Some(Service::Systemd) => "systemd",
                },
            ),
            Task::Defaults => {
                writeln!(
                    text,
                    "if ! error=$(/usr/bin/defaults delete dev.sandhill.ide 2>&1); then\n    case \"$error\" in *'not found'*) ;; *) fail {path} \"$error\";; esac\nfi"
                )?;
                continue;
            }
        };
        writeln!(
            text,
            "error=$(remove_path {} '{}' 2>&1) || fail {path} \"$error\"",
            file.script_args().join(" "),
            service
        )?;
    }
    text.push_str("if [ -n \"$failures\" ]; then\n    (set -C; printf '%s\\n%s' 'Sandhill could not remove these:' \"$failures\" > \"$report\")\nfi\n");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_both_pids_before_exactly_the_listed_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let target = root.join("Sandhill's folder");
        let preserved = root.join("Project");
        fs::create_dir(&target).unwrap();
        fs::create_dir(&preserved).unwrap();
        let script = script(
            &root,
            false,
            &[(11, "daemon".into()), (12, "app".into())],
            &[(
                display(&target),
                Task::Remove(File::new(&target, false).unwrap()),
            )],
        )
        .unwrap();
        let first = script.find("wait_for 11 'daemon'").unwrap();
        let second = script.find("wait_for 12 'app'").unwrap();
        let remove = script.find("error=$(remove_path").unwrap();
        assert!(first < second && second < remove);
        assert!(script[remove..].contains(&shell_quote("./Sandhill's folder")));
        assert!(!script.contains("Project"));
        assert!(script.contains(
            "trap '/bin/rm -f -- \"$0\"; [ -z \"$marker\" ] || /bin/rm -f -- \"$marker\"' 0"
        ));
        assert!(script.contains("sandhill-uninstall-leftovers.txt"));
    }

    #[tokio::test]
    async fn marker_blocks_reopen_until_the_real_finisher_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let mut parent = Command::new("/bin/sleep").arg("20").spawn().unwrap();
        let pid = parent.id();
        let mut child = launch(
            &root,
            cfg!(target_os = "macos"),
            &[(pid, birth(pid).unwrap())],
            &[(
                display(&target),
                Task::Remove(File::new(&target, false).unwrap()),
            )],
        )
        .unwrap();
        let marker = Path::new(TEMP).join(format!("{}-{}", marker_prefix(&root), child.id()));
        let active = marker.is_file() && target.exists();
        let waiting =
            tokio::time::timeout(Duration::from_millis(150), wait_for_removal(&root)).await;
        parent.kill().unwrap();
        parent.wait().unwrap();
        let exit = child.wait().unwrap();
        assert!(exit.success());
        assert!(active);
        assert!(waiting.is_err());
        assert!(!marker.exists());
        assert!(!target.exists());
        assert!(!root.join("sandhill-uninstall-leftovers.txt").exists());
        assert!(
            !root
                .join("Desktop/sandhill-uninstall-leftovers.txt")
                .exists()
        );
    }
}
