//! Thinking effort as one segmented bar (Huan, 2026-11-12, after a Dribbble
//! reference): the fill runs up to the chosen level and carries its name,
//! hairline ticks divide the rest, a click or drag snaps to a level and the
//! fill springs to its new length, glowing while hovered or dragged. The
//! strongest level is drawn warm.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::v_flex;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::agents::Agents;
use crate::agents::composer::{Models, effort_name};
use crate::look::{self, Palette};

const LABEL_INSET: f32 = look::GAP_8;
/// Room for the handle at the fill's end.
const HANDLE_ROOM: f32 = 12.;
/// Slightly under critical damping: quick, with a hint of give.
const SPRING: SpringConfig = SpringConfig::new(420., 37., 1.);

/// The bar's state between frames.
#[derive(Default)]
pub(crate) struct Bar {
    bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
}

#[derive(Clone)]
struct DragEffort;

impl Render for DragEffort {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// The level under `x` on a bar starting at `left`, `width` wide, cut into
/// `levels` equal segments; outside the bar, the nearest end.
fn level_at(x: f32, left: f32, width: f32, levels: usize) -> usize {
    if levels == 0 || width <= 0. {
        return 0;
    }
    let segment = ((x - left) / width * levels as f32).floor().max(0.) as usize;
    segment.min(levels - 1)
}

/// The level shown: the one picked if the agent offers it, else its
/// recommended one.
fn shown_index(models: &Models, picked: Option<String>) -> usize {
    let current = picked
        .filter(|e| models.efforts.contains(e))
        .unwrap_or_else(|| models.recommended.clone());
    models
        .efforts
        .iter()
        .position(|e| *e == current)
        .unwrap_or(0)
}

impl Agents {
    fn pick_effort_at(
        &mut self,
        kind: &str,
        x: Pixels,
        bounds: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let models = self.models_of(kind);
        let index = level_at(
            f32::from(x),
            f32::from(bounds.left()),
            f32::from(bounds.size.width),
            models.efforts.len(),
        );
        if index == shown_index(&models, self.effort_shown(kind)) {
            return;
        }
        if let Some(level) = models.efforts.get(index).cloned() {
            self.choose_effort(kind, level, cx);
        }
    }

    fn end_effort_drag(&mut self, cx: &mut Context<Self>) {
        if self.effort_bar.dragging {
            self.effort_bar.dragging = false;
            cx.notify();
        }
    }

