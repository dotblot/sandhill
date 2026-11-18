//! The large plot view's state: opening, stepping, sizing and fitting.

use super::*;

impl Workbench {
    /// The plots the large view steps through, and the one it shows.
    pub(super) fn zoom_plots(&self) -> (Vec<&PlotItem>, Option<usize>) {
        match self
            .plot_history
            .zoom
            .as_ref()
            .and_then(|z| z.only.as_ref())
        {
            Some((items, at)) => (items.iter().collect(), Some(*at)),
            None => {
                let (mut items, _) = self.plots_now();
                let at = self.plot_history.zoom.as_ref().and_then(|zoom| {
                    items
                        .iter()
                        .position(|item| item.key == zoom.key)
                        .or_else(|| {
                            let item = zoom.retained.as_ref()?;
                            items.push(item);
                            Some(items.len() - 1)
                        })
                });
                (items, at)
            }
        }
    }

    /// The plots under notebook cell `ix`, as `PlotItem`s: of its latest
    /// run, else as saved in the file.
    fn cell_plots(&self, ix: usize) -> Vec<PlotItem> {
        let Some(open) = self.open.as_ref() else {
            return Vec::new();
        };
        let Some(cell) = open.notebook.cells.get(ix) else {
            return Vec::new();
        };
        let run = open
            .cell_runs
            .get(&cell.id)
            .and_then(|id| self.runs.get(id).map(|run| (id, run)));
        let (outputs, run_id, session) = match run {
            Some((id, run)) => (
                &run.outputs,
                id.clone(),
                open.session.id().unwrap_or_default().to_string(),
            ),
            None => (&cell.outputs, format!("saved-{}", cell.id), String::new()),
        };
        let place = Some((open.notebook.path.clone(), cell.id.clone()));
        outputs
            .iter()
            .filter(|o| {
                matches!(
                    o["output_type"].as_str(),
                    Some("display_data" | "execute_result")
                )
            })
            .filter_map(|o| {
                plot_item(
                    &run_id,
                    &session,
                    now_ms(),
                    false,
                    place.clone(),
                    None,
                    &o["data"],
                )
            })
            .collect()
    }

    /// Opens a plot under notebook cell `ix` large, to save it.
    pub(crate) fn open_cell_plot(
        &mut self,
        ix: usize,
        key: &str,
        bounds_key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = self.cell_plots(ix);
        let Some(at) = items.iter().position(|p| plot_key(&p.value) == key) else {
            return;
        };
        let picture = self
            .plot_history
            .image_bounds
            .borrow()
            .get(bounds_key)
            .copied();
        self.open_zoom_on(Some((items, at)), picture, window, cx);
    }

    /// The large view's previous or next plot.
    pub(super) fn step_zoom(&mut self, by: isize, window: &mut Window, cx: &mut Context<Self>) {
        match self.plot_history.zoom.as_mut().and_then(|z| z.only.take()) {
            Some((items, at)) => {
                let to = (at as isize + by).clamp(0, items.len() as isize - 1) as usize;
                self.open_zoom_on(Some((items, to)), None, window, cx);
            }
            None => {
                let (items, at) = self.zoom_plots();
                let Some(at) = at else { return };
                let to = (at as isize + by).clamp(0, items.len() as isize - 1) as usize;
                self.plot_history.picked = Some(items[to].key.clone());
                self.open_zoom(window, cx);
            }
        }
    }

