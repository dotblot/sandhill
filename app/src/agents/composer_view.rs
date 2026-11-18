//! Drawing the message box, in the look of the Tauri app's
//! `AgentComposer.tsx` (drafts 18 and 30–35), text 2px larger: chips for
//! what was added with @ and #, the text and a round send (or stop)
//! button; under it web search, following edits and the mode on the left,
//! the context used and the model with its effort on the right. Menus open
//! above the box.

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{
    Backspace, Editor, Enter, Escape, IndentInline, MoveDown, MoveUp, Paste,
};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::TerminalInfo;
use serde_json::Value;

use crate::RunCell;
use crate::agents::composer::{
    self, ModelChoice, effort_name, ref_kind, ref_name, short_model, tokens,
};
use crate::agents::view::provider;
use crate::agents::{Agents, KINDS, Menu, Show, kind_of, terminal_only};
use crate::look::{self, Palette, icon, picture};

const LINE: f32 = 20.;
/// The message box grows to this many lines, then scrolls.
const LINES: usize = 8;
const MENU_WIDTH: f32 = 280.;
const MODEL_MENU_WIDTH: f32 = 380.;
const MODEL_MENU_HEIGHT: f32 = 360.;
const MODEL_PROVIDER_WIDTH: f32 = 32.;
const MODEL_COLUMN_GAP: f32 = look::GAP_6;
// Popup border + inset, left provider rail + gap, effort section inset.
const EFFORT_WIDTH: f32 = MODEL_MENU_WIDTH
    - 2. * (1. + look::GAP_6 + look::GAP_8)
    - MODEL_PROVIDER_WIDTH
    - MODEL_COLUMN_GAP;
/// The model menu's agents, turned on, alphabetical.
fn model_providers() -> Vec<&'static crate::agents::Kind> {
    let mut providers: Vec<_> = KINDS
        .iter()
        .filter(|kind| !crate::agents::turned_off(kind.kind))
        .collect();
    providers.sort_by_key(|kind| kind.name.to_lowercase());
    providers
}

/// A menu's frame: the Tauri app's `.apop`.
pub(crate) fn popup(p: &Palette) -> Div {
    v_flex()
        .occlude()
        .on_scroll(|_, _, cx| cx.stop_propagation())
        .gap(px(look::GAP_2))
        .p(px(look::GAP_6))
        .bg(p.bg)
        .border_1()
        .border_color(p.glass_edge)
        .rounded(px(look::RADIUS))
        .text_size(px(look::SMALL))
        .text_color(p.text)
        .shadow(vec![BoxShadow {
            color: p.shadow.opacity(0.35),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
            inset: false,
        }])
}