    /// The model menu's effort section for `kind`, `width` wide.
    pub(crate) fn render_effort(
        &self,
        kind: &str,
        width: f32,
        p: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let models = self.models_of(kind);
        let levels = models.efforts.len();
        let at = shown_index(&models, self.effort_shown(kind));
        let enabled = levels >= 2;
        let strongest = enabled && at + 1 == levels;
        let fraction = if levels == 0 {
            0.
        } else {
            (at + 1) as f32 / levels as f32
        };
        let name = match models.efforts.get(at) {
            Some(level) => effort_name(level),
            None => "Set by the agent".to_string(),
        };
        let style = window.text_style();
        let name_width = f32::from(
            window
                .text_system()
                .shape_line(
                    name.clone().into(),
                    px(look::SMALL),
                    &[style.to_run(name.len())],
                    None,
                )
                .width,
        );
        let inside = levels > 0 && LABEL_INSET + name_width + HANDLE_ROOM <= fraction * width;

        let white = hsla(0., 0., 1., 1.);
        let purple: Hsla = rgb(0xa855f7).into();
        let pink: Hsla = rgb(0xec4899).into();
        let orange: Hsla = rgb(0xfb923c).into();
        // Lighter at the start, the accent's depth at the end, as in the
        // reference; dark mode's accent is already light.
        let (lift, sink) = if p.dark { (0.06, 0.1) } else { (0.22, 0.) };
        let bright = Hsla {
            l: (p.accent.l + lift).min(0.92),
            ..p.accent
        };
        let deep = Hsla {
            l: p.accent.l - sink,
            ..p.accent
        };
        let glow_color = if strongest { pink } else { p.accent };
        let glow = vec![BoxShadow {
            color: glow_color.opacity(0.45),
            offset: point(px(0.), px(0.)),
            blur_radius: px(12.),
            spread_radius: px(0.),
            inset: false,
        }];

        let ticks = (1..levels).map(|i| {
            div()
                .absolute()
                .top(px(7.))
                .bottom(px(7.))
                .left(relative(i as f32 / levels as f32))
                .ml(px(-0.5))
                .w(px(1.))
                .bg(p.faint)
        });
        let fill = div()
            .absolute()
            .top_0()
            .left_0()
            .h_full()
            .rounded(px(look::CONTROL_RADIUS))
            .when(levels == 0, |d| d.invisible())
            .map(|d| {
                if strongest {
                    d.bg(linear_gradient(
                        90.,
                        linear_color_stop(purple, 0.),
                        linear_color_stop(pink, 1.),
                    ))
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(look::CONTROL_RADIUS))
                            .bg(linear_gradient(
                                90.,
                                linear_color_stop(orange.opacity(0.), 0.45),
                                linear_color_stop(orange, 1.),
                            )),
                    )
                } else {
                    d.bg(linear_gradient(
                        90.,
                        linear_color_stop(bright, 0.),
                        linear_color_stop(deep, 1.),
                    ))
                }
            })
            .child(
                div()
                    .absolute()
                    .top(px(5.))
                    .bottom(px(5.))
                    .right(px(5.))
                    .w(px(2.))
                    .rounded_full()
                    .bg(white.opacity(0.9)),
            )
            .when(self.effort_bar.dragging, |d| d.shadow(glow.clone()))
            .group_hover("effort-bar", move |s| s.shadow(glow))
            .with_spring(
                "effort-fill",
                SpringAnimation::new(SPRING).to(fraction),
                |d, value| d.w(relative(value.clamp(0., 1.))),
            );
        let label = div()
            .absolute()
            .top_0()
            .h_full()
            .flex()
            .items_center()
            .whitespace_nowrap()
            .font_weight(FontWeight::MEDIUM)
            .map(|d| {
                if inside {
                    d.left(px(LABEL_INSET)).text_color(if strongest {
                        white
                    } else {
                        p.primary_text
                    })
                } else {
                    d.left(px(fraction * width + LABEL_INSET))
                        .text_color(p.muted)
                }
            })
            .child(name);
        let bounds = self.effort_bar.bounds.clone();
        let down_kind = kind.to_string();
        let drag_kind = kind.to_string();
        let bar = div()
            .id("effort-bar")
            .group("effort-bar")
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(look::CONTROL_CARD))
            .rounded(px(look::CONTROL_RADIUS))
            .bg(p.ink(8.))
            .text_size(px(look::SMALL))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .children(ticks)
            .child(fill)
            .child(label)
            .map(|d| {
                if !enabled {
                    return d.opacity(0.6);
                }
                d.cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.effort_bar.dragging = true;
                            let bounds = this.effort_bar.bounds.get();
                            this.pick_effort_at(&down_kind, event.position.x, bounds, cx);
                            cx.notify();
                        }),
                    )
                    .on_drag(DragEffort, |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    })
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DragEffort>, _, cx| {
                            this.pick_effort_at(
                                &drag_kind,
                                event.event.position.x,
                                event.bounds,
                                cx,
                            );
                        },
                    ))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_effort_drag(cx)),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_effort_drag(cx)),
                    )
            });
        v_flex()
            .flex_none()
            .gap(px(look::GAP_6))
            .px(px(look::GAP_8))
            .pt(px(look::GAP_8))
            .pb(px(look::GAP_6))
            .mt(px(look::GAP_4))
            .border_t_1()
            .border_color(p.line)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Thinking effort"),
            )
            .child(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::level_at;

    #[test]
    fn positions_snap_to_their_segment_and_ends_clamp() {
        // Four levels on a bar from 100 to 500: segments of 100.
        assert_eq!(level_at(100., 100., 400., 4), 0);
        assert_eq!(level_at(199.9, 100., 400., 4), 0);
        assert_eq!(level_at(200., 100., 400., 4), 1);
        assert_eq!(level_at(499., 100., 400., 4), 3);
        assert_eq!(level_at(40., 100., 400., 4), 0);
        assert_eq!(level_at(900., 100., 400., 4), 3);
        assert_eq!(level_at(300., 100., 400., 0), 0);
    }
}
