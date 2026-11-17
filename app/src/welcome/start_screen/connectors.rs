//! Welcome → Connectors: device-owned MCP servers (Huan, 2026-11-16).
use super::manage::{card_shell, grey, item};
use super::*;
use gpui_kit::component::input::{Textarea, TextareaState};
use sandhill_proto::{
    ConnectorAdd, ConnectorInfo, ConnectorList, ConnectorSpec, SuggestedConnector,
};

mod card;
mod form;
mod menu;
mod page;

#[derive(Clone)]
struct Destination {
    host: String,
    daemon: Rc<Daemon>,
}

#[derive(Clone)]
enum Completion {
    Reload,
    Copied { name: String, device: String },
}

enum Card {
    Suggested(SuggestedConnector),
    Web,
    Command,
    Secret {
        add: ConnectorAdd,
        destination: Destination,
        name: String,
    },
    Busy(String),
    Tools(ConnectorInfo),
    Remove(ConnectorInfo),
    Error(String),
}

#[derive(Clone, PartialEq)]
enum Menu {
    Add,
    Connector(String),
    Copy(String),
}

pub(crate) struct ConnectorsView {
    daemon: Rc<Daemon>,
    host: String,
    destinations: Vec<Destination>,
    list: Option<ConnectorList>,
    problem: Option<String>,
    loading: bool,
    busy: bool,
    generation: u64,
    notice: Option<(String, String, bool)>,
    card: Option<Card>,
    /// Why the Web Address or Command card's last Add failed.
    form_error: Option<String>,
    menu: Option<(Menu, Point<Pixels>)>,
    focus: FocusHandle,
    form: form::Form,
    triggers: crate::shell::popup::PopupTriggers,
    _subscriptions: Vec<Subscription>,
}

impl ConnectorsView {
    fn new(daemon: Rc<Daemon>, host: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (form, subscriptions) = form::Form::new(window, cx);
        let mut this = Self {
            daemon,
            host,
            destinations: Vec::new(),
            list: None,
            problem: None,
            loading: false,
            busy: false,
            generation: 0,
            notice: None,
            card: None,
            form_error: None,
            menu: None,
            focus: cx.focus_handle(),
            form,
            triggers: Default::default(),
            _subscriptions: subscriptions,
        };
        this.load(cx);
        this
    }

    fn error_text(error: anyhow::Error) -> String {
        let text = format!("{error:#}");
        if text.contains("unknown variant") {
            "Update Sandhill on this device to manage its connectors.".into()
        } else {
            let text = text.strip_prefix("daemon error: ").unwrap_or(&text);
            let mut chars = text.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>() + "."
            })
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let asked = self.daemon.call::<ConnectorList>(Call::ListConnectors);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(list) => {
                        this.list = Some(list);
                        this.problem = None;
                    }
                    Err(error) => this.problem = Some(Self::error_text(error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_card(&mut self, card: Card, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        self.notice = None;
        self.menu = None;
        self.card = Some(card);
        self.form_error = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.generation += 1;
        self.card = None;
        self.menu = None;
        cx.notify();
    }

    fn fail(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        let text = Self::error_text(error);
        if text.starts_with("Update Sandhill") {
            self.problem = Some(text.clone());
        }
        self.card = Some(Card::Error(text));
        self.busy = false;
        cx.notify();
    }

    fn add(
        &mut self,
        add: ConnectorAdd,
        daemon: Rc<Daemon>,
        completion: Completion,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        // A form that fails comes back as typed, with the reason under it.
        let back = match self.card.take() {
            Some(card @ (Card::Web | Card::Command)) => Some(card),
            _ => None,
        };
        self.form_error = None;
        self.card = Some(Card::Busy("Connecting…".into()));
        let asked = daemon.call::<ConnectorInfo>(Call::AddConnector { add });
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.card = None;
                        match completion {
                            Completion::Reload => this.load(cx),
                            Completion::Copied { name, device } => {
                                this.set_notice(name, format!("Copied to {device}"), false, cx)
                            }
                        }
                    }
                    Err(error) => match back {
                        Some(card) => {
                            this.form_error = Some(Self::error_text(error));
                            this.card = Some(card);
                        }
                        None => this.fail(error, cx),
                    },
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn mutate(&mut self, call: Call, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.notice = None;
        self.menu = None;
        let asked = self.daemon.call::<serde_json::Value>(call);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.card = None;
                        this.load(cx);
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn set_notice(&mut self, name: String, text: String, error: bool, cx: &mut Context<Self>) {
        self.notice = Some((name.clone(), text.clone(), error));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(6))
                .await;
            this.update(cx, |this, cx| {
                if this
                    .notice
                    .as_ref()
                    .is_some_and(|(n, t, _)| n == &name && t == &text)
                {
                    this.notice = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn test(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.menu = None;
        self.busy = true;
        self.notice = Some((name.clone(), "Testing…".into(), false));
        let asked = self
            .daemon
            .call::<ConnectorInfo>(Call::TestConnector { name: name.clone() });
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(info) => {
                        let error = info.error.is_some();
                        let text = info
                            .error
                            .clone()
                            .unwrap_or_else(|| format!("Working · {} tools", info.tools.len()));
                        if let Some(list) = &mut this.list
                            && let Some(old) = list.added.iter_mut().find(|c| c.name == name)
                        {
                            *old = info;
                        }
                        this.set_notice(name, text, error, cx);
                    }
                    Err(error) => {
                        let text = Self::error_text(error);
                        if text.starts_with("Update Sandhill") {
                            this.problem = Some(text.clone());
                        }
                        this.set_notice(name, text, true, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn copy(
        &mut self,
        name: String,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_card(Card::Busy("Reading connector…".into()), window, cx);
        let generation = self.generation;
        let asked = self
            .daemon
            .call::<ConnectorAdd>(Call::ExportConnector { name: name.clone() });
        cx.spawn_in(window, async move |this, cx| {
            let result = asked.await;
            this.update_in(cx, |this, window, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(add) => {
                        if matches!(
                            &add,
                            ConnectorAdd::Custom {
                                spec: ConnectorSpec::Http {
                                    header: Some(_),
                                    ..
                                },
                                ..
                            }
                        ) {
                            this.card = Some(Card::Secret {
                                add,
                                destination,
                                name,
                            });
                            this.form.secret.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                                input.focus(window, cx);
                            });
                        } else {
                            let completion = Completion::Copied {
                                name,
                                device: host_label(&destination.host),
                            };
                            this.add(add, destination.daemon, completion, cx);
                        }
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