fn heading(text: impl Into<SharedString>, p: &Palette) -> Div {
    div()
        .px(px(look::GAP_8))
        .pt(px(look::GAP_4))
        .pb(px(look::GAP_2))
        .text_size(px(look::SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.faint)
        .child(text.into())
}

/// A ref's icon: a file's, else a cell's, a plot's, a command's, a skill's
/// or a table.
pub(crate) fn ref_icon(item: &Value) -> AnyElement {
    match ref_kind(item) {
        "file" | "attach" => {
            let path = item["path"].as_str().unwrap_or(ref_name(item));
            picture(&look::file_icon(path))
                .size(px(look::ICON))
                .into_any_element()
        }
        "chunk" => icon("outline").size(px(look::ICON)).into_any_element(),
        "selection" => icon("edit.copy").size(px(look::ICON)).into_any_element(),
        "plot" => icon("zoom").size(px(look::ICON)).into_any_element(),
        "command" => icon("terminal").size(px(look::ICON)).into_any_element(),
        "skill" | "guide" => icon("skill").size(px(look::ICON)).into_any_element(),
        "compact" => icon("compact").size(px(look::ICON)).into_any_element(),
        _ => icon("table").size(px(look::ICON)).into_any_element(),
    }
}

impl Agents {
    pub(crate) fn render_composer(
        &self,
        current: Option<&TerminalInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = *Palette::get(cx);
        let editor_height = self.composer_editor_height(cx);
        let t = current.filter(|_| self.show != Show::New);
        let working = t.is_some_and(|t| self.working(t));
        let typed = !self.composer.read(cx).value().trim().is_empty()
            || self.refs.iter().any(composer::is_slash);
        let stopping = working && !typed;
        let kind = self.box_kind().to_string();
        let focused = self
            .composer
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx);
        let edge = if focused {
            p.accent.opacity(0.6)
        } else {
            p.ink(if p.dark { 26. } else { 22. })
        };
        let mode = self.box_mode();
        let effort = self
            .box_effort()
            .unwrap_or_else(|| self.models_of(&kind).recommended);
        let status = t
            .and_then(|t| t.agent.as_ref())
            .map(|a| a["status"].clone());
        // How full the conversation is: the ring and "42%" (Huan,
        // 2026-11-16); the numbers are in the ring's panel.
        let used = status
            .as_ref()
            .and_then(|s| s["context"].as_f64())
            .map(|percent| format!("{}%", percent.clamp(0., 100.).round()));
        let percent = status
            .as_ref()
            .and_then(|s| s["context"].as_f64())
            .unwrap_or(0.);
        let menu = self.render_menu(t, &p, window, cx);
        let chips: Vec<_> = self
            .refs
            .iter()
            .enumerate()
            .map(|(i, item)| {
                h_flex()
                    .gap(px(look::GAP_4))
                    .h(px(look::CONTROL_ROW))
                    .pl(px(look::GAP_6))
                    .pr(px(look::GAP_2))
                    .max_w(px(220.))
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.ink(6.))
                    .text_size(px(look::SMALL))
                    .child(ref_icon(item))
                    .child(div().min_w_0().overflow_hidden().whitespace_nowrap().child(
                        if composer::is_slash(item) {
                            format!("/{}", ref_name(item))
                        } else {
                            ref_name(item).to_string()
                        },
                    ))
                    .child(
                        div()
                            .id(("ref-x", i))
                            .flex_none()
                            .size(px(look::CONTROL_ROW))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|d| d.bg(p.ink(look::BUTTON_HOVER)))
                            .child(icon("close").size(px(look::ICON_SMALL)).text_color(p.muted))
                            .on_click(cx.listener(move |this, _, _, cx| this.remove_ref(i, cx))),
                    )
            })
            .collect();
        let chat_agent = !terminal_only(&kind);
        let console = self.box_console();
        let model_label = if !chat_agent {
            kind_of(&kind).map_or("Agent", |kind| kind.name).to_string()
        } else if kind == "antigravity"
            && !self.models_of(&kind).models.iter().any(|m| m.id.is_some())
        {
            "Antigravity models not loaded".to_string()
        } else if composer::configured_models(&kind) && effort.is_empty() {
            short_model(&self.box_model())
        } else {
            format!(
                "{} ({})",
                short_model(&self.box_model()),
                effort_name(&effort)
            )
        };
        let pill_hover = p.ink(look::BUTTON_HOVER);
        // The bottom row's pills, in the status bar's capsule look.
        let pill = move |id: &'static str| {
            h_flex()
                .id(id)
                .flex_none()
                .gap(px(look::GAP_4))
                .h(px(look::CONTROL_ROW))
                .px(px(look::GAP_8))
                .rounded_full()
                .bg(p.cap_bg)
                .cursor_pointer()
                .whitespace_nowrap()
                .hover(move |d| d.bg(pill_hover))
        };
        let tip = |text: &'static str| {
            move |w: &mut Window, cx: &mut App| {
                gpui_kit::component::tooltip::Tooltip::new(text).build(w, cx)
            }
        };
        div()
            .flex_none()
            .px(px(look::PANE_PAD_X))
            .py(px(look::PANE_PAD_Y))
            .on_action(cx.listener(|this, _: &RunCell, window, cx| {
                // Shift+Enter adds a line.
                this.composer.update(cx, |c, cx| c.insert("\n", window, cx));
            }))
            // Up, Down, Tab and Enter are bound to the input's own actions,
            // so the mention list takes those actions rather than key events.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if matches!(this.menu, Some(Menu::Mention { .. })) {
                    this.move_mention(false, cx);
                    cx.stop_propagation();
                } else if this.recall(true, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if matches!(this.menu, Some(Menu::Mention { .. })) {
                    this.move_mention(true, cx);
                    cx.stop_propagation();
                } else if this.recall(false, window, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if matches!(this.menu, Some(Menu::Mention { .. })) {
                    this.pick_mention(None, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, action: &Enter, window, cx| {
                if matches!(this.menu, Some(Menu::Mention { .. }))
                    && !action.shift
                    && !this.slash_unmatched()
                {
                    this.pick_mention(None, window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.menu.is_some() {
                    this.close_menu(window, cx);
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Paste, window, cx| {
                // An image or files on the clipboard are attached; text
                // pastes as usual. Not in the context panel's own field.
                let here = this.composer.read(cx).focus_handle(cx).is_focused(window);
                if here && this.paste_attachments(cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Backspace, window, cx| {
                // Backspace at the start takes the last chip away.
                let at_start = {
                    let c = this.composer.read(cx);
                    c.focus_handle(cx).is_focused(window)
                        && c.selected_range().is_empty()
                        && c.cursor() == 0
                };
                if at_start && !this.refs.is_empty() {
                    this.refs.pop();
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .when_some(menu, |d, menu| d.child(menu))
            .child(
                v_flex()
                    .id("agent-box")
                    .relative()
                    .gap(px(look::GAP_6))
                    .px(px(look::GAP_8))
                    .pt(px(look::GAP_8))
                    .pb(px(look::GAP_6))
                    .rounded(px(look::RADIUS))
                    .border_1()
                    .border_color(edge)
                    // Dragged by its top edge (Huan, 2026-11-16): taller
                    // borrows room from the conversation above. The box's
                    // other rows keep their size; the text takes the rest.
                    .on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<look::ResizeInput>, window, cx| {
                            if event.drag(cx).0 != "agent-box-grip" {
                                return;
                            }
                            let current = this.composer_editor_height(cx);
                            let others = f32::from(event.bounds.size.height) - current;
                            let least = 2. * LINE + 8.;
                            let box_h = look::resized_input(
                                event.bounds.bottom(),
                                event.event.position.y,
                                least + others,
                                window,
                            );
                            let h = box_h - others;
                            this.composer_height = (h > least + 1.).then_some(h);
                            cx.notify();
                        },
                    ))
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.save_composer_height()),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, _| this.save_composer_height()),
                    )
                    .child(look::resize_grip("agent-box-grip", &p).on_click(cx.listener(
                        |this, event: &ClickEvent, _, cx| {
                            if event.click_count() == 2 {
                                this.composer_height = None;
                                crate::prefs::save_input_height("agent", None);
                                cx.notify();
                            }
                        },
                    )))
                    // Files dragged in from Finder are attached.
                    .drag_over::<ExternalPaths>(move |d, _, _, _| d.border_color(p.accent))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.attach_paths(paths.paths().to_vec(), cx)
                    }))
                    .when(!self.refs.is_empty(), |d| {
                        d.child(h_flex().flex_wrap().gap(px(look::GAP_4)).children(chips))
                    })
                    .child(
                        Editor::new(&self.composer)
                            .bordered(false)
                            .appearance(false)
                            .font_family(".SystemUIFont")
                            .text_size(px(look::CHROME))
                            .line_height(px(LINE))
                            .h(px(editor_height)),
                    )
                    .child(
                        h_flex()
                            .gap(px(look::GAP_6))
                            .h(px(look::CONTROL_CARD))
                            .text_size(px(look::SMALL))
                            .text_color(p.text)
                            .child(
                                pill("agent-add")
                                    .on_prepaint(self.popup_triggers.track("agent-mention"))
                                    .w(px(look::CONTROL_ROW))
                                    .px(px(0.))
                                    .justify_center()
                                    .when(matches!(self.menu, Some(Menu::Mention { .. })), |d| {
                                        d.bg(pill_hover)
                                    })
                                    .child(icon("add").size(px(look::ICON)).text_color(p.muted))
                                    .tooltip(tip("Add files, cells, variables, plots, commands or skills (@ # /)"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_add(window, cx)
                                    })),
                            )
                            .when_some(mode.filter(|_| chat_agent), |d, mode| {
                                d.child(
                                    pill("agent-mode")
                                        .on_prepaint(self.popup_triggers.track("agent-mode"))
                                        .when(self.menu == Some(Menu::Mode), |d| d.bg(pill_hover))
                                        .child(icon(mode.icon).size(px(look::ICON)).text_color(p.muted))
                                        .child(mode.label)
                                        .child(
                                            icon("chevron.down")
                                                .size(px(look::ICON_SMALL))
                                                .text_color(p.muted),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_menu(Menu::Mode, window, cx)
                                        })),
                                )
                            })
                            .when(chat_agent, |d| {
                                d.child(
                                    pill("agent-code")
                                        .child(div().text_color(p.muted).child("Code"))
                                        .child(if console { "Console" } else { "File" })
                                        .tooltip(tip(if console {
                                            "The agent's code goes into the Console: click to put it in the file"
                                        } else {
                                            "The agent's code goes into the file: click to run it in the Console"
                                        }))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_console(!console, cx)
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .when_some(used, |d, used| {
                                d.child(
                                    h_flex()
                                        .id("agent-tokens")
                                        .on_prepaint(self.popup_triggers.track("agent-context"))
                                        .flex_none()
                                        .gap(px(look::GAP_4))
                                        .h(px(look::CONTROL_ROW))
                                        .px(px(look::GAP_6))
                                        .rounded_full()
                                        .cursor_pointer()
                                        .text_color(p.muted)
                                        .hover(move |d| d.bg(pill_hover))
                                        .child(ring(percent, &p))
                                        .child(used)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_menu(Menu::Context, window, cx)
                                        })),
                                )
                            })
                            // New conversations always keep a provider
                            // picker, including Posit Assistant.
                            .when(chat_agent || self.show == Show::New, |d| {
                                d.child(
                                    pill("agent-model")
                                        .on_prepaint(self.popup_triggers.track("agent-model"))
                                        .flex_shrink(1.)
                                        .min_w(px(0.))
                                        .overflow_hidden()
                                        .pl(px(look::GAP_6))
                                        .when(self.menu == Some(Menu::Model), |d| d.bg(pill_hover))
                                        .child(provider(&kind))
                                        .child(shrinking(model_label))
                                        .child(
                                            icon("chevron.down")
                                                .size(px(look::ICON_SMALL))
                                                .text_color(p.muted),
                                        )
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.toggle_menu(Menu::Model, window, cx)
                                        })),
                                )
                            })
                            .child(
                                div()
                                    .id("agent-send")
                                    .flex_none()
                                    .size(px(look::CONTROL_CARD))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .map(|d| {
                                        if stopping {
                                            d.bg(p.text)
                                                .child(div().size(px(9.)).rounded(px(2.)).bg(p.bg))
                                        } else {
                                            d.bg(p.accent).when(!typed, |d| d.opacity(0.5)).child(
                                                icon("arrow.up")
                                                    .size(px(look::ICON))
                                                    .text_color(p.primary_text),
                                            )
                                        }
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if stopping {
                                            this.stop(cx)
                                        } else {
                                            this.submit(window, cx)
                                        }
                                    })),
                            ),
                    ),
            )
    }

    /// The message text's height: its lines (2 to 8), or more when the box
    /// was dragged taller.
    pub(crate) fn composer_editor_height(&self, cx: &App) -> f32 {
        let lines = self
            .composer
            .read(cx)
            .value()
            .split('\n')
            .count()
            .clamp(2, LINES);
        let own = lines as f32 * LINE + 8.;
        self.composer_height.map_or(own, |h| h.max(own))
    }

    /// Keeps the message box's dragged height, when it changed.
    fn save_composer_height(&mut self) {
        if crate::prefs::input_height("agent") != self.composer_height {
            crate::prefs::save_input_height("agent", self.composer_height);
        }
    }

    /// The open menu, drawn above the box.
    fn render_menu(
        &self,
        t: Option<&TerminalInfo>,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (body, right) = match self.menu.as_ref()? {
            Menu::Mention { .. } => (self.render_mentions(p, cx), false),
            Menu::Mode => (self.render_modes(p, cx), false),
            Menu::Model => (self.render_models(p, window, cx), true),
            Menu::Context => {
                let t = t?;
                let body = if crate::agents::context_panel::has_panel(&t.kind) {
                    self.render_context_panel(t, p, cx)
                } else {
                    render_context(t, p, cx)
                };
                (body, true)
            }
        };
        let trigger = match self.menu.as_ref()? {
            Menu::Mode => "agent-mode",
            Menu::Model => "agent-model",
            Menu::Context => "agent-context",
            Menu::Mention { .. } => "agent-mention",
        };
        let menu = body.on_mouse_down_out(cx.listener(
            move |this, event: &MouseDownEvent, window, cx| {
                if !this.popup_triggers.contains(trigger, event.position) {
                    this.close_menu(window, cx);
                }
            },
        ));
        Some(
            div()
                .h(px(0.))
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .when(right, |d| d.justify_end())
                .child(deferred(
                    anchored()
                        // It takes its menu's width in the row, which puts
                        // a right one at the right end.
                        .anchor(Anchor::BottomLeft)
                        .snap_to_window_with_margin(px(look::GAP_8))
                        .child(div().pb(px(look::GAP_4)).child(menu)),
                ))
                .into_any_element(),
        )
    }

    /// The @ menu: files and variables; the # menu: cells.
    fn render_mentions(&self, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let Some(Menu::Mention {
            sign,
            query,
            items,
            active,
        }) = &self.menu
        else {
            return div().id("none");
        };
        let sign = *sign;
        let groups = self.mention_groups();
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut index = 0;
        for (label, list) in &groups {
            rows.push(heading(label.clone(), p).into_any_element());
            for m in list {
                let i = index;
                index += 1;
                let on = i == *active;
                rows.push(
                    h_flex()
                        .id(("mention", i))
                        .flex_none()
                        .gap(px(look::GAP_4))
                        .h(px(look::ROW))
                        .px(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .cursor_pointer()
                        .when(on, |d| d.bg(p.hover))
                        .hover(|d| d.bg(p.accent).text_color(p.primary_text))
                        .child(ref_icon(&m.item))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(170.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(m.name().to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(p.faint)
                                .when(sign != '/', |d| d.font_family(look::MONO))
                                .text_size(px(look::SMALL))
                                .child(m.detail.clone().unwrap_or_default()),
                        )
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_mention(Some(i), window, cx)
                        }))
                        .into_any_element(),
                );
            }
        }
        if groups.is_empty() {
            let text = match items {
                None => "Loading…".to_string(),
                Some(_) if sign == '#' && composer::cell_documents(&self.refs).is_empty() => {
                    "Add a notebook with @ first; then # picks one of its cells".into()
                }
                Some(_) if sign == '/' && !query.is_empty() => {
                    format!("Nothing matches “{query}”: Enter sends it as typed")
                }
                Some(_) if !query.is_empty() => format!("Nothing matches “{query}”"),
                Some(_) if sign == '#' => "No code cells in it yet".into(),
                Some(_) if sign == '/' => "No commands or skills".into(),
                Some(_) => "Nothing to add yet".into(),
            };
            rows.push(heading(text, p).into_any_element());
        }
        let kind = self.box_kind();
        if sign == '/'
            && matches!(kind, "openscience" | "omp")
            && self.box_terminal().is_none_or(|t| !t.running)
        {
            let name = kind_of(kind).map_or("The agent", |k| k.name);
            rows.push(
                heading(
                    format!("{name}'s own commands come once the conversation starts"),
                    p,
                )
                .into_any_element(),
            );
        }
        if sign == '@' {
            rows.push(
                div()
                    .h(px(1.))
                    .my(px(look::GAP_4))
                    .bg(p.line)
                    .into_any_element(),
            );
            rows.push(
                h_flex()
                    .id("mention-attach")
                    .flex_none()
                    .gap(px(look::GAP_4))
                    .h(px(look::ROW))
                    .px(px(look::GAP_8))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .hover(|d| d.bg(p.accent).text_color(p.primary_text))
                    .child(icon("upload").size(px(look::ICON)))
                    .child("Attach from this computer…")
                    .child(div().flex_1())
                    .child(div().text_color(p.faint).child("files, images, PDFs"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_menu(window, cx);
                        this.attach(cx);
                    }))
                    .into_any_element(),
            );
            if !terminal_only(kind) {
                rows.push(
                    h_flex()
                        .id("mention-slash")
                        .flex_none()
                        .gap(px(look::GAP_4))
                        .h(px(look::ROW))
                        .px(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .cursor_pointer()
                        .hover(|d| d.bg(p.accent).text_color(p.primary_text))
                        .child(icon("skill").size(px(look::ICON)))
                        .child("Commands and skills…")
                        .child(div().flex_1())
                        .child(div().text_color(p.faint).child("/"))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(|this, _, window, cx| this.open_slash(window, cx)))
                        .into_any_element(),
                );
            }
        }
        popup(p)
            .id("mention-menu")
            .w(px(if sign == '/' {
                MENU_WIDTH + 80.
            } else {
                MENU_WIDTH
            }))
            .max_h(px(320.))
            .overflow_y_scroll()
            .track_scroll(&self.mention_scroll)
            .gap(px(0.))
            .children(rows)
    }

    /// The agent's own modes, in its own words.
    fn render_modes(&self, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let kind = self.box_kind().to_string();
        let current = self.box_mode().map(|m| m.permission);
        let rows = composer::modes(&kind).iter().map(|mode| {
            let on = Some(mode.permission) == current;
            let blocked = self.mode_note(mode).is_some();
            h_flex()
                .id(SharedString::from(format!("mode-{}", mode.permission)))
                .items_start()
                .min_h(px(look::ROW))
                .gap(px(look::GAP_4))
                .px(px(look::GAP_8))
                .py(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .when(on, |d| d.bg(p.hover))
                .map(|d| {
                    if blocked {
                        d.opacity(0.5)
                    } else {
                        d.cursor_pointer().hover(|d| d.bg(p.ink(7.)))
                    }
                })
                .child(
                    div()
                        .pt(px(look::GAP_2))
                        .child(icon(mode.icon).size(px(look::ICON))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(mode.label))
                        .child(
                            div()
                                .text_color(p.muted)
                                .line_height(rems(1.35))
                                .child(self.mode_text(mode)),
                        ),
                )
                .when(on, |d| {
                    d.child(
                        div()
                            .pt(px(look::GAP_2))
                            .child(icon("check").size(px(look::ICON)).text_color(p.accent)),
                    )
                })
                .on_click(cx.listener(move |this, _, _, cx| this.choose_mode(mode, cx)))
        });
        let name = kind_of(&kind).map_or("Agent", |k| k.name);
        popup(p)
            .id("mode-menu")
            .w(px(MENU_WIDTH))
            .child(heading(format!("{name} modes"), p))
            .children(rows)
    }

    /// The model menu's open tab: favourites, or an agent's.
    pub(crate) fn open_model_tab(&self) -> String {
        self.model_tab
            .clone()
            .filter(|t| t == "fav" || self.usable(t))
            .unwrap_or_else(|| self.box_kind().to_string())
    }

    /// The models the open tab lists, with their agents.
    fn model_rows(&self) -> Vec<(String, ModelChoice)> {
        let tab = self.open_model_tab();
        if tab == "fav" {
            let mut rows: Vec<(String, ModelChoice)> = Vec::new();
            for k in KINDS
                .iter()
                .filter(|k| !terminal_only(k.kind) && !crate::agents::turned_off(k.kind))
            {
                let models = self.models_of(k.kind);
                for key in &self.favourites {
                    let Some((kind, id)) = key.split_once(':') else {
                        continue;
                    };
                    if kind != k.kind
                        || rows.iter().any(|(kind, row)| {
                            kind == k.kind && models.same_model(row.id.as_deref(), Some(id))
                        })
                    {
                        continue;
                    }
                    let model = if id.is_empty() {
                        models.models.iter().find(|m| m.id.is_none()).cloned()
                    } else {
                        Some(models.choice(id))
                    };
                    if let Some(model) = model {
                        rows.push((kind.into(), model));
                    }
                }
            }
            rows
        } else if terminal_only(&tab) {
            Vec::new()
        } else {
            self.models_of(&tab)
                .models
                .into_iter()
                .map(|m| (tab.clone(), m))
                .collect()
        }
    }

    /// ⌥1, ⌥2… pick a model in the open tab of the model menu.
    pub(crate) fn model_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let keys = &event.keystroke;
        if self.menu != Some(Menu::Model)
            || !keys.modifiers.alt
            || keys.modifiers.platform
            || keys.modifiers.control
        {
            return false;
        }
        let Some(n) = keys
            .key
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=9).contains(n))
        else {
            return false;
        };
        let Some((kind, model)) = self.model_rows().into_iter().nth(n - 1) else {
            return true;
        };
        if self.usable(&kind) {
            self.choose_model(&kind, model, window, cx);
        }
        true
    }

    /// The model menu: tabs for favourites and each agent, the models, and
    /// the thinking effort.
    fn render_models(
        &self,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let tab = self.open_model_tab();
        let tab_button = |id: String, on: bool, usable: bool, inner: AnyElement, label: &str| {
            h_flex()
                .id(SharedString::from(id))
                .flex_none()
                .w_full()
                .h(px(look::CONTROL_CARD))
                .justify_center()
                .rounded(px(look::CONTROL_RADIUS))
                .when(on, |d| d.bg(p.hover))
                .map(|d| {
                    if usable {
                        d.cursor_pointer().hover(|d| d.bg(p.ink(7.)))
                    } else {
                        d.opacity(0.4)
                    }
                })
                .child(inner)
                .tooltip({
                    let label = label.to_string();
                    move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
                    }
                })
        };
        let mut tabs = vec![
            tab_button(
                "tab-fav".into(),
                tab == "fav",
                true,
                icon("star").size(px(look::ICON)).into_any_element(),
                "Favorites",
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.model_tab = Some("fav".into());
                cx.notify();
            })),
        ];
        for k in model_providers() {
            let usable = self.usable(k.kind);
            let name = k.kind;
            tabs.push(
                tab_button(
                    format!("tab-{}", k.kind),
                    tab == k.kind,
                    usable,
                    provider(k.kind).into_any_element(),
                    match k.kind {
                        "claude" => "Claude",
                        "posit" => "Posit",
                        _ => k.name,
                    },
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.usable(name) {
                        if terminal_only(name) {
                            this.choose_new_kind(name, cx);
                            this.model_tab = None;
                            this.close_menu(window, cx);
                        } else {
                            this.model_tab = Some(name.into());
                            cx.notify();
                        }
                    }
                })),
            );
        }
        let sign_in =
            |k: &crate::agents::Kind| k.plan.rsplit(" · ").next().unwrap_or(k.plan).to_string();
        let sub = if tab == "fav" {
            "Favourites from every agent".to_string()
        } else {
            let k = kind_of(&tab).unwrap_or(&KINDS[0]);
            format!("{} · {}", k.name, sign_in(k))
        };
        let rows = self.model_rows();
        let list: Vec<AnyElement> = if terminal_only(&tab) {
            vec![
                div()
                    .px(px(look::GAP_8))
                    .py(px(look::GAP_6))
                    .text_color(p.muted)
                    .child("Choose models inside Posit Assistant. Use the providers on the left to switch.")
                    .into_any_element(),
            ]
        } else if rows.is_empty() {
            vec![
                div()
                    .px(px(look::GAP_8))
                    .py(px(look::GAP_6))
                    .text_color(p.faint)
                    .child("Star a model to keep it here.")
                    .into_any_element(),
            ]
        } else {
            rows.into_iter()
                .enumerate()
                .map(|(i, (k, model))| {
                    let on = self.is_model(&k, &model);
                    let usable = self.usable(&k) && (k != "antigravity" || model.id.is_some());
                    let models = self.models_of(&k);
                    let starred = self.favourites.iter().any(|key| {
                        key.split_once(':').is_some_and(|(kind, id)| {
                            kind == k && models.same_model(Some(id), model.id.as_deref())
                        })
                    });
                    let (k2, m2) = (k.clone(), model.clone());
                    h_flex()
                        .id(("model", i))
                        .flex_none()
                        .w_full()
                        .min_w_0()
                        .gap(px(look::GAP_4))
                        .h(px(look::ROW))
                        .px(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .when(on, |d| d.bg(p.hover))
                        .map(|d| {
                            if usable {
                                d.cursor_pointer().hover(|d| d.bg(p.ink(7.)))
                            } else {
                                d.opacity(0.45)
                            }
                        })
                        .when(tab == "fav", |d| d.child(provider(&k)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(model.label.clone()),
                        )
                        .when(i < 9, |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(format!("⌥{}", i + 1)),
                            )
                        })
                        .child(
                            div()
                                .id(("star", i))
                                .flex_none()
                                .size(px(look::CONTROL_ROW))
                                .rounded(px(look::CONTROL_RADIUS))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|d| d.bg(p.ink(look::BUTTON_HOVER)))
                                .child(
                                    icon(if starred { "star.filled" } else { "star" })
                                        .size(px(look::ICON))
                                        .text_color(if starred { p.accent } else { p.faint }),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.star(&k2, &m2, cx)
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_model(&k, model.clone(), window, cx)
                        }))
                        .into_any_element()
                })
                .collect()
        };
        let content = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .flex_none()
                    .min_w_0()
                    .px(px(look::GAP_6))
                    .py(px(look::GAP_2))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(sub),
            )
            .when_some(self.model_note(), |d, note| {
                d.child(
                    div()
                        .flex_none()
                        .mx(px(look::GAP_4))
                        .px(px(look::GAP_4))
                        .py(px(look::GAP_4))
                        .rounded(px(look::CONTROL_RADIUS))
                        .bg(p.accent_soft)
                        .child(note),
                )
            })
            .child(
                v_flex()
                    .id(SharedString::from(format!("model-list-{tab}")))
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .gap(px(look::GAP_2))
                    .overflow_y_scroll()
                    .children(list),
            )
            .when_some(
                Some(self.effort_kind()).filter(|kind| {
                    !terminal_only(kind)
                        && (!composer::configured_models(kind)
                            || !self.models_of(kind).efforts.is_empty())
                        && (kind != "antigravity"
                            || self.models_of(kind).models.iter().any(|m| m.id.is_some()))
                }),
                |d, kind| d.child(self.render_effort(&kind, EFFORT_WIDTH, p, window, cx)),
            );
        popup(p)
            .id("model-menu")
            .flex_none()
            .w(px(MODEL_MENU_WIDTH))
            .h(px(MODEL_MENU_HEIGHT))
            .overflow_hidden()
            .child(
                h_flex()
                    .items_stretch()
                    .w_full()
                    .h_full()
                    .gap(px(MODEL_COLUMN_GAP))
                    .child(
                        v_flex()
                            .id("model-provider-tabs")
                            .flex_none()
                            .w(px(MODEL_PROVIDER_WIDTH))
                            .gap(px(look::GAP_4))
                            .children(tabs),
                    )
                    .child(content),
            )
    }
}

