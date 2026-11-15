use super::*;

impl Explorer {
    /// The action row: what the table is on the left (the column panel's
    /// toggle, the table's name and a status dot), what to do with it on
    /// the right (Search, Filter, Export, read again, a window of its own,
    /// settings), in an order that never moves.
    pub(super) fn render_top(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let what = self
            .shape
            .as_ref()
            .map_or_else(|| self.name(), |s| s.code.clone());
        let code = format!("{} · {what}", self.language_name());
        let side = self.side_shown;
        let (dot, status) = if let Some(err) = &self.error {
            (p.err, err.clone())
        } else if self.busy {
            (
                p.warn,
                format!(
                    "{} is running code; the table updates when it finishes",
                    self.language_name()
                ),
            )
        } else {
            (
                p.ok,
                "Live: read again after each run in its session".to_string(),
            )
        };
        let filters = self.filters.len();
        h_flex()
            .flex_1()
            .w_full()
            .h_full()
            .px(px(6.))
            .gap(px(4.))
            .whitespace_nowrap()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && this.edit.is_some() {
                    this.edit = None;
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                look::icon_button("de-side", "sidebar.left", &p, side)
                    .tooltip(tip(if side {
                        "Hide the column panel"
                    } else {
                        "Show the column panel"
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_side(!side, cx))),
            )
            .child(
                h_flex()
                    .id("de-name")
                    // The name gives way first, so the actions stay in reach.
                    .flex_shrink(1.)
                    .min_w(px(40.))
                    .gap(px(6.))
                    .px(px(4.))
                    .children(
                        look::language_icon(&self.language)
                            .map(|path| look::picture(path).size(px(14.))),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.name()),
                    )
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(code.clone()).build(w, cx)
                    }),
            )
            .child(
                div()
                    .id("de-status")
                    .flex_none()
                    .p(px(3.))
                    .child(look::dot(dot))
                    .tooltip(move |w, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(status.clone()).build(w, cx)
                    }),
            )
            .child(div().flex_1())
            .child(
                div().flex_none().w(px(150.)).child(
                    Input::new(&self.search_input)
                        .small()
                        .prefix(icon("search").size(px(13.)).text_color(p.muted)),
                ),
            )
            .child(
                bar_button(
                    "de-add-filter",
                    if filters == 0 {
                        "Filter".to_string()
                    } else {
                        format!("Filter · {filters}")
                    },
                    Some("filter"),
                    &p,
                )
                .on_prepaint(self.popup_triggers.track("add-filter"))
                .when(filters > 0, |d| d.text_color(p.accent))
                .tooltip(tip("Add a filter"))
                .on_click(cx.listener(|this, _, window, cx| this.open_filter(None, window, cx))),
            )
            .child(
                bar_button("de-export", "Export", Some("doc.save"), &p)
                    .on_prepaint(self.popup_triggers.track("export"))
                    .when(self.export_menu.is_some(), |d| d.bg(p.ink(10.)))
                    .child(icon("chevron.down").size(px(10.)))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        let open = this.export_menu.is_none();
                        this.close_menus();
                        if open {
                            this.export_menu = Some(event.position());
                        }
                        cx.notify();
                    })),
            )
            .child(
                look::icon_button("de-refresh", "refresh", &p, false)
                    .tooltip(tip("Read the table again"))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
            .when(!self.popped, |d| {
                d.child(
                    look::icon_button("de-pop-out", "window.pop-out", &p, false)
                        .tooltip(tip("Open in a new window"))
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(PopOut))),
                )
            })
            .child(
                look::icon_button("de-settings", "settings", &p, false)
                    .on_prepaint(self.popup_triggers.track("settings"))
                    .tooltip(tip("Settings"))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        let open = this.settings_menu.is_none();
                        this.close_menus();
                        if open {
                            this.settings_menu = Some(event.position());
                        }
                        cx.notify();
                    })),
            )
    }

    pub(super) fn menu_open(&self) -> bool {
        self.column_menu.is_some()
            || self.settings_menu.is_some()
            || self.export_menu.is_some()
            || self.columns_menu.is_some()
    }

    pub(super) fn close_menus(&mut self) {
        self.column_menu = None;
        self.settings_menu = None;
        self.export_menu = None;
        self.columns_menu = None;
    }

    pub(super) fn render_note(&self, p: &Palette) -> Option<AnyElement> {
        let (text, failed) = if self.busy {
            (
                format!(
                    "{} is running code; the table updates when it finishes.",
                    self.language_name()
                ),
                false,
            )
        } else if let Some(err) = &self.error {
            (err.clone(), true)
        } else {
            self.note.clone()?
        };
        let busy_note = self.busy;
        Some(
            div()
                .flex_none()
                .px(px(10.))
                .py(px(4.))
                .border_b_1()
                .border_color(p.line)
                .when(failed, |d| d.text_color(p.err).bg(p.err_soft))
                .when(!failed && busy_note, |d| {
                    d.text_color(p.busy).bg(p.busy_soft)
                })
                .when(!failed && !busy_note, |d| d.text_color(p.muted))
                .child(text)
                .into_any_element(),
        )
    }
}
