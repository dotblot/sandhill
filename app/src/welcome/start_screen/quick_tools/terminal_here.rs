//! Open Terminal Here: a Terminal window with one environment's settings,
//! for those who want it. Nothing is written to a file: the settings are
//! typed into the new window's shell (Terminal.app's `do script` on a Mac),
//! or given to the terminal's shell (Linux), or to `ssh` for a server.

use std::process::Command;

use sandhill_daemon::agents::shell_quote;

/// Terminals tried on Linux, the system's choice first, with how each is
/// told the command to run.
const LINUX_TERMINALS: &[(&str, &[&str])] = &[
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("xterm", &["-e"]),
];

/// A name a shell takes for a variable.
fn variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// The shell lines that put a shell in the environment: `cd` to the home
/// folder, then each variable set or unset.
pub(super) fn settings(vars: &[(String, Option<String>)]) -> String {
    let mut lines = vec!["cd".to_owned()];
    for (name, value) in vars.iter().filter(|(name, _)| variable_name(name)) {
        lines.push(match value {
            Some(value) => format!("export {name}={}", shell_quote(value)),
            None => format!("unset {name}"),
        });
    }
    lines.join("; ")
}

/// The command a new shell runs for these settings: they, then the
/// user's own interactive shell.
fn shell_command(vars: &[(String, Option<String>)]) -> String {
    format!("{}; exec \"${{SHELL:-/bin/sh}}\" -i", settings(vars))
}

/// What the local terminal runs: the settings here, or `ssh` to a server
/// with the settings there.
pub(super) fn command(host: Option<&str>, vars: &[(String, Option<String>)]) -> Vec<String> {
    match host {
        None => vec!["sh".into(), "-c".into(), shell_command(vars)],
        Some(host) => vec!["ssh".into(), "-t".into(), host.into(), shell_command(vars)],
    }
}

/// AppleScript's string literal for `text`.
fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The line Terminal.app types into its new window: the settings right
/// after the user's shell has started, or `ssh` to the server.
pub(super) fn typed_line(host: Option<&str>, vars: &[(String, Option<String>)]) -> String {
    match host {
        None => format!("{}; clear", settings(vars)),
        Some(_) => command(host, vars)
            .iter()
            .map(|word| shell_quote(word))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Opens the terminal. `host` is a server's ssh name, or `None` for this
/// computer.
pub(super) fn open(host: Option<&str>, vars: &[(String, Option<String>)]) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        let line = typed_line(host, vars);
        let status = Command::new("/usr/bin/osascript")
            .args(["-e", "tell application \"Terminal\""])
            .args(["-e", &format!("do script {}", applescript_string(&line))])
            .args(["-e", "activate", "-e", "end tell"])
            .status()
            .map_err(|error| format!("Terminal didn't open: {error}"))?;
        return if status.success() {
            Ok(())
        } else {
            Err("Terminal didn't open. Sandhill may need permission to control Terminal: see System Settings, Privacy and Security, Automation.".into())
        };
    }
    let command = command(host, vars);
    let home = std::env::var_os("HOME");
    for (terminal, run) in LINUX_TERMINALS {
        let mut launch = Command::new(terminal);
        launch.args(*run).args(&command);
        if let Some(home) = &home {
            launch.current_dir(home);
        }
        if launch.spawn().is_ok() {
            return Ok(());
        }
    }
    Err("No terminal was found on this computer.".into())
}

#[cfg(test)]
mod tests {
    use super::{applescript_string, command, settings, typed_line};

    fn vars() -> Vec<(String, Option<String>)> {
        vec![
            ("CONDA_PREFIX".into(), Some("/Users/me/my env".into())),
            ("PATH".into(), Some("/Users/me/my env/bin:/usr/bin".into())),
            ("PYTHONPATH".into(), None),
            ("not a name".into(), Some("x".into())),
        ]
    }

    #[test]
    fn settings_quote_values_and_skip_bad_names() {
        assert_eq!(
            settings(&vars()),
            "cd; export CONDA_PREFIX='/Users/me/my env'; export PATH='/Users/me/my env/bin:/usr/bin'; unset PYTHONPATH"
        );
    }

    #[test]
    fn a_server_gets_the_settings_through_ssh() {
        let ssh = command(Some("lab"), &vars());
        assert_eq!(ssh[..3], ["ssh", "-t", "lab"]);
        assert!(ssh[3].starts_with("cd; export CONDA_PREFIX="));
        assert!(ssh[3].ends_with("exec \"${SHELL:-/bin/sh}\" -i"));
        assert!(typed_line(Some("lab"), &vars()).starts_with("'ssh' '-t' 'lab' 'cd; export"));
        assert!(typed_line(None, &vars()).ends_with("unset PYTHONPATH; clear"));
    }

    #[test]
    fn applescript_strings_escape_quotes_and_backslashes() {
        assert_eq!(applescript_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }
}
