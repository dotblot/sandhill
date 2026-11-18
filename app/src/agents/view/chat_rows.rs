use super::*;

impl Agents {
    pub(super) fn render_chat(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let chat = self.chat(t);
        let agent = kind_of(&t.kind).map_or("The agent", |k| k.short);
        let mut items: Vec<AnyElement> = chat
            .items
            .iter()
            .map(|item| self.render_item(item, agent, cx))
            .collect();
        if items.is_empty() {
            items.push(
                div()
                    .text_color(p.faint)
                    .child(if t.running {
                        format!("{agent} is ready. Type a message below.")
                    } else {
                        "This conversation has ended.".to_string()
                    })
                    .into_any_element(),
            );
        }
        if (t.running || crate::agents::composer::can_resume(t))
            && matches!(t.kind.as_str(), "claude" | "codex" | "antigravity")
        {
            let account_id = t.options.account.as_deref().unwrap_or(&t.kind);
            let limited = self
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .is_some_and(crate::agents::account_at_limit)
                || (!self.working(t) && chat::latest_usage_limit(&chat));
            if limited {
                let mut alternatives = v_flex().gap(px(6.)).text_size(px(look::SMALL));
                for account in self.accounts.iter().filter(|account| {
                    account.kind == t.kind
                        && account.id != account_id
                        && account.signed_in != Some(false)
                        && !crate::agents::account_at_limit(account)
                }) {
                    let account = account.clone();
                    alternatives = alternatives.child(
                        look::button(
                            SharedString::from(format!("continue-{}", account.id)),
                            SharedString::from(format!("Continue with {}", account.name)),
                            None,
                            ButtonSize::Card,
                        )
                        .primary()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.switch_account(account.clone(), window, cx);
                            },
                        )),
                    );
                }
                items.push(alternatives.into_any_element());
            }
        }
        if !t.running {
            let resumable = crate::agents::composer::can_resume(t);
            items.push(
                v_flex()
                    .self_center()
                    .items_center()
                    .gap(px(6.))
                    .pt(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(if t.exit.as_ref().is_some_and(|e| e.idle) {
                        format!("{agent} stopped after 30 minutes idle. Send a message to pick this conversation up again.")
                    } else if resumable {
                        format!("{agent} has stopped. Send a message to pick this conversation up again.")
                    } else {
                        format!("{agent} has stopped.")
                    })
                    .when(!resumable, |d| {
                        d.child(
                            look::button("chat-start-again", "Start a new conversation like it", None, ButtonSize::Card)
                                .on_click(cx.listener(|this, _, window, cx| this.start_again(window, cx))),
                        )
                    })
                    .into_any_element(),
            );
        }
        look::PaneScroll::new(
            "chat-scroll",
            look::STRIP,
            0.,
            div().id("chat").child(
                v_flex()
                    .px(px(14.))
                    .pt(px(12.))
                    .pb(px(10.))
                    .gap(px(8.))
                    .children(items),
            ),
        )
        .handle(&self.chat_scroll)
        .into_any_element()
    }

    /// Claude Code's question, the first time it works in a folder,
    /// whether the user trusts the files there: asked here, as its terminal
    /// is out of sight, and a message typed at it would end it.
    pub(super) fn render_trust(&self, t: &TerminalInfo, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let folder = t.cwd.clone();
        let button = |id, label, primary| {
            look::button(id, label, None, ButtonSize::Card).when(primary, |button| {
                button.primary().font_weight(FontWeight::SEMIBOLD)
            })
        };
        let yes = t.clone();
        let no = t.clone();
        v_flex()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.accent.opacity(0.55))
            .bg(p.accent.opacity(0.06))
            .child(
                h_flex()
                    .items_start()
                    .gap(px(8.))
                    .child(icon("ask").size(px(18.)).text_color(p.accent))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Do you trust the files in this folder?"),
                            )
                            .child(
                                div()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.muted)
                                    .child(format!(
                                        "Claude Code asks once for each folder before it reads, edits or runs files in it: {folder}"
                                    )),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap(px(6.))
                    .child(button("trust-yes", "Yes, I trust this folder", true).on_click(
                        cx.listener(move |this, _, _, cx| this.answer_trust(&yes, true, cx)),
                    ))
                    .child(button("trust-no", "No, end the conversation", false).on_click(
                        cx.listener(move |this, _, _, cx| this.answer_trust(&no, false, cx)),
                    )),
            )
            .into_any_element()
    }

    fn render_item(&self, item: &Item, agent: &str, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        match item {
            // Only what the user added with @ and #: messages carry no
            // document of their own, and older ones' recorded `place.file`
            // isn't shown either.
            Item::User { key, text, place } => {
                let refs: Vec<String> = place["refs"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.as_str().map(str::to_string))
                    .collect();
                v_flex()
                    .items_end()
                    .gap(px(3.))
                    .mt(px(6.))
                    .child(
                        div()
                            .max_w(relative(0.88))
                            .px(px(11.))
                            .py(px(8.))
                            .rounded(px(RADIUS))
                            .bg(p.ink(8.))
                            .line_height(rems(1.45))
                            .child(crate::editor::output_text::OutputText::new(
                                SharedString::from(format!("user-{key}")),
                                text.clone(),
                            ))
                            .on_mouse_down(MouseButton::Right, self.text_menu_opener(text, cx)),
                    )
                    .when(!refs.is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(look::SMALL))
                                .text_color(p.faint)
                                .child(refs.join(" · ")),
                        )
                    })
                    .into_any_element()
            }
            Item::Reply { key, text } => {
                let agents = cx.entity().downgrade();
                let workbench = self.workbench.clone();
                div()
                    .line_height(rems(1.5))
                    .on_mouse_down(MouseButton::Right, self.text_menu_opener(text, cx))
                    .child(
                        TextView::markdown(SharedString::from(format!("md-{key}")), text.clone())
                            .selectable(true)
                            // New words fade in one after another as the reply streams.
                            .stream_fade(true)
                            .code_block_actions(move |block, _, cx| {
                                code_block_buttons(block, agents.clone(), cx)
                            })
                            .table_actions(move |table, _, _| {
                                crate::agents::table_actions::table_buttons(
                                    table,
                                    workbench.clone(),
                                )
                            })
                            .style(
                                TextViewStyle::default()
                                    .paragraph_gap(rems(0.4))
                                    .heading_font_size(|_, _| px(look::CHROME)),
                            ),
                    )
                    .into_any_element()
            }
            Item::Step { key, step } => self.render_step(key, step, agent, cx),
            Item::Ask { ask, .. } => self.render_ask(ask, agent, cx),
            Item::Footer {
                key,
                at_ms,
                ms,
                next,
                ..
            } => {
                let mut parts = Vec::new();
                if let Some(ms) = ms {
                    parts.push(duration_label(*ms));
                }
                if let Some(at) = at_ms {
                    parts.push(time_label(*at));
                }
                let line = h_flex()
                    .gap(px(6.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(icon("check").size(px(12.)))
                    .child(parts.join(" · "));
                // What the agent suggests next: a click puts it in the box,
                // to send or change (Huan, 2026-11-16).
                let chips = next.iter().enumerate().map(|(n, text)| {
                    let fill = text.clone();
                    h_flex()
                        .id(SharedString::from(format!("next-{key}-{n}")))
                        .flex_none()
                        .max_w_full()
                        .gap(px(5.))
                        .h(px(26.))
                        .px(px(10.))
                        .rounded_full()
                        .border(px(0.5))
                        .border_color(p.accent.opacity(0.4))
                        .bg(p.accent_soft)
                        .text_size(px(look::SMALL))
                        .text_color(p.accent)
                        .cursor_pointer()
                        .hover(move |d| d.border_color(p.accent))
                        .child(icon("sparkle").size(px(12.)))
                        .child(div().min_w_0().truncate().child(text.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.fill_box(fill.clone(), window, cx)
                        }))
                });
                v_flex()
                    .gap(px(6.))
                    .mb(px(4.))
                    .child(line)
                    .when(!next.is_empty(), |d| {
                        d.child(h_flex().flex_wrap().gap(px(6.)).children(chips))
                    })
                    .into_any_element()
            }
            Item::Note { text, .. } => div()
                .self_center()
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(text.clone())
                .into_any_element(),
            Item::Account { name, .. } => h_flex()
                .gap(px(8.))
                .my(px(6.))
                .text_size(px(look::SMALL))
                .text_color(p.muted)
                .child(div().flex_1().h(px(1.)).bg(p.line))
                .child(format!("Switched to {name}"))
                .child(div().flex_1().h(px(1.)).bg(p.line))
                .into_any_element(),
            Item::Error { text, .. } => h_flex()
                .items_start()
                .gap(px(7.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(RADIUS))
                .bg(p.err_soft)
                .child(icon("warning").size(px(15.)).text_color(p.err))
                .child(div().flex_1().child(text.clone()))
                .into_any_element(),
            Item::Thinking { .. } => h_flex()
                .gap(px(7.))
                .text_size(px(look::SMALL))
                .text_color(p.faint)
                .child(icon("think").size(px(14.)))
                .child(format!("{agent} is working…"))
                .into_any_element(),
        }
    }

    /// Opens the right-click menu over a message whose text is `whole`.
    fn text_menu_opener(
        &self,
        whole: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        let whole = whole.to_string();
        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
            this.text_menu = Some(crate::shell::text_menu::TextMenu::open(
                event.position,
                whole.clone(),
                window,
                cx,
            ));
            cx.stop_propagation();
            cx.notify();
        })
    }

    /// The conversation's right-click menu: Copy, and Send to Message Box,
    /// which puts the text in the box at the caret (Huan, 2026-11-16).
    pub(crate) fn render_text_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::shell::text_menu::{copy, render, row};
        let menu = self.text_menu.as_ref()?;
        let p = *Palette::get(cx);
        let selected = menu.has_selection();
        let rows = vec![
            row(
                "chat-copy",
                if selected { "Copy" } else { "Copy Message" },
                if selected { "⌘C" } else { "" },
                &p,
                cx,
                |this, _, cx| {
                    if let Some(menu) = this.text_menu.take() {
                        copy(menu.text(), cx);
                    }
                },
            ),
            row(
                "chat-to-box",
                "Send to Message Box",
                "",
                &p,
                cx,
                |this, window, cx| {
                    if let Some(menu) = this.text_menu.take() {
                        let text = menu.text();
                        this.composer.update(cx, |c, cx| {
                            // After other text, on a line of its own.
                            let value = c.value();
                            let before = &value[..c.cursor().min(value.len())];
                            let text = if before.trim_end_matches(' ').is_empty()
                                || before.ends_with('\n')
                            {
                                text
                            } else {
                                format!("\n{text}")
                            };
                            c.insert(text, window, cx);
                            c.focus(window, cx);
                        });
                    }
                },
            ),
        ];
        Some(render(
            "chat-text-menu",
            menu.at,
            rows,
            &p,
            cx,
            |this: &mut Self| {
                this.text_menu = None;
            },
        ))
    }

    /// A step's row, which opens to its card; a request waiting on it shows
    /// under it.
    fn render_step(
        &self,
        key: &str,
        step: &Step,
        agent: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = *Palette::get(cx);
        let view =
            chat::step_details::step_view(&step.tool, &step.input, step.output.as_ref(), None);
        let open = self.open_steps.contains(key);
        let (icon_name, color) = match step.status {
            StepStatus::Fail => ("step.failed", p.err),
            StepStatus::Deny | StepStatus::Cancel => (tool_icon(&view.name), p.faint),
            StepStatus::Wait => ("ask", p.accent),
            StepStatus::Run => (tool_icon(&view.name), p.busy),
            StepStatus::Done => (tool_icon(&view.name), p.muted),
        };
        let detail = match (&step.error, step.status) {
            (Some(err), _) => Some(err.lines().next().unwrap_or_default().to_string()),
            (None, StepStatus::Deny) => Some("denied".into()),
            (None, StepStatus::Cancel) => Some("stopped".into()),
            _ => view.detail.clone(),
        };
        let hover = p.ink(6.);
        let key_owned = key.to_string();
        let row = h_flex()
            .id(SharedString::from(format!("step-{key}")))
            .gap(px(7.))
            .min_h(px(26.))
            .px(px(6.))
            .rounded(px(RADIUS))
            .cursor_pointer()
            .text_size(px(look::SMALL))
            .hover(move |d| d.bg(hover))
            .child(icon(icon_name).size(px(15.)).text_color(color))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(6.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_none()
                            .font_family(look::MONO)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(view.name.clone()),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(p.muted)
                            .when(step.status == StepStatus::Deny, |d| d.line_through())
                            .child(view.desc.clone()),
                    ),
            )
            .when_some(detail, |d, detail| {
                d.child(
                    div()
                        .flex_none()
                        .max_w(px(200.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(if step.error.is_some() { p.err } else { p.faint })
                        .child(detail),
                )
            })
            .child(
                icon(if open {
                    "chevron.down"
                } else {
                    "chevron.right"
                })
                .size(px(11.))
                .text_color(p.faint),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_step(&key_owned, cx)));
        let waiting = step
            .ask
            .as_ref()
            .filter(|a| a.answer.is_none() && step.status == StepStatus::Wait);
        v_flex()
            .mx(px(-6.))
            .rounded(px(RADIUS))
            .when(open, |d| d.bg(p.ink(6.)))
            .child(row)
            .when(open, |d| {
                // Only a file the step got to, in the project: a failed
                // read or a file elsewhere can't be opened.
                let file = view.file.clone().filter(|f| {
                    step.status == StepStatus::Done
                        && crate::editor::notebook::format_of(f).is_some()
                        && self.project_file(f).is_some()
                });
                d.child(
                    v_flex()
                        .gap(px(4.))
                        .pl(px(29.))
                        .pr(px(6.))
                        .pb(px(8.))
                        .child(step_card(&view, key, cx.entity().downgrade(), &p, cx))
                        .when_some(file, |d, file| {
                            let name = file.rsplit('/').next().unwrap_or(&file).to_string();
                            d.child(
                                div()
                                    .id(SharedString::from(format!("open-{key}")))
                                    .text_size(px(look::SMALL))
                                    .text_color(p.accent)
                                    .cursor_pointer()
                                    .hover(|d| d.underline())
                                    .child(format!("Open {name}"))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.open_path(&file, cx)
                                        }),
                                    ),
                            )
                        }),
                )
            })
            .when_some(waiting.cloned(), |d, ask| {
                d.child(
                    div()
                        .px(px(6.))
                        .pb(px(4.))
                        .child(self.render_ask(&ask, agent, cx)),
                )
            })
            .into_any_element()
    }
}

