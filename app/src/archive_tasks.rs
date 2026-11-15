//! Compressing and extracting from the Files menu, with their progress in
//! the status bar: a bar, how far, and Cancel. The daemon does the work
//! and reports how many input bytes it has read; the app asks four times a
//! second while it runs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{ArchiveFormat, ArchiveProgress, CANCELLED_ERROR, Call, FileWritten};

use crate::files::{base_name, join_path};
use crate::look::{self, Palette, bar_button};
use crate::workbench::Workbench;

/// How often a running task is asked how far it is.
const POLL: Duration = Duration::from_millis(250);
/// The status bar's progress bar.
const BAR_WIDTH: f32 = 120.;

/// A compression or extraction under way.
pub(crate) struct ArchiveTask {
    id: String,
    /// "Compressing reads.fastq".
    label: String,
    done: u64,
    total: u64,
    cancelling: bool,
}

impl Workbench {
    /// Packs `rel` into an archive beside it (`format`), or unpacks the
    /// archive it is (none), in the daemon, keeping the original.
    pub(crate) fn archive(
        &mut self,
        rel: String,
        format: Option<ArchiveFormat>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let path = join_path(&project, &rel);
        let name = base_name(&path).to_string();
        // Only what a compression or a gzip file's decompression makes is
        // surely a file; a zip's contents may be a folder.
        let lower = name.to_ascii_lowercase();
        let single_gzip = [".gz", ".bgz"].iter().any(|e| lower.ends_with(e))
            && ![".tar.gz", ".tgz"].iter().any(|e| lower.ends_with(e));
        let file_out = format.is_some() || single_gzip;
        let (doing, done, verb) = match format {
            Some(_) => ("Compressing", "Compressed", "compressing"),
            None if single_gzip => ("Decompressing", "Decompressed", "decompressing"),
            None => ("Extracting", "Extracted", "extracting"),
        };
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = format!(
            "archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        self.archive_tasks.push(ArchiveTask {
            id: id.clone(),
            label: format!("{doing} {name}"),
            done: 0,
            total: 0,
            cancelling: false,
        });
        self.problem = None;
        self.notice = None;
        cx.notify();
        let task = Some(id.clone());
        let call = match format {
            Some(format) => Call::CompressPath { path, format, task },
            None => Call::ExtractPath { path, task },
        };
        let answer = daemon.call::<FileWritten>(call);
        cx.spawn({
            let id = id.clone();
            async move |this, cx| {
                let answer = answer.await;
                this.update(cx, |this, cx| {
                    this.archive_tasks.retain(|t| t.id != id);
                    match answer {
                        Ok(written) => {
                            let made = base_name(&written.path).to_string();
                            this.notice = Some(format!("{done} {name} to {made}"));
                            if file_out
                                && let Some(rel) = written.path.strip_prefix(&format!("{project}/"))
                            {
                                this.files_ui.selected = Some((rel.to_string(), false));
                            }
                            this.refresh_files(cx);
                        }
                        Err(err) if format!("{err:#}").contains(CANCELLED_ERROR) => {
                            this.notice = Some(format!("Stopped {verb} {name}"));
                        }
                        Err(err) => {
                            this.problem = Some(format!("Could not finish {verb} {name}: {err:#}"))
                        }
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
        // Asks how far it is until it has finished; an older daemon that
        // can't say leaves the bar empty.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let running = this
                    .update(cx, |this, _| this.archive_tasks.iter().any(|t| t.id == id))
                    .unwrap_or(false);
                if !running {
                    break;
                }
                let Ok(Some(progress)) = daemon
                    .call::<Option<ArchiveProgress>>(Call::ArchiveProgress { task: id.clone() })
                    .await
                else {
                    continue;
                };
                let updated = this.update(cx, |this, cx| {
                    if let Some(task) = this.archive_tasks.iter_mut().find(|t| t.id == id) {
                        task.done = progress.done;
                        task.total = progress.total;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn cancel_archive(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(task) = self.archive_tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        task.cancelling = true;
        cx.notify();
        let cancelled = daemon.call::<bool>(Call::CancelArchive {
            task: id.to_string(),
        });
        cx.background_spawn(async move {
            let _ = cancelled.await;
        })
        .detach();
    }

    /// The latest task's progress in the status bar: its name, a bar, how
    /// far, and Cancel; "+1 more" when others run too.
    pub(crate) fn render_archive_progress(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let task = self.archive_tasks.last()?;
        let p = *Palette::get(cx);
        let share = if task.total > 0 {
            (task.done as f32 / task.total as f32).clamp(0., 1.)
        } else {
            0.
        };
        let more = self.archive_tasks.len() - 1;
        let id = task.id.clone();
        Some(
            h_flex()
                .flex_none()
                .gap(px(8.))
                .text_size(px(look::SMALL))
                .child(div().text_color(p.text).child(format!("{}…", task.label)))
                .child(
                    div()
                        .w(px(BAR_WIDTH))
                        .h(px(5.))
                        .rounded_full()
                        .bg(p.ink(12.))
                        .child(
                            div()
                                .h_full()
                                .w(px(BAR_WIDTH * share))
                                .rounded_full()
                                .bg(p.accent),
                        ),
                )
                .child(
                    div()
                        .min_w(px(32.))
                        .text_color(p.muted)
                        .child(format!("{}%", (share * 100.).floor() as u32)),
                )
                .when(more > 0, |d| {
                    d.child(div().text_color(p.muted).child(format!("+{more} more")))
                })
                .child(if task.cancelling {
                    div()
                        .text_color(p.muted)
                        .child("Stopping…")
                        .into_any_element()
                } else {
                    bar_button("archive-cancel", "Cancel", None, &p)
                        .h(px(18.))
                        .on_click(cx.listener(move |this, _, _, cx| this.cancel_archive(&id, cx)))
                        .into_any_element()
                })
                .into_any_element(),
        )
    }
}
