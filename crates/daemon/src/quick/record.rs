//! Sandhill's record of what it installed (Huan, 2026-11-19): R packages it
//! added to the user's own library (with the dependencies they pulled in),
//! kernel files it wrote, agents it installed, and for Python the conda
//! environments it created, ipykernel it added and Miniforge. One
//! tab-separated line each, `kind, name, version, place, date`, in the
//! daemon's state folder. Getting Ready marks these, and Remove What
//! Sandhill Installed… removes exactly the ones still as Sandhill left them.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sandhill_proto::{JobSpec, QuickInstalled};

use crate::agents::shell_quote;
use crate::terminal::Launch;

pub const FILE: &str = "installed-by-sandhill.tsv";
pub const R_PACKAGE: &str = "r-package";
pub const KERNEL: &str = "kernel";
pub const AGENT: &str = "agent";
/// A conda environment Sandhill created: its name, Python version, prefix.
pub const PYTHON_ENV: &str = "python-env";
/// ipykernel added to an environment: its version and the prefix.
pub const IPYKERNEL: &str = "ipykernel";
/// Miniforge: how many packages its base had when installed, its folder.
pub const MINIFORGE: &str = "miniforge";
const KINDS: &[&str] = &[R_PACKAGE, KERNEL, AGENT, PYTHON_ENV, IPYKERNEL, MINIFORGE];

/// The record's lines, oldest first; unreadable lines are skipped.
pub fn read(record: &Path) -> Vec<QuickInstalled> {
    let Ok(text) = fs::read_to_string(record) else {
        return Vec::new();
    };
    let mut items: Vec<QuickInstalled> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [kind, name, version, place, ..] = fields[..] else {
            continue;
        };
        if !KINDS.contains(&kind) || name.is_empty() {
            continue;
        }
        let item = QuickInstalled {
            kind: kind.into(),
            name: name.into(),
            version: version.into(),
            place: place.into(),
            ours: false,
        };
        // A later line for the same thing replaces an earlier one.
        items.retain(|i| !(i.kind == item.kind && i.name == item.name && i.place == item.place));
        items.push(item);
    }
    for item in &mut items {
        item.ours = ours(item);
    }
    // Miniforge is Sandhill's to remove only while it holds nothing else:
    // its base as installed, and only environments Sandhill created.
    let ours_envs: Vec<PathBuf> = items
        .iter()
        .filter(|i| i.kind == PYTHON_ENV && i.ours)
        .map(|i| PathBuf::from(&i.place))
        .collect();
    for item in items.iter_mut().filter(|i| i.kind == MINIFORGE) {
        let root = Path::new(&item.place);
        let base_count = fs::read_dir(root.join("conda-meta")).map(|d| {
            d.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .count()
                .to_string()
        });
        let envs_ours = fs::read_dir(root.join("envs")).map_or(true, |dirs| {
            dirs.flatten()
                .filter(|d| d.path().join("conda-meta").is_dir())
                .all(|d| ours_envs.contains(&d.path()))
        });
        item.ours =
            root.is_absolute() && base_count.is_ok_and(|count| count == item.version) && envs_ours;
    }
    items
}

/// A package's version, from its DESCRIPTION in `library`.
pub fn package_version(library: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(library.join(name).join("DESCRIPTION")).ok()?;
    let package = text.lines().find_map(|l| l.strip_prefix("Package:"))?;
    if package.trim() != name {
        return None;
    }
    text.lines()
        .find_map(|l| l.strip_prefix("Version:"))
        .map(|v| v.trim().to_string())
}

