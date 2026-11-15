use super::*;

impl Agents {
    /// Subscribes to a conversation's output and chat, from the start.
    pub(super) fn follow(&mut self, terminal: &TerminalInfo, cx: &mut Context<Self>) {
        let id = terminal.id.clone();
        self.chats.insert(id.clone(), Vec::new());
        let input = self.daemon.ordered({
            let id = id.clone();
            move |data| Call::TerminalInput {
                terminal: id.clone(),
                data,
            }
        });
        let resize = {
            let daemon = self.daemon.clone();
            let id = id.clone();
            Box::new(move |cols, rows| {
                daemon.fire(Call::ResizeTerminal {
                    terminal: id.clone(),
                    cols,
                    rows,
                })
            })
        };
        let window = self.window;
        let view = cx.new(|cx| TerminalView::new(id.clone(), input, resize, window, cx));
        if terminal.kind == "claude" {
            let id = id.clone();
            cx.observe(&view, move |this, view, cx| {
                let asks = crate::agents::composer::trust_question(&view.read(cx).screen_text(cx))
                    .is_some();
                let changed = if asks {
                    this.trust_asked.insert(id.clone())
                } else {
                    this.trust_asked.remove(&id)
                };
                if changed {
                    cx.notify();
                }
            })
            .detach();
        }
        if terminal_only(&terminal.kind) {
            let id = id.clone();
            cx.observe(&view, move |this, view, cx| {
                if this.typing.contains_key(&id)
                    && takes_message(&view.read(cx).screen_text(cx))
                    && let Some(text) = this.typing.remove(&id)
                {
                    this.type_message(&id, "posit", &text, cx);
                }
            })
            .detach();
        }
        if !terminal.running {
            view.update(cx, |view, cx| view.exited("ended".into(), cx));
        }
        self.views.insert(id.clone(), view);
        self.subscribe(id, None, Some(0), cx);
    }

