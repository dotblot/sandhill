//! The same archive checklist in Welcome and an open Explore window.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sandhill_proto::{Call, QuickArchiveMember, QuickArchiveSelection};

use crate::daemon::Daemon;
use crate::look::{self, Palette};

pub(crate) const TABLE_TYPES: &str = "Sandhill explores CSV, TSV, text and Excel tables, including gzipped tables and tables in ZIP or TAR archives.";

struct Archive {
    path: String,
    members: Vec<QuickArchiveMember>,
    checked: Vec<bool>,
}

pub(crate) struct ArchivePicker {
    pub tables: Vec<String>,
    pub sample_sheet: bool,
    archives: Vec<Archive>,
}

impl ArchivePicker {
    pub async fn read(
        daemon: Rc<Daemon>,
        tables: Vec<String>,
        paths: Vec<String>,
        sample_sheet: bool,
    ) -> anyhow::Result<Self> {
        let mut archives = Vec::with_capacity(paths.len());
        let mut total = 0;
        for path in paths {
            let members = daemon
                .call::<Vec<QuickArchiveMember>>(Call::QuickListArchive { path: path.clone() })
                .await?;
            total += members.len();
            archives.push(Archive {
                path,
                members,
                checked: Vec::new(),
            });
        }
        if total == 0 && tables.is_empty() {
            anyhow::bail!("The archive has no tables Sandhill can open.");
        }
        let check = total <= if sample_sheet { 1 } else { 12 };
        for archive in &mut archives {
            archive.checked.resize(archive.members.len(), check);
        }
        Ok(Self {
            tables,
            sample_sheet,
            archives,
        })
    }

    pub fn into_selection(self) -> (Vec<String>, Vec<QuickArchiveSelection>, bool) {
        let archives = self
            .archives
            .into_iter()
            .filter_map(|archive| {
                let members: Vec<_> = archive
                    .members
                    .into_iter()
                    .zip(archive.checked)
                    .filter_map(|(m, on)| on.then_some(m))
                    .collect();
                (!members.is_empty()).then_some(QuickArchiveSelection {
                    path: archive.path,
                    members,
                })
            })
            .collect();
        (self.tables, archives, self.sample_sheet)
    }

    fn stats(&self) -> (usize, usize, bool) {
        let mut total = 0;
        let mut selected = 0;
        let mut all_counts = true;
        for archive in &self.archives {
            total += archive.members.len();
            for (m, on) in archive.members.iter().zip(&archive.checked) {
                if *on {
                    selected += 1;
                    all_counts &= m.sample_counts;
                }
            }
        }
        (total, selected, all_counts)
    }

    pub fn render<V: 'static>(
        &self,
        p: &Palette,
        state: fn(&mut V) -> &mut Option<Self>,
        open: fn(&mut V, bool, &mut Context<V>),
        cx: &mut Context<V>,
    ) -> AnyElement {
        let (total, selected, all_counts) = self.stats();
        let sample_sheet = self.sample_sheet;
        let open_ok = if sample_sheet {
            selected == 1
        } else {
            selected > 0 || !self.tables.is_empty()
        };
        let can_combine = !sample_sheet && selected >= 2 && all_counts;
        let list = v_flex()
            .id("quick-archive-members")
            .w_full()
            .max_h(px(look::row() * 8.))
            .overflow_y_scroll()
            .gap(px(look::GAP_8))
            .children(self.archives.iter().enumerate().map(|(a, archive)| {
                let name = crate::panes::files::base_name(&archive.path).to_owned();
                v_flex()
                    .gap(px(look::GAP_4))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(name))
                    .when(archive.members.is_empty(), |d| {
                        d.child(div().text_color(p.muted).child("No tables found."))
                    })
                    .children(archive.members.iter().enumerate().map(|(m, member)| {
                        div().min_w_0().child(
                            Checkbox::new(SharedString::from(format!("quick-archive-{a}-{m}")))
                                .label(member.name.clone())
                                .checked(archive.checked[m])
                                .on_click(cx.listener(move |this, on: &bool, _, cx| {
                                    if let Some(picker) = state(this) {
                                        if sample_sheet && *on {
                                            for archive in &mut picker.archives {
                                                archive.checked.fill(false);
                                            }
                                        }
                                        picker.archives[a].checked[m] = *on;
                                    }
                                    cx.notify();
                                })),
                        )
                    }))
            }));
        v_flex()
            .id("quick-archive-card")
            .occlude()
            .w_full()
            .p(px(look::PANE_PAD_X))
            .gap(px(look::GAP_8))
            .rounded(px(look::RADIUS))
            .border_1()
            .border_color(p.glass_edge)
            .bg(p.panel)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(look::title()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if sample_sheet {
                        "Choose a Sample Sheet"
                    } else {
                        "Choose Tables from the Archive"
                    }),
            )
            .child(
                div().text_color(p.muted).child(
                    "Sandhill copies only the tables you choose. The archive stays as it is.",
                ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap(px(look::GAP_8))
                    .child(
                        div()
                            .text_color(p.muted)
                            .child(format!("{selected} of {total} selected")),
                    )
                    .when(!sample_sheet && total > 1, |d| {
                        d.child(
                            Button::new("quick-archive-all")
                                .small()
                                .label(if selected == total {
                                    "Clear Selection"
                                } else {
                                    "Select All"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(picker) = state(this) {
                                        for archive in &mut picker.archives {
                                            archive.checked.fill(selected != total);
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(list)
            .when(can_combine, |d| {
                d.child(div().text_color(p.muted).child(
                    "To combine samples, Sandhill keeps only the genes found in every file.",
                ))
            })
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .child(
                        Button::new("quick-archive-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                *state(this) = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("quick-archive-open")
                            .small()
                            .primary()
                            .label(if sample_sheet {
                                "Import Sample Sheet"
                            } else {
                                "Open Tables"
                            })
                            .disabled(!open_ok)
                            .on_click(cx.listener(move |this, _, _, cx| open(this, false, cx))),
                    )
                    .when(can_combine, |d| {
                        d.child(
                            Button::new("quick-archive-combine")
                                .small()
                                .label("Combine into One Counts Table")
                                .on_click(cx.listener(move |this, _, _, cx| open(this, true, cx))),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl crate::workbench::Workbench {
    pub(super) fn render_quick_archive(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let picker = self.quick.as_ref()?.archive_picker.as_ref()?;
        let p = *Palette::get(cx);
        let card = picker.render(
            &p,
            |this: &mut Self| {
                &mut this
                    .quick
                    .as_mut()
                    .expect("Explore archive picker")
                    .archive_picker
            },
            Self::quick_open_archive_selection,
            cx,
        );
        Some(
            deferred(
                div()
                    .id("quick-archive-backdrop")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(
                        div()
                            .w(px(look::row() * 23.))
                            .max_w(relative(0.95))
                            .shadow_lg()
                            .child(card),
                    ),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}
