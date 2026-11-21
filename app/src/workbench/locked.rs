//! The hint under an error the lock caused: while Sandhill's own R and
//! Python are chosen, what Sandhill starts can't write into the user's
//! other R, Python, conda and Homebrew.

use gpui_kit::component::h_flex;
use sandhill_proto::{EnvMode, InstallPrefs};

use super::*;
use crate::look::{self, Palette};
use crate::welcome::start_screen::StartMode;

/// The daemon's plain sentence for a write the lock refused.
pub(crate) use sandhill_daemon::quick::install_failure::LOCKED_MESSAGE as LOCKED;

/// The words a refused write leaves in an error.
pub(crate) fn says_locked(text: &str) -> bool {
    text.contains("Operation not permitted") || text.contains("Read-only file system")
}

/// A stream's or a traceback's text: one string, or a list of them.
fn any_text(value: &Value) -> bool {
    match value {
        Value::String(text) => says_locked(text),
        Value::Array(lines) => lines.iter().filter_map(Value::as_str).any(says_locked),
        _ => false,
    }
}

impl Run {
    /// It ended with an error, or wrote one, that a refused write causes.
    fn locked_error(&self) -> bool {
        self.finished
            && self
                .outputs
                .iter()
                .any(|out| match out["output_type"].as_str() {
                    Some("error") => {
                        any_text(&out["ename"])
                            || any_text(&out["evalue"])
                            || any_text(&out["traceback"])
                    }
                    Some("stream") => out["name"] == "stderr" && any_text(&out["text"]),
                    _ => false,
                })
    }
}

impl Workbench {
    /// Reads whether Sandhill's own R and Python are chosen on this
    /// device: when they are, the lock's errors get their hint.
    pub(super) fn read_env_mode(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let prefs = daemon.call::<InstallPrefs>(Call::InstallPrefs);
        cx.spawn(async move |this, cx| {
            let own = prefs
                .await
                .is_ok_and(|prefs| prefs.mode == Some(EnvMode::Own));
            this.update(cx, |this, cx| {
                if this.own_mode != own {
                    this.own_mode = own;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The hint under a cell's or the Console's error the lock caused.
    pub(super) fn render_lock_hint(&self, run: &Run, cx: &mut Context<Self>) -> Option<Div> {
        (self.own_mode && run.locked_error()).then(|| self.lock_hint(cx))
    }

    /// The hint under a job's log when its output shows a write the lock
    /// refused.
    pub(crate) fn job_lock_hint(&self, output: &str, cx: &mut Context<Self>) -> Option<Div> {
        (self.own_mode && says_locked(output)).then(|| self.lock_hint(cx))
    }

    fn lock_hint(&self, cx: &mut Context<Self>) -> Div {
        let p = *Palette::get(cx);
        let host = self.host();
        h_flex()
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .gap(px(look::GAP_8))
            .text_size(px(look::small()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_color(p.muted)
                    .child(LOCKED),
            )
            .child(
                look::button(
                    "lock-open-r-and-python",
                    "Open R and Python",
                    None,
                    look::ButtonSize::Row,
                )
                .on_click(move |_, _, cx| {
                    let host = host.clone();
                    cx.defer(move |cx| {
                        crate::shell::app_window::show_welcome(Some(StartMode::Ready(host)), cx)
                    });
                }),
            )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Run;

    fn run(outputs: Vec<serde_json::Value>) -> Run {
        Run {
            finished: true,
            outputs,
            ..Run::default()
        }
    }

    #[test]
    fn refused_writes_are_found_in_errors_and_warnings() {
        assert!(
            run(vec![json!({
                "output_type": "error",
                "ename": "PermissionError",
                "evalue": "[Errno 1] Operation not permitted: '/opt/homebrew/lib/python3.13'",
                "traceback": [],
            })])
            .locked_error()
        );
        assert!(
            run(vec![json!({
                "output_type": "stream",
                "name": "stderr",
                "text": ["Warning: cannot create file '/x/y', reason 'Read-only file system'\n"],
            })])
            .locked_error()
        );
        assert!(
            !run(vec![json!({
                "output_type": "stream",
                "name": "stdout",
                "text": "Operation not permitted is just text here",
            })])
            .locked_error()
        );
        let mut running = run(vec![json!({
            "output_type": "error",
            "ename": "OSError",
            "evalue": "Operation not permitted",
        })]);
        running.finished = false;
        assert!(!running.locked_error());
    }
}
