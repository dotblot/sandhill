use super::{Show, ViewPhase, ViewRecovery, ViewSwitch, legacy_view_recovery};

#[test]
fn view_switch_legacy_recovery_requires_idle_resumable_current_service() {
    assert_eq!(
        legacy_view_recovery(Some("idle"), true, Some(true)),
        ViewRecovery::Resume
    );
    assert_eq!(
        legacy_view_recovery(Some("idle"), true, Some(false)),
        ViewRecovery::OlderService
    );
    assert_eq!(
        legacy_view_recovery(Some("idle"), true, None),
        ViewRecovery::UnknownService
    );
    assert_eq!(
        legacy_view_recovery(Some("idle"), false, Some(true)),
        ViewRecovery::NoResume
    );
    // A freshly started bridge on an old daemon is not upgraded by
    // restarting it, even when its terminal is not marked outdated.
    assert_eq!(
        legacy_view_recovery(Some("idle"), false, Some(false)),
        ViewRecovery::OlderService
    );
    for state in [Some("busy"), Some("waiting"), None] {
        for service in [Some(true), Some(false), None] {
            assert_eq!(
                legacy_view_recovery(state, true, service),
                ViewRecovery::Busy
            );
        }
    }
}

#[test]
fn view_switch_recovery_waits_for_new_bridge_before_handoff() {
    let mut switch = ViewSwitch {
        target: Show::Terminal,
        previous: Show::Chat,
        generation: 1,
        started: false,
        phase: ViewPhase::Checking,
    };
    assert!(switch.observe(Some("chat")).is_none());
    switch.phase = ViewPhase::Resuming;
    assert!(switch.observe(Some("chat")).is_none());
    switch.phase = ViewPhase::Switching;
    assert!(switch.observe(Some("chat")).is_none());
    assert!(switch.observe(Some("terminal")) == Some(Show::Terminal));
}

#[test]
fn view_switch_rejects_legacy_handoff_without_blocking_chat() {
    assert!(ViewSwitch::begin(None, Show::Terminal, 1).is_err());
    assert!(ViewSwitch::begin(None, Show::Chat, 1).unwrap().is_none());
    assert!(
        ViewSwitch::begin(Some("chat"), Show::Chat, 1)
            .unwrap()
            .is_none()
    );
    assert!(
        ViewSwitch::begin(Some("terminal"), Show::Terminal, 1)
            .unwrap()
            .is_none()
    );
    assert!(ViewSwitch::begin(Some("switching_terminal"), Show::Chat, 1).is_err());
}

#[test]
fn view_switch_completes_in_both_directions_and_on_bridge_failure() {
    for (previous, target, changing, ready) in [
        ("chat", Show::Terminal, "switching_terminal", "terminal"),
        ("terminal", Show::Chat, "switching_chat", "chat"),
    ] {
        let mut switch = ViewSwitch::begin(Some(previous), target, 1)
            .unwrap()
            .unwrap();
        assert!(switch.observe(None).is_none());
        // An unrelated old report must not acknowledge a new request.
        assert!(switch.observe(Some(previous)).is_none());
        assert!(switch.observe(Some(changing)).is_none());
        assert!(switch.observe(Some(ready)) == Some(target));
        // Failed handoffs report the original mode after switching_*.
        assert!(switch.observe(Some(previous)) == Some(switch.previous));
        // Already-running target: no intermediate report is required.
        let mut switch = ViewSwitch::begin(Some(previous), target, 2)
            .unwrap()
            .unwrap();
        assert!(switch.observe(Some(ready)) == Some(target));
    }
}