/// An R package's name: letters, digits and dots, starting with a letter.
fn package_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Whether the thing is still as Sandhill left it.
fn ours(item: &QuickInstalled) -> bool {
    match item.kind.as_str() {
        R_PACKAGE => {
            let library = Path::new(&item.place);
            library.is_absolute()
                && package_name(&item.name)
                && package_version(library, &item.name).as_deref() == Some(item.version.as_str())
        }
        KERNEL => {
            let dir = Path::new(&item.place);
            dir.is_absolute()
                && fs::read_to_string(dir.join("kernel.json"))
                    .is_ok_and(|spec| spec.contains("IRkernel::main"))
        }
        AGENT => match (item.name.as_str(), item.place.as_str()) {
            ("claude", "native") => home().is_some_and(|h| h.join(".local/bin/claude").exists()),
            ("codex", "npm") => crate::agents::find("codex").is_some(),
            _ => false,
        },
        PYTHON_ENV => {
            let prefix = Path::new(&item.place);
            prefix.is_absolute() && prefix.join("conda-meta").is_dir()
        }
        IPYKERNEL => {
            let prefix = Path::new(&item.place);
            prefix.is_absolute()
                && super::python::conda_package_version(prefix, "ipykernel").as_deref()
                    == Some(item.version.as_str())
        }
        _ => false,
    }
}

/// R code, for the install jobs, that appends to `record` the packages in
/// `lib` that weren't in `before` (a character vector of names).
pub fn r_append(record: &Path) -> String {
    format!(
        r#"sandhill_record <- function(lib, before) {{
    now <- utils::installed.packages(lib.loc = lib)[, c("Package", "Version"), drop = FALSE]
    new <- now[!(now[, "Package"] %in% before), , drop = FALSE]
    if (nrow(new)) cat(paste0(paste("{kind}", new[, "Package"], new[, "Version"], lib, format(Sys.Date()), sep = "\t"), "\n"), sep = "", file = {record}, append = TRUE)
  }}"#,
        kind = R_PACKAGE,
        record = r_string(&record.to_string_lossy()),
    )
}

/// R code that appends a kernel folder Sandhill wrote to `record`.
pub fn r_append_kernel(record: &Path, dir: &str) -> String {
    format!(
        r#"cat(paste0(paste("{kind}", basename({dir}), "", {dir}, format(Sys.Date()), sep = "\t"), "\n"), sep = "", file = {record}, append = TRUE)"#,
        kind = KERNEL,
        dir = dir,
        record = r_string(&record.to_string_lossy()),
    )
}

/// Shell code appending an agent Sandhill installed.
pub fn sh_append_agent(record: &Path, kind: &str, how: &str) -> String {
    sh_append(record, AGENT, kind, "", how)
}

/// Shell code appending a line to `record`, dated today.
pub fn sh_append(record: &Path, kind: &str, name: &str, version: &str, place: &str) -> String {
    sh_append_raw(record, kind, name, &shell_quote(version), place)
}

/// [`sh_append`] with the version given as a shell word, such as
/// `"$(…)"`, worked out when the job runs.
pub fn sh_append_raw(record: &Path, kind: &str, name: &str, version: &str, place: &str) -> String {
    format!(
        "printf '%s\\t%s\\t%s\\t%s\\t%s\\n' {} {} {version} {} \"$(date +%F)\" >> {}",
        shell_quote(kind),
        shell_quote(name),
        shell_quote(place),
        shell_quote(&record.to_string_lossy())
    )
}

