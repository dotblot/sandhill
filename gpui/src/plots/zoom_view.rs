//! The large plot view's rendering.

use super::*;

impl Workbench {
    /// The large view over the window, when open.
    pub(crate) fn render_zoom(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let zoom = self.plot_history.zoom.as_ref()?;
        let p = *Palette::get(cx);
        let (list, at) = self.zoom_plots();
        let item = list.get(at?)?;
        let at = at?;
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let formats: &[&'static str] = if redrawn {
            &["png", "svg", "pdf"]
        } else {
            plain_formats(&item.mime)
        };
        let explain = match &zoom.redraw {
            None => "Checking whether this plot can be redrawn…".to_string(),
            Some(r) if r.can => zoom.preview_note.clone().unwrap_or_else(|| {
                if zoom.redraw_preview {
                    "Drag the title to move. Drag the corner to resize.".into()
                } else {
                    "Preview is scaled only. Save still redraws at the export size.".into()
                }
            }),
            Some(r) => match &r.reason {
                Some(why) => format!("Saved as drawn: {why}"),
                None => "Saved as drawn: its session can't draw it again.".into(),
            },
        };
        let image: AnyElement = match zoom.preview.clone().or_else(|| self.plot_image(item)) {
            Some(i) => img(i)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div().child("Loading the plot…").into_any_element(),
        };
        let segment = |f: &'static str, cx: &mut Context<Self>| {
            let on = zoom.format == f;
            div()
                .id(SharedString::from(format!("fmt-{f}")))
                .flex_1()
                .flex()
                .justify_center()
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(on, |d| d.bg(p.bg).text_color(p.text))
                .child(f.to_uppercase())
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_zoom_format(f, window, cx)),
                )
        };
        let reason = zoom
            .redraw
            .as_ref()
            .and_then(|r| r.reason.clone())
            .unwrap_or_else(|| {
                if redrawn {
                    "Redraw the preview through its live session".into()
                } else {
                    explain.clone()
                }
            });
        let mut units = v_flex().gap(px(2.));
        for unit in [PlotUnit::Pixels, PlotUnit::Inches, PlotUnit::Centimetres] {
            units = units.child(
                look::bar_button(
                    SharedString::from(format!("zoom-unit-{}", unit.label())),
                    unit.label(),
                    None,
                    &p,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(zoom) = &mut this.plot_history.zoom {
                        zoom.unit = unit;
                        zoom.units_open = false;
                    }
                    this.sync_zoom_dimensions(window, cx);
                    this.remember_zoom_sizing(cx);
                })),
            );
        }
        let unit_picker = div()
            .relative()
            .child(
                look::bar_button("zoom-unit", zoom.unit.label(), Some("chevron.down"), &p)
                    .on_prepaint(self.popup_triggers.track("zoom-unit"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(zoom) = &mut this.plot_history.zoom {
                            zoom.units_open = !zoom.units_open;
                        }
                        cx.notify();
                    })),
            )
            .when(zoom.units_open, |d| {
                d.child(
                    div().absolute().top(px(28.)).left_0().child(
                        deferred(
                            crate::composer_view::popup(&p)
                                .id("zoom-units")
                                .occlude()
                                .w(px(78.))
                                .text_size(px(look::SMALL))
                                .child(units)
                                .on_mouse_down_out(cx.listener(
                                    |this, event: &MouseDownEvent, _, cx| {
                                        if this.popup_triggers.contains("zoom-unit", event.position)
                                        {
                                            return;
                                        }
                                        if let Some(zoom) = &mut this.plot_history.zoom {
                                            zoom.units_open = false;
                                        }
                                        cx.notify();
                                    },
                                )),
                        )
                        .with_priority(3),
                    ),
                )
            });
        let back = at > 0;
        let on = at + 1 < list.len();
        // Narrower than this, the settings go under the preview.
        let viewport = window.viewport_size();
        let card_width = zoom
            .size
            .map_or(viewport.width * 0.86, |size| size.width)
            .min(viewport.width * 0.96);
        let beside = card_width >= px(720.);
        let heading = |text: &'static str| {
            div()
                .text_size(px(look::SMALL - 1.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.faint)
                .child(text)
        };
        let switch_row = |switch: AnyElement, label: &'static str| {
            h_flex()
                .h(px(22.))
                .flex_none()
                .gap(px(8.))
                .child(switch)
                .child(label)
        };
        let pdf = zoom.format == "pdf" && redrawn;
        let settings = v_flex()
            .id("plot-zoom-settings")
            .when(beside, |d| {
                d.w(px(260.))
                    .flex_none()
                    .h_full()
                    .border_l(px(0.5))
            })
            .when(!beside, |d| d.w_full().max_h(px(260.)).border_t(px(0.5)))
            .border_color(p.glass_edge)
            .overflow_y_scroll()
            .px(px(14.))
            .py(px(12.))
            .gap(px(8.))
            .text_size(px(look::SMALL))
            .child(heading("FORMAT"))
            .child(
                h_flex()
                    .gap(px(2.))
                    .p(px(2.))
                    .rounded(px(8.))
                    .bg(p.ink(6.))
                    .text_color(p.muted)
                    .children(formats.iter().map(|f| segment(f, cx))),
            )
            .child(div().h(px(4.)))
            .child(heading("SIZE"))
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_none()
                    .gap(px(6.))
                    .child("W")
                    .child(div().w(px(70.)).child(Input::new(&zoom.width).small()))
                    .child("H")
                    .child(div().w(px(70.)).child(Input::new(&zoom.height).small()))
                    .child(unit_picker),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_none()
                    .gap(px(6.))
                    .child("Resolution")
                    .child(div().w(px(70.)).child(Input::new(&zoom.dpi_input).small()))
                    .child("dpi"),
            )
            .child(switch_row(
                Switch::new("zoom-follow")
                    .small()
                    .color(p.accent)
                    .checked(zoom.follow)
                    .accessibility_label("Size follows window")
                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                        if let Some(zoom) = &mut this.plot_history.zoom {
                            zoom.follow = *on;
                            if *on && let Some(room) = zoom.room.get() {
                                zoom.physical = window_inches(room.width, room.height);
                            }
                        }
                        this.sync_zoom_dimensions(window, cx);
                        this.remember_zoom_sizing(cx);
                    }))
                    .into_any_element(),
                "Size follows window",
            ))
            .when(pdf, |d| {
                d.child(switch_row(
                    Switch::new("zoom-rasterize")
                        .small()
                        .color(p.accent)
                        .checked(zoom.rasterize)
                        .accessibility_label("Rasterize")
                        .on_change(cx.listener(|this, on: &bool, _, cx| {
                            if let Some(zoom) = &mut this.plot_history.zoom {
                                zoom.rasterize = *on;
                            }
                            if let Some(project) = &this.project {
                                crate::prefs::save_plot_rasterize(&this.host(), project, *on);
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                    "Rasterize",
                ))
                .child(
                    div()
                        .pl(px(36.))
                        .text_color(p.faint)
                        .whitespace_normal()
                        .child(
                            "Draws the plot as pixels at the resolution above; smaller files for plots with many points.",
                        ),
                )
            })
            .child(div().h(px(4.)))
            .child(heading("PREVIEW"))
            .child(switch_row(
                div()
                    .id("zoom-redraw-tip")
                    .child(
                        Switch::new("zoom-redraw")
                            .small()
                            .color(p.accent)
                            .checked(redrawn && zoom.redraw_preview)
                            .disabled(!redrawn)
                            .accessibility_label("Redraw preview")
                            .on_change(cx.listener(|this, on: &bool, _, cx| {
                                let Some(zoom) = &mut this.plot_history.zoom else {
                                    return;
                                };
                                zoom.redraw_preview = *on;
                                zoom.fitting = None;
                                zoom.preview_note = None;
                                let room = zoom.room.clone();
                                if *on {
                                    this.fit_zoom(room, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(reason.clone()).build(window, cx)
                    })
                    .into_any_element(),
                "Redraw preview",
            ))
            .child(div().h(px(4.)))
            .child(heading("SAVE TO"))
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_none()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(self.relative_path(&zoom.folder)),
                    )
                    .child(
                        look::bar_button("zoom-folder", "Choose…", Some("folder"), &p).on_click(
                            cx.listener(|this, _, window, cx| this.choose_plot_folder(window, cx)),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_none()
                    .gap(px(6.))
                    .child("Name")
                    .child(div().flex_1().min_w_0().child(Input::new(&zoom.name).small())),
            )
            .child(div().flex_1().min_h(px(8.)))
            .child(
                div()
                    .id("zoom-save")
                    .flex_none()
                    .w_full()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(RADIUS))
                    .bg(p.accent)
                    .text_color(p.primary_text)
                    .font_weight(FontWeight::SEMIBOLD)
                    .cursor_pointer()
                    .child(if zoom.replace.is_some() {
                        "Replace"
                    } else {
                        "Save"
                    })
                    .when(zoom.working || zoom.redraw.is_none(), |d| d.opacity(0.5))
                    .when(!zoom.working && zoom.redraw.is_some(), |d| {
                        d.on_click(cx.listener(|this, _, window, cx| this.save_plot(window, cx)))
                    }),
            );
        let shown = zoom.settings;
        let card = v_flex()
            .id("plot-zoom")
            .occlude()
            .relative()
            .left(zoom.offset.x)
            .top(zoom.offset.y)
            .text_size(px(look::SMALL))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .map(|d| match zoom.size {
                Some(size) => d.w(size.width).h(size.height),
                None => d.w(relative(0.86)).h(relative(0.86)),
            })
            .max_w(relative(0.96))
            .max_h(relative(0.96))
            .bg(p.bg)
            .rounded(px(RADIUS))
            .border_1()
            .border_color(p.glass_edge)
            .shadow(vec![BoxShadow {
                color: p.shadow.opacity(0.35),
                offset: point(px(0.), px(12.)),
                blur_radius: px(36.),
                spread_radius: px(0.),
                inset: false,
            }])
            .child(
                h_flex()
                    .flex_none()
                    .h(px(36.))
                    .px(px(8.))
                    .gap(px(4.))
                    .border_b(px(0.5))
                    .border_color(p.glass_edge)
                    .child(
                        div()
                            .id("zoom-title-drag")
                            .cursor(CursorStyle::ClosedHand)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    if let Some(zoom) = &mut this.plot_history.zoom {
                                        zoom.moving = Some((event.position, zoom.offset));
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .flex_1()
                            .min_w_0()
                            .pl(px(6.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(format!(
                                "{} · {} of {} · {}",
                                source(item),
                                at + 1,
                                list.len(),
                                ago(now_ms() - item.ts_ms)
                            )),
                    )
                    .child(
                        icon_button("zoom-settings", "sidebar.right", &p, shown)
                            .tooltip(move |window, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(if shown {
                                    "Hide the settings"
                                } else {
                                    "Show the settings"
                                })
                                .build(window, cx)
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(zoom) = &mut this.plot_history.zoom {
                                    zoom.settings = !zoom.settings;
                                    crate::prefs::save_plot_settings_shown(zoom.settings);
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        icon_button("zoom-back", "previous", &p, false)
                            .when(!back, |d| d.opacity(0.35))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step_zoom(-1, window, cx)),
                            ),
                    )
                    .child(
                        icon_button("zoom-next", "next", &p, false)
                            .when(!on, |d| d.opacity(0.35))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.step_zoom(1, window, cx)),
                            ),
                    )
                    .child(
                        icon_button("zoom-close", "close", &p, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.plot_history.zoom = None;
                                cx.notify();
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(beside, |d| d.flex_row())
                    .when(!beside, |d| d.flex_col())
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .m(px(14.))
                                    .mb(px(6.))
                                    .relative()
                                    .overflow_hidden()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        div()
                                            .relative()
                                            .map(|d| match zoom.initial_picture {
                                                Some(picture) => d
                                                    .w(picture.width)
                                                    .h(picture.height)
                                                    .max_w(relative(1.))
                                                    .max_h(relative(1.)),
                                                None => d.size_full(),
                                            })
                                            .child(image)
                                            .child(self.zoom_room_watcher(cx)),
                                    ),
                            )
                            // Status changes must not change the preview's
                            // height: that would resize it and redraw again.
                            .child(
                                div()
                                    .id("plot-zoom-status")
                                    .h(px(18.))
                                    .mb(px(8.))
                                    .px(px(14.))
                                    .flex_none()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(look::SMALL))
                                    .text_color(p.faint)
                                    .child(zoom.note.clone().unwrap_or_else(|| explain.clone()))
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(explain.clone())
                                            .build(window, cx)
                                    }),
                            ),
                    )
                    .when(shown, |d| d.child(settings)),
            )
            .child(
                div()
                    .id("plot-zoom-resize")
                    .absolute()
                    .right_0()
                    .bottom_0()
                    .size(px(20.))
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .text_color(p.muted)
                    .child("◢")
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "Drag to resize the plot preview",
                        )
                        .build(window, cx)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            let viewport = window.viewport_size();
                            if let Some(zoom) = this.plot_history.zoom.as_mut() {
                                let current = zoom
                                    .size
                                    .unwrap_or(size(viewport.width * 0.86, viewport.height * 0.86));
                                let current = size(
                                    current.width.min(viewport.width * 0.96),
                                    current.height.min(viewport.height * 0.96),
                                );
                                zoom.resizing = Some((event.position, current));
                            }
                            cx.stop_propagation();
                        }),
                    ),
            );
        let folder_dialog = zoom.folder_dialog.clone();
        Some(
            deferred(
                div()
                    .id("plot-zoom-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .children(folder_dialog)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.plot_history.zoom = None;
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}
