//! Sandhill's record of what it installed (Huan, 2026-11-19): R packages it
//! added to the user's own library (with the dependencies they pulled in),
//! kernel files it wrote, and agents it installed. One tab-separated line
//! each, `kind, name, version, place, date`, in the daemon's state folder.
//! Getting Ready marks these, and Remove What Sandhill Installed… removes
//! exactly the ones still as Sandhill left them.

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
        if ![R_PACKAGE, KERNEL, AGENT].contains(&kind) || name.is_empty() {
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
    format!(
        "printf '{AGENT}\\t%s\\t\\t%s\\t%s\\n' {} {} \"$(date +%F)\" >> {}",
        shell_quote(kind),
        shell_quote(how),
        shell_quote(&record.to_string_lossy())
    )
}

/// A string as R writes one.
fn r_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The shell lines that remove what is still Sandhill's, then the record.
fn uninstall_lines(items: &[QuickInstalled], record: &Path) -> Vec<String> {
    let mut lines = Vec::new();
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
            _ => {}
        }
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
}