    /// Follows a conversation from `from` in its output and `chat_from`
    /// in its chat.
    fn subscribe(
        &mut self,
        id: String,
        from: Option<u64>,
        chat_from: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let subscribed = self
            .daemon
            .call::<TerminalSubscribed>(Call::SubscribeTerminal {
                terminal: id,
                from,
                chat_from,
            });
        cx.spawn(async move |this, cx| {
            if let Err(err) = subscribed.await {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Following a conversation failed: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Follows a restarted conversation's new program from where the old
    /// one got to.
    pub(crate) fn follow_again(&mut self, id: &str, cx: &mut Context<Self>) {
        let from = self.output_end.get(id).copied();
        let chat_from = self
            .chats
            .get(id)
            .and_then(|c| c.last())
            .map_or(0, |e| e.seq + 1);
        self.subscribe(id.to_string(), from, Some(chat_from), cx);
    }

    /// Whether this pane follows the terminal.
    pub fn owns(&self, terminal: &str) -> bool {
        self.chats.contains_key(terminal)
    }

    /// Output, chat entries and reports from a conversation's terminal.
    pub fn on_terminal_event(&mut self, event: TerminalEvent, cx: &mut Context<Self>) {
        let id = event.terminal.clone();
        if event
            .agent
            .as_ref()
            .is_some_and(|agent| agent["status"]["rate_limits"].is_object())
        {
            self.refresh_accounts(cx);
        }
        if event
            .chat
            .as_ref()
            .is_some_and(|entry| entry.event["kind"] == "account")
        {
            self.refresh_accounts(cx);
        }
        if let Some(data) = &event.data
            && let Some(view) = self.views.get(&id)
        {
            let end = self.output_end.entry(id.clone()).or_default();
            let start = event.offset.unwrap_or(*end);
            let len = decoded_len(data);
            // A new subscription can repeat output already shown.
            if start + len > *end || event.offset.is_none() {
                *end = start + len;
                view.update(cx, |view, cx| view.feed(data, event.replay, cx));
            }
        }
        if let Some(entry) = event.chat {
            let entries = self.chats.entry(id.clone()).or_default();
            // A replay after a reconnect repeats what is already here.
            if entries.last().is_none_or(|last| entry.seq > last.seq) {
                if let Some(answered) = entry.event["id"]
                    .as_str()
                    .filter(|_| entry.event["kind"] == "answered")
                {
                    self.sent.remove(answered);
                }
                entries.push(Entry {
                    seq: entry.seq,
                    event: entry.event,
                });
                if self.selected.as_deref() == Some(&id) {
                    self.chat_scroll.scroll_to_bottom();
                }
            }
        }
        if let Some(agent) = &event.agent
            && agent.get("state").is_some_and(|s| s != "waiting")
            && self.held.as_ref().is_some_and(|(t, _)| *t == id)
            && let Some((_, command)) = self.held.take()
        {
            self.type_command(&id, command, cx);
        }
        if let Some(agent) = event.agent
            && let Some(t) = self.terminals.iter_mut().find(|t| t.id == id)
        {
            if agent["state"] == "idle" {
                self.stopped.remove(&id);
            }

            let previous_view = t
                .agent
                .as_ref()
                .and_then(|a| a["view_mode"].as_str())
                .map(str::to_owned);
            let completed = self
                .view_switches
                .get_mut(&id)
                .and_then(|switch| switch.observe(agent["view_mode"].as_str()));
            if completed.is_some() {
                self.view_switches.remove(&id);
                self.problem = None;
            }
            // A failed command line says why once, in the notice the user
            // dismisses, in the report that ends its switch back to chat.
            if matches!(
                previous_view.as_deref(),
                Some("switching_terminal" | "switching_chat")
            ) && agent["view_mode"] == "chat"
                && let Some(error) = agent["view_error"].as_str()
            {
                self.problem = Some(error.to_owned());
            }
            // Reports carry what changed; keep the rest.
            let mut merged = t.agent.take().unwrap_or(json!({}));
            if let (Some(old), Some(new)) = (merged.as_object_mut(), agent.as_object()) {
                for (k, v) in new {
                    old.insert(k.clone(), v.clone());
                }
            } else {
                merged = agent;
            }
            t.agent = Some(merged);
            if self.selected.as_deref() == Some(&id) {
                self.show = provider_show(&t.kind, self.show);
            }
            if self.selected.as_deref() == Some(&id)
                && matches!(self.show, Show::Chat | Show::Terminal)
                && drives_chat(&t.kind)
                && (completed.is_some()
                    || t.agent.as_ref().and_then(|a| a["view_mode"].as_str())
                        != previous_view.as_deref())
            {
                match t.agent.as_ref().and_then(|a| a["view_mode"].as_str()) {
                    Some("chat") => {
                        self.show = Show::Chat;
                        let composer = self.composer.clone();
                        let _ = self.window.update(cx, move |_, window, cx| {
                            composer.update(cx, |composer, cx| composer.focus(window, cx));
                        });
                    }
                    Some("terminal") => {
                        self.show = Show::Terminal;
                        if let Some(view) = self.views.get(&id) {
                            let focus = view.read(cx).focus_handle();
                            let _ = self.window.update(cx, move |_, window, cx| {
                                window.focus(&focus, cx);
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        self.continue_resumed_view(&id, cx);
        if let Some(exit) = event.exit
            && let Some(t) = self.terminals.iter_mut().find(|t| t.id == id)
            && exit.ended_ms >= t.started_ms
        {
            t.running = false;
            t.exit = Some(exit);
            if self
                .view_switches
                .get(&id)
                .is_some_and(|s| s.phase != ViewPhase::Resuming || self.starting.is_none())
                && let Some(switch) = self.view_switches.remove(&id)
            {
                if self.selected.as_deref() == Some(&id)
                    && matches!(self.show, Show::Chat | Show::Terminal)
                {
                    self.show = switch.previous;
                }
                self.problem = Some(
                    "The agent ended while changing modes. Resume the conversation to continue."
                        .into(),
                );
            }
            if let Some(view) = self.views.get(&id) {
                view.update(cx, |view, cx| view.exited("ended".into(), cx));
            }
        }
        cx.notify();
    }
}
