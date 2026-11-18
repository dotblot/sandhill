//! The Plots pane's size menu.

use super::*;

impl Workbench {
    pub(crate) fn dismiss_plot_size_menu(&mut self) -> bool {
        std::mem::take(&mut self.plot_history.size_menu)
    }

    pub(super) fn toggle_plot_size_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dismiss_plot_size_menu() {
            cx.notify();
            return;
        }
        let (width, height, dpi) = match self.plot_history.sizing {
            PlotSizing::Fixed { width, height, dpi } => (width, height, dpi),
            PlotSizing::Fit => (7., 5., DPI),
        };
        self.plot_history.size_inputs = Some((
            cx.new(|cx| InputState::new(window, cx).default_value(width.to_string())),
            cx.new(|cx| InputState::new(window, cx).default_value(height.to_string())),
            cx.new(|cx| InputState::new(window, cx).default_value(dpi.to_string())),
        ));
        self.plot_history.size_error = None;
        self.plot_history.size_menu = true;
        cx.notify();
    }

    fn set_plot_sizing(&mut self, sizing: PlotSizing, cx: &mut Context<Self>) {
        self.plot_history.sizing = sizing;
        self.plot_history.size_menu = false;
        self.plot_history.room.borrow_mut().clear();
        self.plot_history.fitting = None;
        self.plot_history
            .notes
            .retain(|key, _| self.plot_history.cannot.contains(key));
        if let Some(project) = &self.project {
            crate::prefs::save_plot_sizing(&self.host(), project, sizing);
        }
        cx.notify();
    }

    fn apply_custom_plot_size(&mut self, cx: &mut Context<Self>) {
        let Some((width, height, dpi)) = &self.plot_history.size_inputs else {
            return;
        };
        let parsed = width
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .zip(height.read(cx).value().trim().parse::<f64>().ok())
            .zip(dpi.read(cx).value().trim().parse::<u32>().ok())
            .map(|((width, height), dpi)| PlotSizing::Fixed { width, height, dpi });
        if let Some(sizing) = parsed.filter(|s| s.pixels().is_some()) {
            self.set_plot_sizing(sizing, cx);
        } else {
            self.plot_history.size_error = Some(
                "Enter positive inches and a whole-number DPI, at most 30,000 pixels per side."
                    .into(),
            );
            cx.notify();
        }
    }

    pub(super) fn render_plot_size_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.plot_history.size_menu {
            return None;
        }
        let (width, height, dpi) = self.plot_history.size_inputs.as_ref()?;
        let p = *Palette::get(cx);
        let mut rows = v_flex().gap(px(look::GAP_2));
        for (i, (label, sizing)) in [
            ("Fit to pane", PlotSizing::Fit),
            (
                "3.5 × 3 in",
                PlotSizing::Fixed {
                    width: 3.5,
                    height: 3.,
                    dpi: DPI,
                },
            ),
            (
                "7 × 5 in",
                PlotSizing::Fixed {
                    width: 7.,
                    height: 5.,
                    dpi: DPI,
                },
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if i == 1 {
                rows = rows.child(
                    div()
                        .px(px(look::GAP_8))
                        .py(px(look::GAP_2))
                        .text_color(p.faint)
                        .child("Fixed size"),
                );
            }
            let picked = self.plot_history.sizing == sizing;
            rows = rows.child(
                h_flex()
                    .id(("plot-size-choice", i))
                    .h(px(look::ROW))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .hover(move |d| d.bg(p.hover))
                    .child(div().w(px(look::ICON)).flex_none().when(picked, |d| {
                        d.child(look::icon("check").size(px(look::ICON)))
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_plot_sizing(sizing, cx))),
            );
        }
        let body = crate::agents::composer_view::popup(&p)
            .id("plot-size-menu")
            .occlude()
            .w(px(310.))
            .text_size(px(look::SMALL))
            .child(rows)
            .child(
                div()
                    .px(px(look::GAP_8))
                    .pt(px(look::GAP_8))
                    .pb(px(look::GAP_4))
                    .text_color(p.faint)
                    .child("Custom: width × height (in), DPI"),
            )
            .child(
                h_flex()
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_8))
                    .child(div().w(px(64.)).child(plot_field(width)))
                    .child("×")
                    .child(div().w(px(64.)).child(plot_field(height)))
                    .child(div().w(px(64.)).child(plot_field(dpi))),
            )
            .child(
                look::bar_button("plot-size-apply", "Apply custom size", None, &p)
                    .h(px(look::CONTROL_CARD))
                    .on_click(cx.listener(|this, _, _, cx| this.apply_custom_plot_size(cx))),
            )
            .when_some(self.plot_history.size_error.clone(), |d, error| {
                d.child(div().px(px(look::GAP_8)).text_color(p.err).child(error))
            })
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("plot-size", event.position) {
                    return;
                }
                this.dismiss_plot_size_menu();
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }
}