/// A step's icon by what it does.
fn tool_icon(name: &str) -> &'static str {
    match name {
        "bash" => "terminal",
        "read" | "grep" | "glob" | "tools" => "search",
        "edit" | "write" | "edit_notebook" => "edit.compose",
        "run_code" => "run",
        "add_cell" | "edit_cell" => "add",
        "web_search" | "web_fetch" => "globe",
        "thinking" => "think",
        "todo" => "check",
        "plan" => "plan",
        "question" => "help",
        "list_variables" | "get_outputs" | "view_table" => "table",
        "list_sessions" | "get_console" => "sessions",
        "get_plots" => "zoom",
        "get_help" => "help",
        "list_packages" => "package",
        "delete_cell" => "delete",
        "get_focus" => "context.show",
        "list_jobs" | "read_job_log" => "cpu",
        "task" | "agent" => "agent",
        _ => "skill",
    }
}

/// Copy and ⋯ in the corner of a reply's code block.
fn code_block_buttons(
    block: &gpui_kit::base::text::CodeBlock,
    agents: WeakEntity<Agents>,
    cx: &mut App,
) -> Div {
    use std::hash::{Hash as _, Hasher as _};
    let code = block.code();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hash);
    crate::agents::code_actions::code_buttons(
        ("chat-code", hash.finish()),
        code,
        block.lang(),
        agents,
        Palette::get(cx),
    )
}

/// "3.1 s", "2 min 5 s".
fn duration_label(ms: i64) -> String {
    let s = ms as f64 / 1000.;
    if s < 60. {
        format!("{s:.1} s")
    } else {
        format!("{} min {} s", (s / 60.).floor(), (s % 60.).round())
    }
}
