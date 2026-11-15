//! A removed installation must not be resurrected by a stale client/service.
mod common;

use std::process::Command;

#[test]
fn tombstone_blocks_both_service_and_autostart_without_recreating_state() {
    let tmp = common::tempdir().unwrap();
    let state = tmp.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let marker = state.join("uninstalled");
    std::fs::write(&marker, "explicit removal\n").unwrap();
    for args in [vec!["serve"], vec!["attach", "--start"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_sandhilld"))
            .arg("--state-dir")
            .arg(&state)
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Automatic startup is disabled"));
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap(),
            "explicit removal\n"
        );
        assert!(!state.join("journal.sqlite").exists());
        assert!(!state.join("sandhilld.log").exists());
    }
    std::fs::remove_file(marker).unwrap();
    // A valid fresh install still exposes the normal CLI without startup.
    let output = Command::new(env!("CARGO_BIN_EXE_sandhilld"))
        .arg("--state-dir")
        .arg(&state)
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("sandhilld "));
}
