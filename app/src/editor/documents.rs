//! Standalone source documents and explicit system-app handoff.

use std::rc::Rc;

use anyhow::{Result, ensure};
use base64::Engine;
use gpui_kit::component::button::Button;
use gpui_kit::component::{Sizable, h_flex};
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{Call, FileBytes, FileText, MAX_READ_BYTES};

use crate::daemon::Daemon;
use crate::editor::delimited::TEXT_LIMIT;
use crate::editor::notebook;
use crate::look::{self, Palette};
use crate::workbench::{Open, Workbench};

/// The last codepoint may be cut by the prefix request, but invalid UTF-8
/// anywhere else and NULs identify a binary-looking file.
fn text_prefix(bytes: &[u8], complete: bool) -> bool {
    !bytes.contains(&0)
        && match std::str::from_utf8(bytes) {
            Ok(_) => true,
            Err(error) => !complete && error.error_len().is_none(),
        }
}

/// PLINK's binary genotypes begin with these two bytes. They are `.bed`
/// files, as BED intervals are, and small ones can be free of NULs and
/// invalid UTF-8, so the prefix alone would pass for text.
const PLINK_BED: [u8; 2] = [0x6c, 0x1b];

/// Whether a file beginning with `bytes` opens as text.
fn opens_as_text(bytes: &[u8], complete: bool) -> bool {
    !bytes.starts_with(&PLINK_BED) && text_prefix(bytes, complete)
}

fn trim_preview(bytes: &mut Vec<u8>) -> Result<()> {
    let end = match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => error.valid_up_to(),
        Err(error) => return Err(error.into()),
    };
    bytes.truncate(end);
    // Keep a complete last line when there is one; a single huge line still
    // shows its bounded prefix rather than a blank editor.
    if let Some(newline) = bytes.iter().rposition(|byte| *byte == b'\n') {
        bytes.truncate(newline + 1);
    }
    Ok(())
}

/// Keep small-file exact reads on the established bounded-read path. Only a
/// known text file may be truncated; unknown large files use the external app.
pub(crate) async fn read_text_preview(
    daemon: Rc<Daemon>,
    path: String,
) -> Result<(FileText, bool)> {
    let before: FileBytes = daemon
        .call(Call::ReadBytes {
            path: path.clone(),
            offset: 0,
            limit: Some(0),
        })
        .await?;
    ensure!(
        before.path == path && before.offset == 0 && before.data.is_empty(),
        "Invalid file metadata response"
    );
    if before.size <= TEXT_LIMIT {
        return Ok((daemon.read_text_bounded(path, TEXT_LIMIT).await?, false));
    }
    let mut bytes = Vec::with_capacity(TEXT_LIMIT as usize);
    while (bytes.len() as u64) < TEXT_LIMIT {
        let offset = bytes.len() as u64;
        let count = (TEXT_LIMIT - offset).min(MAX_READ_BYTES);
        let part: FileBytes = daemon
            .call(Call::ReadBytes {
                path: path.clone(),
                offset,
                limit: Some(count),
            })
            .await?;
        ensure!(
            part.path == path
                && part.offset == offset
                && part.size == before.size
                && part.mtime_ms == before.mtime_ms,
            "File changed while it was being read; try again"
        );
        ensure!(
            part.data.len() as u64 == count.div_ceil(3) * 4,
            "Invalid file chunk length"
        );
        let decoded = base64::engine::general_purpose::STANDARD.decode(part.data.trim())?;
        ensure!(decoded.len() as u64 == count, "File read was truncated");
        bytes.extend_from_slice(&decoded);
    }
    let after: FileBytes = daemon
        .call(Call::ReadBytes {
            path: path.clone(),
            offset: 0,
            limit: Some(0),
        })
        .await?;
    ensure!(
        after.path == path
            && after.offset == 0
            && after.data.is_empty()
            && after.size == before.size
            && after.mtime_ms == before.mtime_ms,
        "File changed while it was being read; try again"
    );
    trim_preview(&mut bytes)?;
    Ok((
        FileText {
            path,
            text: String::from_utf8(bytes)?,
            mtime_ms: before.mtime_ms,
        },
        true,
    ))
}

