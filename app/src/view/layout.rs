//! The workbench's column layout: panel widths, splitters and their dragging.

use super::*;

/// The height the left panel's two panes share: the window's, less the
/// title and status bars and the gaps.
fn left_column(height: f32) -> f32 {
    (height - look::TITLE_BAR - look::STATUS_BAR - 2. * GAP).max(0.)
}

/// The width the three panels share: the window's, less the room at its
/// edges and the gaps between them.
fn panel_room(width: f32) -> f32 {
    (width - 2. * EDGE - 2. * GAP).max(0.)
}

/// Shares refer to the normal three-column room, even with a column hidden.
fn column_widths(
    width: f32,
    left: bool,
    middle: bool,
    agents: bool,
    left_share: f32,
    agents_share: f32,
) -> (f32, f32, f32) {
    let count = left as u8 + middle as u8 + agents as u8;
    let available = (width - 2. * EDGE - GAP * count.saturating_sub(1) as f32).max(0.);
    if count == 0 {
        return (0., 0., 0.);
    }
    if !middle && !agents {
        return (available, 0., 0.);
    }
    if !middle {
        let l = if left {
            (panel_room(width) * left_share)
                .max(220.)
                .min((available - 300.).max(0.))
        } else {
            0.
        };
        return (l, 0., available - l);
    }
    let normal = (width - 2. * EDGE - GAP * (left as u8 + agents as u8) as f32).max(0.);
    let side_room = (normal - 360.).max(0.);
    let ai_min = if agents { 300_f32.min(side_room) } else { 0. };
    let l = if left {
        (panel_room(width) * left_share)
            .max(220.)
            .min((side_room - ai_min).max(0.))
    } else {
        0.
    };
    let a = if agents {
        (panel_room(width) * agents_share)
            .max(300.)
            .min((side_room - l).max(0.))
    } else {
        0.
    };
    (l, (available - l - a).max(0.), a)
}

impl Workbench {
    pub(crate) fn reveal_middle(&mut self) {
        self.show_middle = true;
        if self
            .maximized_panel
            .is_some_and(|panel| panel != Panel::Editor)
        {
            self.maximized_panel = None;
        }
    }

    pub(crate) fn toggle_column(&mut self, panel: Panel, window: &Window, cx: &mut Context<Self>) {
        let agents = self.show_agents && self.agents_available();
        let can_hide = match panel {
            Panel::LeftTop => self.show_middle || agents,
            Panel::Editor => self.show_files || agents,
            Panel::Agents => self.show_files || self.show_middle,
            _ => return,
        };
        let keep_left = panel == Panel::Editor && self.show_middle && self.show_files;
        let shown = match panel {
            Panel::LeftTop => &mut self.show_files,
            Panel::Editor => &mut self.show_middle,
            Panel::Agents => &mut self.show_agents,
            _ => return,
        };
        if !*shown || can_hide {
            // Preserve the actual left width, including any narrow-window
            // clamping, when the middle hands its room to the AI pane.
            if keep_left {
                self.left_share =
                    self.files_width / panel_room(f32::from(window.viewport_size().width)).max(1.);
            }
            *shown = !*shown;
            self.maximized_panel = None;
            self.dragging = None;
            if let Some(agents) = self.agents.clone() {
                agents.update(cx, |_, cx| cx.notify());
            }
            cx.notify();
        }
    }