/// How full the context is, as a ring: amber from 70%, red from 90%, as
/// in the context panel.
fn ring(percent: f64, p: &Palette) -> AnyElement {
    let color = if percent >= 90. {
        p.err
    } else if percent >= 70. {
        p.warn
    } else {
        p.accent
    };
    crate::shell::meter::ring(Some((percent / 100.) as f32), color, p.ink(14.))
}

/// How much context is used, and Compact now.
fn render_context(t: &TerminalInfo, p: &Palette, cx: &mut Context<Agents>) -> Stateful<Div> {
    let status = t
        .agent
        .as_ref()
        .map(|a| a["status"].clone())
        .unwrap_or_default();
    let percent = status["context"].as_f64().unwrap_or(0.);
    let used = match status["context_size"].as_u64() {
        Some(size) => format!(
            "{} of {}",
            tokens((size as f64 * percent / 100.).round() as u64),
            tokens(size)
        ),
        None => format!("{}%", percent.round()),
    };
    popup(p)
        .id("context-menu")
        .w(px(340.))
        .gap(px(look::GAP_8))
        .p(px(look::GAP_12))
        .child(
            h_flex()
                .gap(px(look::GAP_4))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Context"))
                .child(div().text_color(p.faint).child(format!("{used} used"))),
        )
        .child(
            div()
                .h(px(6.))
                .rounded_full()
                .bg(p.ink(10.))
                .child(
                    div()
                        .h_full()
                        .w(relative((percent / 100.).clamp(0.01, 1.) as f32))
                        .rounded_full()
                        .bg(if percent >= 70. { p.warn } else { p.accent }),
                ),
        )
        .child(
            div()
                .text_color(p.muted)
                .line_height(rems(1.4))
                .child("Compacting replaces the conversation with a summary, to make room. Sent to the agent as /compact."),
        )
        .child(
            h_flex().justify_end().child(
                look::button("compact", "Compact now", None, look::ButtonSize::Card)
                    .primary()
                    .font_weight(FontWeight::SEMIBOLD)
                    .on_click(cx.listener(|this, _, _, cx| this.compact(cx))),
            ),
        )
}

/// A pill's text that gives way first, ending in "…", so its icons stay.
fn shrinking(text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .overflow_hidden()
        .text_ellipsis()
        .child(text.into())
}