impl Workbench {
    pub(crate) fn open_shell_or_external(
        &mut self,
        path: String,
        rel: String,
        cx: &mut Context<Self>,
    ) {
        if notebook::is_binary(&path) {
            self.offer_external(path, cx);
            return;
        }
        let Some(daemon) = self.daemon() else { return };
        let generation = self.generation;
        let project = self.project.clone();
        let known = notebook::format_of(&path).is_some();
        let load = self.next_document_load;
        self.next_document_load += 1;
        self.document_loads.insert(path.clone(), load);
        self.put_away();
        self.explorer_shown = false;
        self.diff_shown = false;
        self.problem = None;
        self.tabs.push(path.clone());
        self.opening_shown = Some(path.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let read: Result<Option<(notebook::Notebook, i64)>> = async {
                // A file of a known text type is read as text unasked, but
                // one whose extension binary files share is looked at first.
                let unsure = notebook::shared_with_binary(&path);
                if !known || unsure {
                    let prefix: FileBytes = daemon
                        .call(Call::ReadBytes {
                            path: path.clone(),
                            offset: 0,
                            limit: Some(8192),
                        })
                        .await?;
                    ensure!(
                        prefix.path == path
                            && prefix.offset == 0
                            && prefix.data.len() <= 8192_usize.div_ceil(3) * 4,
                        "Invalid file prefix response"
                    );
                    let bytes =
                        base64::engine::general_purpose::STANDARD.decode(prefix.data.trim())?;
                    ensure!(
                        bytes.len() as u64 == prefix.size.min(8192),
                        "Incomplete file prefix response"
                    );
                    // A known type over the limit still shows its first part.
                    if (!known && prefix.size > TEXT_LIMIT)
                        || !opens_as_text(&bytes, prefix.size <= bytes.len() as u64)
                    {
                        return Ok(None);
                    }
                }
                let (file, truncated) = read_text_preview(daemon, path.clone()).await?;
                cx.background_spawn(async move {
                    let mut nb = notebook::parse(&file.path, &file.text)?;
                    nb.truncated = truncated;
                    Ok(Some((nb, file.mtime_ms)))
                })
                .await
            }
            .await;
            this.update_in(cx, |this, window, cx| {
                if this.document_loads.get(&path) != Some(&load) {
                    return;
                }
                this.document_loads.remove(&path);
                if this.generation != generation || this.project != project {
                    return;
                }
                let selected = this.opening_shown.as_deref() == Some(&path);
                match read {
                    Ok(Some((nb, mtime))) => {
                        let previous = this.open.take();
                        if selected {
                            this.opening_shown = None;
                            this.notebook_scroll.set_offset(Point::default());
                        }
                        this.show_notebook(nb, mtime, window, cx);
                        if selected {
                            this.session_changed(cx);
                            this.take_pending_location(window, cx);
                        } else {
                            this.others
                                .push(this.open.take().expect("document just constructed"));
                            this.open = previous;
                        }
                    }
                    outcome => {
                        this.tabs.retain(|tab| tab != &path);
                        if selected {
                            this.opening_shown = None;
                            if let Some(next) = this.tabs.last().cloned() {
                                this.show_file_tab(&next, cx);
                            }
                        }
                        match outcome {
                            Ok(None) => this.offer_external(path, cx),
                            Err(error) => {
                                this.problem = Some(format!("Opening {rel} failed: {error:#}"))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_external(&mut self, path: String, cx: &mut Context<Self>) {
        if self.target != Target::Local {
            self.download_and_open(path, cx);
        } else {
            cx.open_with_system(std::path::Path::new(&path));
        }
    }

    pub(crate) fn render_truncated_note(
        &self,
        open: &Open,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !open.notebook.truncated {
            return None;
        }
        let p = *Palette::get(cx);
        let path = open.notebook.path.clone();
        Some(
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .text_size(px(look::small()))
                .text_color(p.muted)
                .bg(p.chrome)
                .child("Read-only: showing the first 5 MB of this file.")
                .child(
                    Button::new("truncated-open-external")
                        .small()
                        .label(if self.target == Target::Local {
                            "Open with Default App"
                        } else {
                            "Download to This Mac and Open"
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_external(path.clone(), cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn offer_external(&mut self, path: String, cx: &mut Context<Self>) {
        let remote = self.target != Target::Local;
        let generation = self.generation;
        let project = self.project.clone();
        cx.spawn(async move |this, cx| {
            let title = format!("Open “{}” with its default app?", crate::panes::files::base_name(&path));
            let detail = if remote {
                "This file is on the server. Download a copy to this Mac and open that copy with its default app? No application will run on the server."
            } else {
                "This file is not supported by the editor. Open it with this Mac's default application?"
            };
            let action = if remote { "Download to This Mac and Open" } else { "Open with Default App" };
            let Ok(answer) = this.update_in(cx, |_, window, cx| {
                window.prompt(PromptLevel::Info, &title, Some(detail), &[action, "Cancel"], cx)
            }) else { return };
            if answer.await != Ok(0) { return; }
            this.update(cx, |this, cx| {
                if this.generation != generation || this.project != project { return; }
                this.open_external(path, cx);
            }).ok();
        }).detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{opens_as_text, text_prefix, trim_preview};

    #[test]
    fn plink_genotypes_are_not_text_but_bed_intervals_are() {
        // Magic, the SNP-major mode, then genotypes that are valid ASCII.
        assert!(!opens_as_text(&[0x6c, 0x1b, 0x01, 0x55, 0x55], true));
        assert!(opens_as_text(b"chr1\t10\t20\n", true));
        assert!(opens_as_text(b"track name=l\n", true));
        assert!(!opens_as_text(&[0xff, 0xfe], true));
    }

    #[test]
    fn prefix_accepts_text_and_only_a_cut_final_codepoint() {
        assert!(text_prefix(b"", true));
        assert!(text_prefix("hello\nα".as_bytes(), true));
        assert!(text_prefix(&[b'a', 0xe2, 0x82], false));
        assert!(!text_prefix(&[b'a', 0xe2, 0x82], true));
        assert!(!text_prefix(&[0xff, b'a'], false));
        assert!(!text_prefix(b"abc\0def", false));
    }

    #[test]
    fn preview_ends_on_a_complete_line_or_codepoint() {
        let mut bytes = b"first\r\npartial\xe2\x82".to_vec();
        trim_preview(&mut bytes).unwrap();
        assert_eq!(bytes, b"first\r\n");
        let mut bytes = b"single line\xe2\x82".to_vec();
        trim_preview(&mut bytes).unwrap();
        assert_eq!(bytes, b"single line");
        assert!(trim_preview(&mut vec![0xff]).is_err());
    }
}