/// A string as R writes one.
fn r_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The shell lines that remove what is still Sandhill's, then the record.
/// Miniforge goes last, and what is inside a Miniforge being removed goes
/// with it.
fn uninstall_lines(items: &[QuickInstalled], record: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let removed_roots: Vec<&Path> = items
        .iter()
        .filter(|i| i.kind == MINIFORGE && i.ours)
        .map(|i| Path::new(&i.place))
        .collect();
    let inside_removed = |place: &str| {
        removed_roots
            .iter()
            .any(|r| Path::new(place).starts_with(r))
    };
    let removed_envs: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == PYTHON_ENV && i.ours)
        .map(|i| i.place.as_str())
        .collect();
    for item in items.iter().filter(|i| i.ours) {
        match item.kind.as_str() {
            R_PACKAGE => {
                let dir = Path::new(&item.place).join(&item.name);
                lines.push(format!(
                    "echo {} && rm -rf {}",
                    shell_quote(&format!("Removing the R package {} {}", item.name, item.version)),
                    shell_quote(&dir.to_string_lossy())
                ));
            }
            KERNEL => lines.push(format!(
                "echo {} && rm -rf {}",
                shell_quote(&format!("Removing the kernel {}", item.name)),
                shell_quote(&item.place)
            )),
            AGENT => match item.name.as_str() {
                "claude" => lines.push(
                    "echo 'Removing Claude Code (its settings in ~/.claude stay)' && rm -f \"$HOME/.local/bin/claude\" && rm -rf \"$HOME/.local/share/claude\"".into(),
                ),
                "codex" => lines.push(
                    "echo 'Removing Codex (its settings in ~/.codex stay)' && npm uninstall -g @openai/codex".into(),
                ),
                _ => {}
            },
            PYTHON_ENV if !inside_removed(&item.place) => lines.push(format!(
                "echo {} && {} env remove -y -p {}",
                shell_quote(&format!("Removing the Python environment {}", item.name)),
                shell_quote(&crate::irkernel::conda_tool(Path::new(&item.place))),
                shell_quote(&item.place)
            )),
            IPYKERNEL
                if !inside_removed(&item.place) && !removed_envs.contains(&item.place.as_str()) =>
            {
                lines.push(format!(
                    "echo {} && {} remove -y -p {} ipykernel",
                    shell_quote(&format!("Removing ipykernel {} from {}", item.version, item.place)),
                    shell_quote(&crate::irkernel::conda_tool(Path::new(&item.place))),
                    shell_quote(&item.place)
                ))
            }
            _ => {}
        }
    }
    for root in &removed_roots {
        lines.push(format!(
            "echo {} && rm -rf {}",
            shell_quote(&format!("Removing Miniforge in {}", root.display())),
            shell_quote(&root.to_string_lossy())
        ));
    }
    let skipped = items.iter().filter(|i| !i.ours).count();
    if skipped > 0 {
        lines.push(format!(
            "echo {}",
            shell_quote(&format!(
                "Left {skipped} that changed since Sandhill installed them: they are yours now."
            ))
        ));
    }
    lines.push(format!("rm -f {}", shell_quote(&record.to_string_lossy())));
    lines.push("echo 'Done: what Sandhill installed is removed.'".into());
    lines
}

