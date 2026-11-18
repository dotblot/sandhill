//! The Plots pane: its tools, images, body and room watcher.

use super::*;

impl Workbench {
    /// The buttons over the Plots pane.
    pub(crate) fn plot_tools(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = *Palette::get(cx);
        let (list, at) = self.plots_now();
        let has = at.is_some();
        let back = at.is_some_and(|a| a > 0);
        let on = at.is_some_and(|a| a + 1 < list.len());
        let sizing = self.plot_history.sizing;
        // The tools share a narrow strip with the pane's tabs: keep it short.
        let size_label = match sizing {
            PlotSizing::Fit => "Fit".to_string(),
            PlotSizing::Fixed { width, height, .. } => format!("{width}×{height}"),
        };
        vec![
            look::bar_button("plot-size", size_label, Some("chevron.down"), &p)
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Plot size").build(w, cx)
                })
                .on_prepaint(self.popup_triggers.track("plot-size"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_plot_size_menu(window, cx)))
                .into_any_element(),
            icon_button("plot-back", "previous", &p, false)
                .when(!back, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.step_plot(-1, cx)))
                .into_any_element(),
            icon_button("plot-next", "next", &p, false)
                .when(!on, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.step_plot(1, cx)))
                .into_any_element(),
            icon_button("plot-zoom", "zoom", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| this.open_zoom(window, cx)))
                .into_any_element(),
            icon_button("plot-copy", "edit.copy", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .tooltip(|w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new("Copy as PNG").build(w, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.copy_plot(cx)))
                .into_any_element(),
            icon_button("plot-save", "doc.save", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, window, cx| this.open_zoom(window, cx)))
                .into_any_element(),
            icon_button("plot-clear", "delete", &p, false)
                .when(!has, |d| d.opacity(0.35))
                .on_click(cx.listener(|this, _, _, cx| this.clear_plots(cx)))
                .into_any_element(),
        ]
    }

    /// The image of a plot, as cached; asks for it the first time.
    pub(super) fn plot_image(&self, item: &PlotItem) -> Option<std::sync::Arc<Image>> {
        match self.plots.get(&plot_key(&item.value)) {
            Some(Plot::Ready(image)) => Some(image.clone()),
            _ => None,
        }
    }

    /// Asks for the images of the plots shown that aren't read yet.
    pub(crate) fn fetch_plot_images(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(String, Value)> = {
            let (list, _) = self.plots_now();
            list.iter()
                .filter(|p| !self.plots.contains_key(&plot_key(&p.value)))
                .map(|p| (p.mime.clone(), p.value.clone()))
                .collect()
        };
        for (mime, value) in wanted {
            self.fetch_image(&mime, &value, cx);
        }
    }

    /// The Plots pane's body: the plot in front, where it came from, and a
    /// strip of thumbnails.
    pub(crate) fn render_plots(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let (list, at) = self.plots_now();
        let Some(at) = at else {
            return v_flex().size_full().relative()
                .child(div().px(px(14.)).py(px(8.)).text_color(p.muted)
                    .child("Plots that R and Python draw show here: from the Console, scripts and agents. Notebooks' plots stay under their cells."))
                .children(self.render_plot_size_menu(cx))
                .into_any_element();
        };
        let item = list[at];
        let shown = self.fitted_image(item).or_else(|| self.plot_image(item));
        let main: AnyElement = match shown.clone() {
            Some(image) => img(image)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .text_color(p.faint)
                .child("Loading the plot…")
                .into_any_element(),
        };
        let thumbs = (list.len() > 1).then(|| {
            let hover = p.ink(8.);
            h_flex()
                .id("plot-thumbs")
                .flex_none()
                .h(px(62.))
                .gap(px(6.))
                .px(px(8.))
                .py(px(6.))
                .border_t(px(0.5))
                .border_color(p.glass_edge)
                .overflow_x_scroll()
                .children(list.iter().enumerate().map(|(ix, t)| {
                    let on = ix == at;
                    div()
                        .id(SharedString::from(format!("thumb-{ix}")))
                        .flex_none()
                        .w(px(64.))
                        .h(px(48.))
                        .p(px(2.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(if on { p.accent } else { p.line })
                        .bg(p.bg)
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .children(
                            self.plot_image(t)
                                .map(|i| img(i).size_full().object_fit(ObjectFit::Contain)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.pick_plot(ix, cx)))
                }))
        });
        let when = ago(now_ms() - item.ts_ms);
        v_flex()
            .size_full()
            .relative()
            .child(
                div()
                    .id("plot-stage")
                    .flex_1()
                    .min_h_0()
                    .m(px(8.))
                    .relative()
                    .overflow_hidden()
                    .when_some(shown, |d, image| {
                        d.on_prepaint(self.track_plot_bounds(plot_key(&item.value), image))
                    })
                    .child(main)
                    .child(self.room_watcher(item.key.clone(), cx))
                    .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                        if event.click_count() == 2 {
                            this.open_zoom(window, cx);
                        }
                    })),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px(px(10.))
                    .pb(px(4.))
                    .gap(px(8.))
                    .text_size(px(look::SMALL))
                    .text_color(p.faint)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(p.muted)
                            .child(source(item)),
                    )
                    .child(format!("{} of {} · {when}", at + 1, list.len())),
            )
            .when_some(self.plot_history.notes.get(&item.key), |d, note| {
                d.child(
                    div()
                        .flex_none()
                        .px(px(10.))
                        .pb(px(4.))
                        .text_size(px(look::SMALL))
                        .text_color(p.muted)
                        .child(note.clone()),
                )
            })
            .children(thumbs)
            .children(self.render_plot_size_menu(cx))
            .into_any_element()
    }

    /// Draws nothing, but sees the room the plot has each frame and, when
    /// it or the plot in front changed, asks for the plot to fit it.
    fn room_watcher(&self, plot: String, cx: &mut Context<Self>) -> impl IntoElement {
        let room = self.plot_history.room.clone();
        let sizing = self.plot_history.sizing;
        let this = cx.weak_entity();
        canvas(
            move |bounds, window, cx| {
                let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let key = size_key(&plot, sizing, width, height);
                if *room.borrow() == key {
                    return;
                }
                *room.borrow_mut() = key;
                let scale = window.scale_factor();
                let plot = plot.clone();
                let this = this.clone();
                cx.defer(move |cx| {
                    this.update(cx, |this, cx| this.fit_plot(plot, width, height, scale, cx))
                        .ok();
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
}
