//! Saving plots and choosing their folder.

use super::*;

impl Workbench {
    pub(super) fn choose_plot_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(zoom) = &self.plot_history.zoom else {
            return;
        };
        if self.host() == sandhill_link::LOCAL {
            // This Mac's folders: the system's own window.
            let chosen = crate::folder_dialog::choose_local_folder("Choose", cx);
            cx.spawn(async move |this, cx| {
                if let Some(folder) = chosen.await {
                    this.update(cx, |this, cx| this.set_plot_folder(folder, cx))
                        .ok();
                }
            })
            .detach();
            return;
        }
        let start = Some(zoom.folder.clone());
        let places = self
            .project
            .iter()
            .map(|project| crate::folder_dialog::Place {
                label: project
                    .rsplit('/')
                    .next()
                    .unwrap_or(project)
                    .to_string()
                    .into(),
                path: project.clone(),
            })
            .collect();
        let dialog = cx.new(|cx| {
            crate::folder_dialog::FolderDialog::new(
                daemon,
                "Choose a Folder for Plots".into(),
                "Choose".into(),
                start,
                places,
                window,
                cx,
            )
        });
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &crate::folder_dialog::FolderDialogDone, _, cx| {
                if let Some(zoom) = &mut this.plot_history.zoom {
                    zoom.folder_dialog = None;
                }
                match &event.0 {
                    Some(folder) => this.set_plot_folder(folder.clone(), cx),
                    None => cx.notify(),
                }
            },
        );
        if let Some(zoom) = &mut self.plot_history.zoom {
            zoom.folder_dialog = Some(dialog);
            zoom._subscriptions.push(subscription);
        }
        cx.notify();
    }

    fn set_plot_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        let host = self.host();
        if let Some(zoom) = &mut self.plot_history.zoom {
            zoom.folder = folder.clone();
            zoom.replace = None;
            if let Some(project) = &self.project {
                crate::prefs::save_plot_folder(&host, project, &folder);
            }
        }
        cx.notify();
    }

    /// Saves the plot in the project's folder: drawn again by its session
    /// at the size asked when it can, else as it was drawn.
    pub(super) fn save_plot(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (list, at) = self.zoom_plots();
        let (Some(at), Some(daemon), Some(project)) = (at, self.daemon(), self.project.clone())
        else {
            return;
        };
        let item = list[at].clone();
        let (image, index) = redraw_target(&item, &list);
        let host = self.host();
        let Some(zoom) = self.plot_history.zoom.as_mut() else {
            return;
        };
        let typed = zoom.name.read(cx).value().trim().to_string();
        if typed.is_empty() || typed.contains('/') {
            zoom.note = Some("Enter a file name, and choose its folder separately.".into());
            cx.notify();
            return;
        }
        let folder = zoom.folder.clone();
        let mut path = format!("{}/{typed}", folder.trim_end_matches('/'));
        let redrawn = zoom.redraw.as_ref().is_some_and(|r| r.can);
        let original_format = plain_formats(&item.mime)[0];
        let format = zoom.format;
        if !redrawn {
            path = std::path::Path::new(&path)
                .with_extension(extension(original_format))
                .to_string_lossy()
                .into_owned();
        }
        let parsed = zoom
            .width
            .read(cx)
            .value()
            .trim()
            .parse::<f64>()
            .ok()
            .zip(zoom.height.read(cx).value().trim().parse::<f64>().ok())
            .zip(zoom.dpi_input.read(cx).value().trim().parse::<u32>().ok());
        let Some(((width, height), dpi)) = parsed else {
            zoom.note = Some("Enter positive dimensions and a whole-number dpi.".into());
            cx.notify();
            return;
        };
        let physical = (zoom.unit.inches(width, dpi), zoom.unit.inches(height, dpi));
        if (PlotSizing::Fixed {
            width: physical.0,
            height: physical.1,
            dpi,
        })
        .pixels()
        .is_none()
        {
            zoom.note = Some("Enter positive dimensions, at most 30,000 pixels per side.".into());
            cx.notify();
            return;
        }
        // Display rounding (especially after switching units) must never alter
        // the canonical physical size used for export.
        let physical = zoom.physical;
        let (width, height) = (physical.0 * f64::from(dpi), physical.1 * f64::from(dpi));
        let room = zoom.room.clone();
        let replace = zoom.replace.clone();
        zoom.working = true;
        zoom.note = None;
        crate::prefs::save_plot_folder(&host, &project, &folder);
        let stat = daemon.call::<Vec<FileStat>>(Call::StatFiles {
            paths: vec![path.clone()],
        });
        let blob = item.value[sandhill_proto::BLOB_KEY]
            .as_str()
            .map(str::to_string);
        let near = self.open.as_ref().map(|o| o.notebook.path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let mut unavailable = None;
            let outcome: anyhow::Result<Option<String>> = async {
                let exists = stat.await?.first().is_some_and(|s| s.mtime_ms.is_some());
                if exists && replace.as_deref() != Some(path.as_str()) {
                    return Ok(None);
                }
                let folder_stat = daemon
                    .call::<Vec<FileStat>>(Call::StatFiles {
                        paths: vec![folder.clone()],
                    })
                    .await?;
                if !folder_stat
                    .first()
                    .is_some_and(|stat| stat.mtime_ms.is_some())
                {
                    daemon.call::<Value>(Call::MakeDir { path: folder }).await?;
                }
                if redrawn {
                    let reply = daemon
                        .call::<PlotRedraw>(Call::RedrawPlot {
                            session: item.session.clone(),
                            run: item.run.clone(),
                            image,
                            index,
                            file: Some(path.clone()),
                            format: format.to_string(),
                            width: width / f64::from(dpi),
                            height: height / f64::from(dpi),
                            dpi,
                            show: false,
                        })
                        .await?;
                    if reply.can {
                        let size = if format == "png" {
                            format!("{width:.0} × {height:.0} px")
                        } else {
                            format!(
                                "{:.1} × {:.1} in",
                                width / f64::from(dpi),
                                height / f64::from(dpi)
                            )
                        };
                        return Ok(Some(size));
                    }
                    unavailable = Some(reply);
                    let original_path = std::path::Path::new(&path)
                        .with_extension(extension(original_format))
                        .to_string_lossy()
                        .into_owned();
                    if original_path != path {
                        path = original_path;
                        let exists = daemon
                            .call::<Vec<FileStat>>(Call::StatFiles {
                                paths: vec![path.clone()],
                            })
                            .await?
                            .first()
                            .is_some_and(|stat| stat.mtime_ms.is_some());
                        if exists && replace.as_deref() != Some(path.as_str()) {
                            return Ok(None);
                        }
                    }
                }
                // As drawn: the image's own text (base64 for a PNG).
                let text = match (&item.value, blob) {
                    (Value::String(text), _) => text.clone(),
                    (_, Some(hash)) => {
                        daemon
                            .call::<sandhill_proto::Blob>(Call::GetBlob { hash, near })
                            .await?
                            .data
                    }
                    _ => anyhow::bail!("the plot has no image"),
                };
                let base64 = item.mime != "image/svg+xml";
                daemon
                    .call::<Value>(Call::WriteFile {
                        path: path.clone(),
                        text,
                        expected_mtime_ms: None,
                        base64,
                        create: false,
                    })
                    .await?;
                Ok(Some("as drawn".into()))
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                let shown = this.relative_path(&path);
                if let Some(zoom) = this.plot_history.zoom.as_mut() {
                    if !Rc::ptr_eq(&zoom.room, &room) {
                        return;
                    }
                    zoom.working = false;
                    let used_original = !redrawn || unavailable.is_some();
                    if let Some(unavailable) = unavailable {
                        zoom.redraw = Some(unavailable);
                        zoom.redraw_preview = false;
                        zoom.fitting = None;
                        zoom.preview_note = None;
                    }
                    if used_original {
                        zoom.format = original_format;
                        if let Some(name) = std::path::Path::new(&path).file_name() {
                            zoom.name.update(cx, |input, cx| {
                                input.set_value(name.to_string_lossy().into_owned(), window, cx)
                            });
                        }
                    }
                    match outcome {
                        Ok(Some(size)) => {
                            zoom.replace = None;
                            zoom.note = Some(format!("Saved {shown}, {size}."));
                        }
                        Ok(None) => {
                            zoom.replace = Some(path.clone());
                            zoom.note = Some(format!(
                                "{shown} is already there. Save again to replace it."
                            ));
                        }
                        Err(err) => zoom.note = Some(format!("Could not save the plot: {err:#}")),
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A path shown relative to the project.
    pub(super) fn relative_path(&self, path: &str) -> String {
        match &self.project {
            Some(project) => path
                .strip_prefix(project.trim_end_matches('/'))
                .map_or(path, |rest| rest.trim_start_matches('/'))
                .to_string(),
            None => path.to_string(),
        }
    }
}