/// The job that removes what Sandhill installed, its output also in `log`.
pub fn uninstall(record: &Path, log: &Path) -> Result<Launch> {
    let items = read(record);
    if items.is_empty() {
        bail!("Sandhill hasn't installed anything here");
    }
    let script = uninstall_lines(&items, record).join(" && ");
    let home = home().context("HOME isn't set")?;
    let job = JobSpec {
        path: None,
        command: Some(format!(
            "set -o pipefail; ( {script} ) 2>&1 | tee {}",
            shell_quote(&log.to_string_lossy())
        )),
    };
    let mut launch = crate::jobs::launch(&job, Some(&home), None)?;
    launch.title = "Remove what Sandhill installed".into();
    Ok(launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(lib: &Path, name: &str, version: &str) {
        fs::create_dir_all(lib.join(name)).unwrap();
        fs::write(
            lib.join(name).join("DESCRIPTION"),
            format!("Package: {name}\nVersion: {version}\n"),
        )
        .unwrap();
    }

    #[test]
    fn only_what_is_still_as_sandhill_left_it_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("library");
        package(&lib, "ggrepel", "0.9.6");
        package(&lib, "writexl", "1.5.4");
        package(&lib, "ggplot2", "4.0.3");
        let record = dir.path().join(FILE);
        let lib_s = lib.to_string_lossy();
        fs::write(
            &record,
            format!(
                "r-package\tggrepel\t0.9.6\t{lib_s}\t2026-11-19\nr-package\twritexl\t1.5.3\t{lib_s}\t2026-11-19\nbad line\nr-package\t../etc\t1\t{lib_s}\t2026-11-19\n"
            ),
        )
        .unwrap();
        let items = read(&record);
        let ours: Vec<&str> = items
            .iter()
            .filter(|i| i.ours)
            .map(|i| i.name.as_str())
            .collect();
        // writexl was updated since; ggplot2 was never Sandhill's.
        assert_eq!(ours, vec!["ggrepel"]);
        let script = uninstall_lines(&items, &record).join("\n");
        assert!(
            script.contains(&format!("rm -rf '{lib_s}/ggrepel'")),
            "{script}"
        );
        assert!(!script.contains("writexl'"));
        assert!(!script.contains("ggplot2"));
        assert!(!script.contains("../etc"));
        assert!(script.contains("Left 2 that changed"));
    }

    #[test]
    fn an_empty_record_has_nothing_to_remove() {
        let dir = tempfile::tempdir().unwrap();
        let err = uninstall(&dir.path().join(FILE), &dir.path().join("log")).unwrap_err();
        assert!(err.to_string().contains("hasn't installed anything"));
    }

    fn conda_env(prefix: &Path, packages: &[&str]) {
        fs::create_dir_all(prefix.join("conda-meta")).unwrap();
        for package in packages {
            fs::write(
                prefix.join("conda-meta").join(format!("{package}.json")),
                "{}",
            )
            .unwrap();
        }
    }

    #[test]
    fn python_environments_ipykernel_and_miniforge_are_removed_only_while_sandhills() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("miniforge3");
        conda_env(&root, &["python-3.12.4-h_0", "conda-25.1.1-py_0"]);
        let made = root.join("envs/sandhill-python");
        conda_env(&made, &["python-3.12.4-h_0", "ipykernel-7.2.0-pyh_0"]);
        let theirs = dir.path().join("other/envs/old");
        conda_env(&theirs, &["python-3.11.9-h_0", "ipykernel-6.29.5-pyh_0"]);
        let record = dir.path().join(FILE);
        let line = |kind: &str, name: &str, version: &str, place: &Path| {
            format!(
                "{kind}\t{name}\t{version}\t{}\t2026-11-19\n",
                place.display()
            )
        };
        let text = [
            line(MINIFORGE, "miniforge", "2", &root),
            line(PYTHON_ENV, "sandhill-python", "3.12", &made),
            line(IPYKERNEL, "ipykernel", "6.29.5", &theirs),
        ]
        .concat();
        fs::write(&record, &text).unwrap();
        let items = read(&record);
        assert!(items.iter().all(|i| i.ours), "{items:?}");
        let script = uninstall_lines(&items, &record);
        let joined = script.join("\n");
        // The environment goes with Miniforge, which goes last.
        assert!(!joined.contains("env remove"), "{joined}");
        assert!(joined.contains(&format!("remove -y -p '{}' ipykernel", theirs.display())));
        let last_removal = script.iter().rposition(|l| l.contains("rm -rf")).unwrap();
        assert!(script[last_removal].contains("Removing Miniforge"));

        // Another environment in Miniforge, or a package added to its base,
        // makes Miniforge the user's; Sandhill's environment then goes alone.
        conda_env(&root.join("envs/mine"), &["python-3.13.0-h_0"]);
        let items = read(&record);
        let miniforge = items.iter().find(|i| i.kind == MINIFORGE).unwrap();
        assert!(!miniforge.ours);
        let joined = uninstall_lines(&items, &record).join("\n");
        assert!(joined.contains(&format!("env remove -y -p '{}'", made.display())));
        assert!(!joined.contains("Removing Miniforge"));

        // ipykernel updated since is the user's.
        fs::remove_file(theirs.join("conda-meta/ipykernel-6.29.5-pyh_0.json")).unwrap();
        conda_env(&theirs, &["ipykernel-7.2.0-pyh_0"]);
        assert!(read(&record).iter().any(|i| i.kind == IPYKERNEL && !i.ours));
    }
}