    /// Opens the plot in front large, asking its session whether it can
    /// draw it again at any size.
    pub(crate) fn open_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_zoom_on(None, None, window, cx);
    }

    pub(super) fn open_zoom_on(
        &mut self,
        only: Option<(Vec<PlotItem>, usize)>,
        source_picture: Option<Size<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (item, image, index, name) = {
            let (list, at) = match &only {
                Some((items, at)) => (items.iter().collect::<Vec<_>>(), Some(*at)),
                None => self.plots_now(),
            };
            let Some(at) = at else {
                return;
            };
            let item = list[at].clone();
            let (image, index) = redraw_target(&item, &list);
            let name = plot_name(&item, &list);
            (item, image, index, name)
        };
        let project = self.project.as_deref().unwrap_or_default();
        let folder = crate::prefs::plot_folder(&self.host(), project)
            .unwrap_or_else(|| format!("{}/{FOLDER}", project.trim_end_matches('/')));
        let format = plain_formats(&item.mime)[0];
        let (unit, dpi) = crate::prefs::plot_export(&self.host(), project);
        let picture = source_picture
            .or_else(|| {
                self.plot_history
                    .image_bounds
                    .borrow()
                    .get(&plot_key(&item.value))
                    .copied()
            })
            .unwrap_or(size(px(614.), px(461.)));
        let previous = self.plot_history.zoom.as_ref();
        let card_size = previous.and_then(|zoom| zoom.size).or(Some(size(
            (picture.width + px(2. * look::PANE_PAD_X)).max(px(720.)),
            picture.height + px(256.),
        )));
        let offset = previous.map_or(point(px(0.), px(0.)), |zoom| zoom.offset);
        let physical = match self.plot_history.sizing {
            PlotSizing::Fixed { width, height, .. } => (width, height),
            PlotSizing::Fit => window_inches(
                f64::from(f32::from(picture.width)),
                f64::from(f32::from(picture.height)),
            ),
        };
        let width = cx.new(|cx| {
            InputState::new(window, cx).default_value(dimension_text(physical.0, unit, dpi))
        });
        let height = cx.new(|cx| {
            InputState::new(window, cx).default_value(dimension_text(physical.1, unit, dpi))
        });
        let dpi_input = cx.new(|cx| InputState::new(window, cx).default_value(dpi.to_string()));
        let name = cx.new(|cx| {
            InputState::new(window, cx).default_value(format!("{name}.{}", extension(format)))
        });
        let mut subscriptions = Vec::new();
        for input in [&width, &height] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.zoom_dimensions_changed(window, cx);
                    }
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &dpi_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.zoom_dpi_changed(window, cx);
                }
            },
        ));
        self.plot_history.zoom = Some(Zoom {
            redraw: None,
            key: item.key.clone(),
            retained: only.is_none().then(|| item.clone()),
            preview: only.is_none().then(|| self.fitted_image(&item)).flatten(),
            preview_note: None,
            room: Rc::new(Cell::new(None)),
            fitting: None,
            size: card_size,
            resizing: None,
            offset,
            moving: None,
            initial_picture: Some(picture),
            follow: matches!(self.plot_history.sizing, PlotSizing::Fit),
            redraw_preview: true,
            unit,
            units_open: false,
            rasterize: self
                .project
                .as_ref()
                .is_some_and(|project| crate::prefs::plot_rasterize(&self.host(), project)),
            settings: crate::prefs::plot_settings_shown(),
            physical,
            folder,
            folder_dialog: None,
            _subscriptions: subscriptions,
            format,
            width,
            height,
            name,
            dpi,
            dpi_input,
            note: None,
            replace: None,
            working: false,
            only,
        });
        let Some(daemon) = self.daemon() else {
            if let Some(zoom) = &mut self.plot_history.zoom {
                zoom.redraw = Some(PlotRedraw {
                    can: false,
                    reason: Some("reconnect to redraw this plot".into()),
                    png: None,
                });
                zoom.redraw_preview = false;
            }
            cx.notify();
            return;
        };
        let asked = daemon.call::<PlotRedraw>(Call::RedrawPlot {
            session: item.session.clone(),
            run: item.run.clone(),
            image,
            index,
            file: None,
            format: String::new(),
            width: 0.,
            height: 0.,
            dpi: 0,
            show: false,
            rasterize: false,
        });
        let room = self.plot_history.zoom.as_ref().unwrap().room.clone();
        cx.spawn_in(window, async move |this, cx| {
            let answer = asked.await.unwrap_or_else(|err| PlotRedraw {
                can: false,
                reason: Some(format!("{err:#}")),
                png: None,
            });
            this.update_in(cx, |this, window, cx| {
                let Some(zoom) = this.plot_history.zoom.as_mut() else {
                    return;
                };
                if !Rc::ptr_eq(&zoom.room, &room) {
                    return;
                }
                let can_redraw = answer.can;
                zoom.redraw_preview &= answer.can;
                zoom.redraw = Some(answer);
                if can_redraw {
                    this.set_zoom_format("png", window, cx);
                }
                this.fit_zoom(room, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
    pub(crate) fn dismiss_zoom_units(&mut self) -> bool {
        self.plot_history
            .zoom
            .as_mut()
            .is_some_and(|zoom| std::mem::take(&mut zoom.units_open))
    }

    pub(super) fn sync_zoom_dimensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let width = dimension_text(zoom.physical.0, zoom.unit, zoom.dpi);
        let height = dimension_text(zoom.physical.1, zoom.unit, zoom.dpi);
        if zoom.width.read(cx).value().as_str() != width {
            zoom.width
                .update(cx, |input, cx| input.set_value(width, window, cx));
        }
        if zoom.height.read(cx).value().as_str() != height {
            zoom.height
                .update(cx, |input, cx| input.set_value(height, window, cx));
        }
        cx.notify();
    }

    pub(super) fn remember_zoom_sizing(&mut self, cx: &mut Context<Self>) {
        let Some(zoom) = &self.plot_history.zoom else {
            return;
        };
        let sizing = if zoom.follow {
            PlotSizing::Fit
        } else {
            PlotSizing::Fixed {
                width: zoom.physical.0,
                height: zoom.physical.1,
                dpi: zoom.dpi,
            }
        };
        if let Some(project) = &self.project {
            crate::prefs::save_plot_sizing(&self.host(), project, sizing);
            crate::prefs::save_plot_export(&self.host(), project, zoom.unit, zoom.dpi);
        }
        self.plot_history.sizing = sizing;
        cx.notify();
    }

    fn zoom_dimensions_changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let width = zoom.width.read(cx).value().to_string();
        let height = zoom.height.read(cx).value().to_string();
        // Programmatic conversions and follow-window updates emit Change too.
        if width == dimension_text(zoom.physical.0, zoom.unit, zoom.dpi)
            && height == dimension_text(zoom.physical.1, zoom.unit, zoom.dpi)
        {
            return;
        }
        zoom.follow = false;
        if let Some((width, height)) = width
            .trim()
            .parse::<f64>()
            .ok()
            .zip(height.trim().parse::<f64>().ok())
        {
            let physical = (
                zoom.unit.inches(width, zoom.dpi),
                zoom.unit.inches(height, zoom.dpi),
            );
            if (PlotSizing::Fixed {
                width: physical.0,
                height: physical.1,
                dpi: zoom.dpi,
            })
            .pixels()
            .is_some()
            {
                zoom.physical = physical;
                self.remember_zoom_sizing(cx);
            }
        }
        cx.notify();
    }

    fn zoom_dpi_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zoom) = &mut self.plot_history.zoom else {
            return;
        };
        let Ok(dpi) = zoom.dpi_input.read(cx).value().trim().parse::<u32>() else {
            return;
        };
        if dpi == 0 || dpi > 30000 || dpi == zoom.dpi {
            return;
        }
        zoom.dpi = dpi;
        self.sync_zoom_dimensions(window, cx);
        self.remember_zoom_sizing(cx);
    }

    /// One request at a time. Resizes during a render are coalesced, and an
    /// obsolete reply never replaces the plot in the current preview.
    pub(super) fn fit_zoom(&mut self, room: Rc<Cell<Option<PreviewSize>>>, cx: &mut Context<Self>) {
        let (list, at) = self.zoom_plots();
        let Some(item) = at.map(|at| list[at]) else {
            return;
        };
        let (image, index) = redraw_target(item, &list);
        let (session, run) = (item.session.clone(), item.run.clone());
        let Some(daemon) = self.daemon() else {
            return;
        };
        let generation = self.generation;
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        if !Rc::ptr_eq(&zoom.room, &room)
            || room.get().is_none()
            || zoom.fitting.is_some()
            || !zoom.redraw_preview
            // Until the window is resized the picture keeps its original
            // pixels, so it looks exactly as it did before zooming.
            || zoom.initial_picture.is_some()
            || !zoom.redraw.as_ref().is_some_and(|r| r.can)
        {
            return;
        }
        zoom.preview_note = Some("Waiting for the preview size to settle…".into());
        zoom.fitting = Some(cx.spawn(async move |this, cx| {
            loop {
                let Some(size) = room.get() else {
                    return;
                };
                cx.background_executor().timer(FIT_DELAY).await;
                if room.get() != Some(size) {
                    continue;
                }
                let current = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    let Some(zoom) = this.plot_history.zoom.as_mut() else {
                        return false;
                    };
                    if !Rc::ptr_eq(&zoom.room, &room) || !zoom.redraw_preview {
                        return false;
                    }
                    zoom.preview_note = Some("Redrawing preview…".into());
                    cx.notify();
                    true
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
                let answer = daemon
                    .call::<PlotRedraw>(Call::RedrawPlot {
                        session: session.clone(),
                        run: run.clone(),
                        image: image.clone(),
                        index,
                        file: None,
                        format: String::new(),
                        width: size.width / f64::from(POINTS),
                        height: size.height / f64::from(POINTS),
                        dpi: size.dpi,
                        show: true,
                        rasterize: false,
                    })
                    .await;
                let again = this
                    .update(cx, |this, cx| {
                        if this.generation != generation {
                            return false;
                        }
                        let Some(zoom) = this.plot_history.zoom.as_mut() else {
                            return false;
                        };
                        if !Rc::ptr_eq(&zoom.room, &room) || !zoom.redraw_preview {
                            return false;
                        }
                        if room.get() != Some(size) {
                            return true;
                        }
                        let preview = answer.and_then(|reply| {
                            if !reply.can {
                                zoom.redraw_preview = false;
                                zoom.redraw = Some(reply.clone());
                            }
                            let png = reply.png.ok_or_else(|| {
                                anyhow::anyhow!(
                                    "{}",
                                    reply
                                        .reason
                                        .as_deref()
                                        .unwrap_or("no redrawn image was returned")
                                )
                            })?;
                            let bytes = sandhill_proto::decode_base64(png.trim())?;
                            Ok(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)))
                        });
                        match preview {
                            Ok(image) => {
                                zoom.preview = Some(image);
                                zoom.preview_note = None;
                            }
                            Err(err) => {
                                zoom.preview_note = Some(format!("Preview unchanged: {err:#}"))
                            }
                        }
                        zoom.fitting = None;
                        cx.notify();
                        false
                    })
                    .unwrap_or(false);
                if !again {
                    break;
                }
            }
        }));
    }

    pub(super) fn zoom_room_watcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let room = self.plot_history.zoom.as_ref().unwrap().room.clone();
        let this = cx.weak_entity();
        let sizing = PlotSizing::Fit;
        let resize_this = this.clone();
        canvas(
            move |bounds, window, cx| {
                let size = PreviewSize {
                    width: f64::from(f32::from(bounds.size.width)),
                    height: f64::from(f32::from(bounds.size.height)),
                    dpi: (POINTS * window.scale_factor().clamp(1., 4.)).round() as u32,
                };
                if (matches!(sizing, PlotSizing::Fit)
                    && (size.width < f64::from(MIN_FIT) || size.height < f64::from(MIN_FIT)))
                    || !preview_size_changed(room.get(), size, sizing)
                {
                    return;
                }
                room.set(Some(size));
                let (this, room) = (this.clone(), room.clone());
                window.defer(cx, move |window, cx| {
                    this.update(cx, |this, cx| {
                        if let Some(zoom) = &mut this.plot_history.zoom
                            && Rc::ptr_eq(&zoom.room, &room)
                            && zoom.follow
                        {
                            zoom.physical = window_inches(size.width, size.height);
                            this.sync_zoom_dimensions(window, cx);
                        }
                        this.fit_zoom(room, cx);
                        cx.notify();
                    })
                    .ok();
                });
            },
            move |_, _, window, _| {
                let this = resize_this.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    this.update(cx, |this, cx| {
                        let Some(zoom) = this.plot_history.zoom.as_mut() else {
                            return;
                        };
                        let viewport = window.viewport_size();
                        if let Some((start, original)) = zoom.moving {
                            if !event.dragging() {
                                zoom.moving = None;
                                return;
                            }
                            let card = zoom.size.unwrap_or(size(px(720.), px(600.)));
                            zoom.offset = clamp_zoom_offset(
                                original + (event.position - start),
                                card,
                                viewport,
                            );
                        } else if let Some((start, original)) = zoom.resizing {
                            if !event.dragging() {
                                zoom.resizing = None;
                                return;
                            }
                            let max = size(viewport.width * 0.96, viewport.height * 0.96);
                            let delta = event.position - start;
                            // The card remains centred on its translated centre. Each
                            // edge moves half the size delta, hence the factor of two.
                            let resized = size(
                                (original.width + delta.x * 2.)
                                    .clamp(px(720.).min(max.width), max.width),
                                (original.height + delta.y * 2.)
                                    .clamp(px(340.).min(max.height), max.height),
                            );
                            zoom.size = Some(resized);
                            zoom.initial_picture = None;
                            zoom.offset = clamp_zoom_offset(zoom.offset, resized, viewport);
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })
                    .ok();
                });
                let this = resize_this.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        this.update(cx, |this, cx| {
                            if let Some(zoom) = this.plot_history.zoom.as_mut() {
                                let resized = zoom.resizing.take().is_some();
                                let moved = zoom.moving.take().is_some();
                                if resized || moved {
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }
                        })
                        .ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }

    pub(super) fn set_zoom_format(
        &mut self,
        format: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        zoom.format = format;
        zoom.replace = None;
        // The typed name takes the new extension.
        let typed = zoom.name.read(cx).value().to_string();
        let stem = match typed.rsplit_once('.') {
            Some((stem, ext)) if !ext.contains('/') => stem.to_string(),
            _ => typed,
        };
        let renamed = format!("{stem}.{}", extension(format));
        zoom.name
            .update(cx, |i, cx| i.set_value(renamed, window, cx));
        cx.notify();
    }
}
