//! The browser stays on this Mac; the selected device exchanges the code.
//! Huan, 2026-11-16; `docs/design/done/connector-sign-in-plan.md`.
use super::loopback::{Callback, Loopback};
use super::*;

impl ConnectorsView {
    pub(super) fn begin_sign_in(
        &mut self,
        target: SignInTarget,
        daemon: Rc<Daemon>,
        completion: Completion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_card(Card::Busy("Starting sign-in…".into()), window, cx);
        let generation = self.generation;
        cx.spawn_in(window, async move |this, cx| {
            let bound = cx.background_spawn(async { Loopback::bind() }).await;
            let (listener, callback) = match bound {
                Ok(bound) => bound,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        if generation == this.generation {
                            this.fail(error, cx);
                        }
                    })
                    .ok();
                    return;
                }
            };
            let redirect_uri = listener.redirect_uri.clone();
            let asked = this
                .update(cx, |this, _| {
                    if generation != this.generation {
                        return None;
                    }
                    this.sign_in = Some(listener);
                    Some(daemon.call::<ConnectorSignIn>(Call::BeginConnectorSignIn {
                        target,
                        redirect_uri,
                    }))
                })
                .ok()
                .flatten();
            let Some(asked) = asked else {
                return;
            };
            let result = asked.await;
            let active = this
                .update_in(cx, |this, window, cx| {
                    if generation != this.generation {
                        return false;
                    }
                    match result {
                        Ok(sign_in) => {
                            let expected = this
                                .sign_in
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Sign-in cancelled."))
                                .and_then(|listener| {
                                    listener.expect(sign_in.state, sign_in.service.clone())
                                });
                            if let Err(error) = expected {
                                this.fail(error, cx);
                                return false;
                            }
                            this.card = Some(Card::SignIn(sign_in.service));
                            this.focus.focus(window, cx);
                            cx.open_url(&sign_in.authorize_url);
                            cx.notify();
                            true
                        }
                        Err(error) => {
                            this.fail(error, cx);
                            false
                        }
                    }
                })
                .unwrap_or(false);
            if !active {
                return;
            }
            let result = callback
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result);
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                this.sign_in = None;
                match result {
                    Ok(callback) => this.finish_sign_in(callback, daemon, completion, cx),
                    Err(error) => this.fail(error, cx),
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_sign_in(
        &mut self,
        callback: Callback,
        daemon: Rc<Daemon>,
        completion: Completion,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.card = Some(Card::Busy("Connecting…".into()));
        let asked = daemon.call::<ConnectorInfo>(Call::FinishConnectorSignIn {
            state: callback.state,
            code: callback.code,
            error: callback.error,
        });
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => this.complete(completion, cx),
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn complete(&mut self, completion: Completion, cx: &mut Context<Self>) {
        self.card = None;
        match completion {
            Completion::Reload => self.load(cx),
            Completion::Copied { name, device } => {
                self.set_notice(name, format!("Copied to {device}"), false, cx)
            }
        }
    }
}