    pub(super) fn render_workspace(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(panel) = self.maximized_panel {
            return match panel {
                Panel::LeftTop | Panel::LeftBottom => self.render_left(cx).into_any_element(),
                Panel::Editor => div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .child(self.render_notebook(window, cx))
                    .into_any_element(),
                Panel::Console => self.render_console_pane(window, cx).into_any_element(),
                Panel::Agents => div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_agents_pane(cx))
                    .into_any_element(),
            };
        }
        let agents = self.show_agents && self.agents_available();
        h_flex()
            .size_full()
            .items_stretch()
            .when(self.show_files, |d| d.child(self.render_left(cx)))
            .when(self.show_files && (self.show_middle || agents), |d| {
                d.child(self.splitter(Split::Files, cx))
            })
            .when(self.show_middle, |d| {
                d.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(look::strip()))
                                .flex()
                                .child(self.render_notebook(window, cx)),
                        )
                        .when(self.show_console, |d| {
                            d.child(self.splitter(Split::Console, cx))
                                .child(self.render_console_pane(window, cx))
                        }),
                )
            })
            .when(agents, |d| {
                d.when(self.show_middle, |d| {
                    d.child(self.splitter(Split::Agents, cx))
                })
                .child(
                    div()
                        .w(px(self.agents_width))
                        .flex_none()
                        .h_full()
                        .child(self.render_agents_pane(cx)),
                )
            })
            .into_any_element()
    }

    /// The left and AI panels' widths from their shares of the window: a
    /// quarter each at first (1:2:1), the middle taking what is left.
    pub(super) fn size_panels(&mut self, window: &Window) {
        let width = f32::from(window.viewport_size().width);
        let (left, _, agents) = column_widths(
            width,
            self.show_files,
            self.show_middle,
            self.show_agents && self.agents_available(),
            self.left_share,
            self.agents_share,
        );
        self.files_width = left;
        self.agents_width = agents;
        let height = left_column(f32::from(window.viewport_size().height));
        self.variables_height = self.left_vertical.heights(height, self.variables_share).0;
        self.console_height = self
            .middle_vertical
            .heights(height, 1. - self.console_share)
            .1;
    }

    /// A gap between panes that can be dragged; an accent line shows under
    /// the pointer.
    pub(crate) fn splitter(&self, split: Split, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        let across = matches!(split, Split::Variables | Split::Console);
        let dragging = self.dragging == Some(split);
        let id: SharedString = format!("split-{split:?}").into();
        let line = div()
            .absolute()
            .rounded(px(1.))
            .bg(p.accent)
            .map(|d| {
                if across {
                    d.top(px(0.5))
                        .h(px(2.))
                        .left(relative(0.3))
                        .right(relative(0.3))
                } else {
                    d.left(px(0.5))
                        .w(px(2.))
                        .top(relative(0.3))
                        .bottom(relative(0.3))
                }
            })
            .opacity(if dragging { 0.8 } else { 0. });
        let group: SharedString = format!("{id}-group").into();
        div()
            .id(id)
            .group(group.clone())
            .relative()
            .flex_none()
            .map(|d| {
                if across {
                    d.h(px(GAP)).w_full().cursor(CursorStyle::ResizeUpDown)
                } else {
                    d.w(px(GAP)).h_full().cursor(CursorStyle::ResizeLeftRight)
                }
            })
            .child(line.group_hover(group, |d| d.opacity(0.8)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.dragging = Some(split);
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
    }

    /// Resizes the panes beside a gap being dragged to the pointer.
    pub(super) fn drag_split(
        &mut self,
        split: Split,
        at: Point<Pixels>,
        window: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let (width, height) = (f32::from(window.width), f32::from(window.height));
        let agents_shown = self.show_agents && self.agents_available();
        let visible = self.show_files as u8 + self.show_middle as u8 + agents_shown as u8;
        let available = (width - 2. * EDGE - GAP * visible.saturating_sub(1) as f32).max(0.);
        let room = panel_room(width).max(1.);
        let middle = if self.show_middle { 360. } else { 0. };
        match split {
            Split::Files => {
                let other = if self.show_middle {
                    if agents_shown { self.agents_width } else { 0. }
                } else if agents_shown {
                    300.
                } else {
                    0.
                };
                let most = (available - other - middle).max(0.);
                self.left_share = (x - EDGE).clamp(220_f32.min(most), most) / room;
            }
            Split::Agents => {
                let files = if self.show_files {
                    self.files_width
                } else {
                    0.
                };
                let most = (available - files - middle).max(0.);
                self.agents_share = (width - x - EDGE).clamp(300_f32.min(most), most) / room;
            }
            Split::Variables => {
                self.left_vertical = crate::shell::pane_sizes::Vertical::Split;
                // The top pane's height, from the top of the column.
                let top = look::TITLE_BAR + GAP;
                let most = (height - look::STATUS_BAR - top - 120.).max(90.);
                self.variables_height = (y - top).clamp(90., most);
                self.variables_share = self.variables_height / left_column(height).max(1.);
            }
            Split::Console => {
                self.middle_vertical = crate::shell::pane_sizes::Vertical::Split;
                let bottom = height - look::STATUS_BAR;
                let most = (bottom - look::TITLE_BAR - 160.).max(80.);
                self.console_height = (bottom - y).clamp(80., most);
                self.console_share = self.console_height / left_column(height).max(1.);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::column_widths;
    #[test]
    fn visible_columns_share_all_available_room() {
        for width in [1000., 1440., 1920.] {
            for mask in 1u8..8 {
                let (left, middle, agents) = (mask & 1 != 0, mask & 2 != 0, mask & 4 != 0);
                let (l, m, a) = column_widths(width, left, middle, agents, 0.25, 0.25);
                let gaps = (mask.count_ones() - 1) as f32 * crate::look::GAP;
                assert!((l + m + a + gaps + 2. * crate::look::EDGE - width).abs() < 0.01);
                assert_eq!(l > 0., left);
                assert_eq!(m > 0., middle);
                assert_eq!(a > 0., agents);
                if middle {
                    assert!(m >= 360.);
                }
            }
            let normal = column_widths(width, true, true, true, 0.25, 0.25);
            let hidden = column_widths(width, true, false, true, 0.25, 0.25);
            assert_eq!(normal.0, hidden.0);
            assert!(hidden.2 > normal.2);
        }
    }

    #[test]
    fn side_only_split_can_use_the_former_middle_width() {
        let (left, middle, agents) = column_widths(1440., true, false, true, 0.7, 0.25);
        assert!(left > 900.);
        assert_eq!(middle, 0.);
        assert!(agents >= 300.);
        let (_, middle, _) = column_widths(1000., true, true, true, 0.9, 0.9);
        assert!(middle >= 360.);
        assert_eq!(
            column_widths(0., true, true, true, 0.25, 0.25),
            (0., 0., 0.)
        );
    }
}
