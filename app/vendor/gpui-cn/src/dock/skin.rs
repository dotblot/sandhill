//! The gpui-cn look of a dock area, and the drag that rearranges its
//! panes.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, CursorStyle, DispatchPhase, Div, ElementId, Empty,
    Entity, InteractiveElement as _, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, canvas, deferred, div, prelude::FluentBuilder as _,
};
use gpui_kit::{
    assets::IconName,
    base::{
        ElementExt as _, ResizeHandleContext, TestSupportExt as _,
        dock::{
            DockArea, DockAreaRenderer, DockEvent, DockPlacement, DragPanel, NodeId, PanelId,
            TabGroupContext, TabGroupRenderer,
        },
        h_flex,
    },
};

use super::{motion::LayoutMotion, session::DockSkinState};
use crate::{ActiveTheme as _, Icon, Theme};

/// Paint order of the lifted pane's card, above every moving pane.
const CARD: usize = 3;
/// Paint order of the drop surface, above the card, so a release over the
/// dock always lands on it.
const DROP_SURFACE: usize = 4;

/// The gpui-cn look of a `DockArea`, with Ghostty-style pane dragging.
///
/// Each pane is a tab group of `gpui-base`'s dock. In a split, the panes
/// are parted by hairlines, the panes without focus are dimmed, and a
/// grab handle at the top center of each pane lifts it. A lifted
/// pane floats as a card under the pointer; resting on another pane for a
/// moment previews it beside that pane, on the nearest side, and the other
/// panes make room. Letting go keeps the preview; Escape, or letting go
/// outside every pane, puts the layout back. A group shows a tab strip only
/// when it holds more than one panel.
///
/// The drag never merges a pane into a tab group: it bypasses the drop
/// handling of `gpui-base`'s groups, whose center zone merges.
#[derive(Clone)]
pub struct DockSkin {
    state: Entity<DockSkinState>,
}

impl DockSkin {
    /// A dock area drawn in the gpui-cn look. Fill it with
    /// [`DockArea::set_center`].
    pub fn area(
        id: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<DockArea> {
        let id = id.into();
        cx.new(|cx| {
            let area = cx.weak_entity();
            let state = cx.new(|_| DockSkinState::new(area));
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            let skin = state.clone();
            cx.subscribe_self(move |area: &mut DockArea, event, cx| {
                if let DockEvent::LayoutChanged = event
                    && let Some(tree) = area.layout(DockPlacement::Center).cloned()
                {
                    skin.update(cx, |skin, cx| skin.layout_changed(&tree, cx));
                }
            })
            .detach();
            DockArea::new(id, None, window, cx).with_renderer(Rc::new(DockSkin { state }))
        })
    }

    fn panel_id(group: &TabGroupContext, cx: &App) -> Option<PanelId> {
        group.active_panel().map(|panel| panel.panel_id(cx))
    }

    /// The card a lifted pane floats as, under the pointer.
    fn render_card(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let super::session::Card {
            view,
            rect: card,
            content: slot,
            growing,
        } = state.card(cx)?;
        let frame = state.frame().origin;
        if growing {
            window.request_animation_frame();
        }
        let theme = cx.theme();
        let shadow = Theme::global(cx).shadow;
        Some(
            deferred(
                div()
                    .id("dock-card")
                    .test_support()
                    .absolute()
                    .left(card.origin.x - frame.x)
                    .top(card.origin.y - frame.y)
                    .w(card.size.width)
                    .h(card.size.height)
                    .overflow_hidden()
                    .rounded(theme.radius_lg())
                    .border_1()
                    .border_color(theme.ring())
                    .bg(theme.background())
                    .when(shadow, |this| this.shadow(theme.base.shadow.lg.clone()))
                    .child(
                        div()
                            .w(slot.size.width)
                            .h(slot.size.height)
                            .flex_none()
                            .child(view.view()),
                    ),
            )
            .priority(CARD)
            .into_any_element(),
        )
    }

    /// The surface every drop over the dock lands on while a pane is held,
    /// and the watch for a release anywhere else.
    fn render_drop_surface(&self) -> AnyElement {
        let on_drop = self.state.clone();
        let on_release = self.state.clone();
        deferred(
            div()
                .id("dock-drop-surface")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_drop::<DragPanel>(move |_, window, cx| {
                    let position = window.mouse_position();
                    on_drop.update(cx, |state, cx| state.drop_at(position, window, cx));
                })
                .child(
                    canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let state = on_release.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                                if phase != DispatchPhase::Capture {
                                    return;
                                }
                                // After the drop listeners: one that took
                                // the release has already ended the drag.
                                let state = state.clone();
                                window.defer(cx, move |window, cx| {
                                    state.update(cx, |state, cx| state.released(window, cx));
                                });
                            });
                        },
                    )
                    .absolute()
                    .size_full(),
                ),
        )
        .priority(DROP_SURFACE)
        .into_any_element()
    }

    /// The grab handle at the top center of a pane, and the band that
    /// reveals it.
    fn render_handle(&self, panel: PanelId, node: NodeId, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let metrics = &theme.metrics;
        let touch = theme.touch;
        let state = self.state.read(cx);
        let shown = touch || state.revealed() == Some(panel) || state.lifted() == Some(panel);
        let color = if !touch && state.hovered() == Some(panel) {
            theme.dock_handle_hover
        } else {
            theme.dock_handle
        };
        let reveal = metrics.dock_handle_reveal;
        let handle_height = metrics.dock_handle_height;
        let band = {
            let state = self.state.clone();
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let height = (bounds.size.height * reveal)
                        .max(handle_height)
                        .min(bounds.size.height);
                    let band = gpui_kit::Bounds::new(
                        bounds.origin,
                        gpui_kit::size(bounds.size.width, height),
                    );
                    let state = state.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let inside = band.contains(&event.position);
                            state.update(cx, |state, cx| state.reveal(panel, inside, cx));
                        }
                    });
                },
            )
            .absolute()
            .size_full()
        };
        let press = self.state.clone();
        let hover = self.state.clone();
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(band)
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .justify_center()
                    .child(
                        div()
                            .id(ElementId::NamedInteger("dock-grab".into(), panel.as_u64()))
                            .test_support()
                            .w(metrics.dock_handle_width)
                            .h(metrics.dock_handle_height)
                            .flex()
                            .justify_center()
                            .items_start()
                            .cursor(CursorStyle::OpenHand)
                            .text_color(color)
                            .on_hover(move |hovered, _, cx| {
                                hover.update(cx, |state, cx| {
                                    state.hover_handle(panel, *hovered, cx)
                                });
                            })
                            // Ghostty's handle consumes its press too: a pane
                            // under it, such as a terminal, would start a
                            // selection that the drag then extends.
                            .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                                cx.stop_propagation();
                                press
                                    .update(cx, |state, cx| state.press(panel, event.position, cx));
                            })
                            .on_drag(DragPanel::new(panel, node), |drag, _, _, cx| {
                                cx.new(|_| drag.clone())
                            })
                            .when(shown, |this| {
                                this.child(
                                    div()
                                        .id(ElementId::NamedInteger(
                                            "dock-grab-icon".into(),
                                            panel.as_u64(),
                                        ))
                                        .test_support()
                                        .child(
                                            Icon::from(IconName::Ellipsis)
                                                .size(metrics.dock_handle_icon),
                                        ),
                                )
                            }),
                    ),
            )
    }
}

impl DockAreaRenderer for DockSkin {
    fn frame(&self, window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let dragging = self.state.read(cx).is_dragging();
        let record = self.state.clone();
        let moved = self.state.clone();
        let escape = self.state.clone();
        let card = self.render_card(window, cx);
        div()
            .id("dock-area")
            .bg(cx.theme().background())
            .on_prepaint(move |bounds, _, cx| {
                record.update(cx, |state, _| state.record_frame(bounds));
            })
            .on_drag_move::<DragPanel>(move |event, window, cx| {
                let position = event.event.position;
                moved.update(cx, |state, cx| state.moved(position, window, cx));
            })
            .capture_key_down(move |event, window, cx| {
                if event.keystroke.key == "escape"
                    && escape.update(cx, |state, cx| state.escape(window, cx))
                {
                    cx.stop_propagation();
                }
            })
            .children(card)
            .when(dragging, |this| this.child(self.render_drop_surface()))
    }

    fn render_split_handle(
        &self,
        handle: &ResizeHandleContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let color = if handle.is_active() {
            theme.ring()
        } else {
            theme.border()
        };
        Some(div().size_full().bg(color).into_any_element())
    }

    fn tab_group_renderer(&self) -> Rc<dyn TabGroupRenderer> {
        Rc::new(self.clone())
    }
}

impl TabGroupRenderer for DockSkin {
    fn frame(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        div()
            .id(ElementId::NamedInteger(
                "dock-group".into(),
                group.node().as_u64(),
            ))
            .bg(cx.theme().background())
    }

    fn content_frame(&self, group: &TabGroupContext, _: &mut Window, _: &mut App) -> Stateful<Div> {
        let node = group.node();
        let record = self.state.clone();
        div()
            .id(ElementId::NamedInteger(
                "dock-group-content".into(),
                node.as_u64(),
            ))
            .relative()
            .on_prepaint(move |bounds, _, cx| {
                record.update(cx, |state, _| state.record_group(node, bounds));
            })
    }

    fn render_tab_bar(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> AnyElement {
        let lifted = self.state.read(cx).lifted();
        let shown = group
            .panels()
            .iter()
            .filter(|panel| Some(panel.panel_id(cx)) != lifted)
            .count();
        if shown < 2 {
            return Empty.into_any_element();
        }
        let theme = cx.theme();
        let active = group.active_ix();
        h_flex()
            .h(theme.metrics.row_sm)
            .flex_none()
            .px_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border())
            .text_size(theme.text_control.size)
            .children(
                group
                    .panels()
                    .iter()
                    .enumerate()
                    .filter(|(_, panel)| Some(panel.panel_id(cx)) != lifted)
                    .map(|(ix, panel)| {
                        let select = group.clone();
                        div()
                            .id(ElementId::NamedInteger(
                                "dock-tab".into(),
                                panel.panel_id(cx).as_u64(),
                            ))
                            .test_support()
                            .px_2()
                            .rounded(theme.radius_sm())
                            .text_color(if ix == active {
                                theme.foreground()
                            } else {
                                theme.muted_foreground()
                            })
                            .when(ix == active, |this| this.bg(theme.selected))
                            .child(panel.panel_name(cx))
                            .on_click(move |_, window, cx| select.select_tab(ix, window, cx))
                    }),
            )
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(id) = Self::panel_id(group, cx) else {
            return panel.into_any_element();
        };
        let background = cx.theme().background();
        let dock_drop = cx.theme().dock_drop;
        let ring = cx.theme().ring();
        let (lifted, split) = {
            let state = self.state.read(cx);
            (state.lifted() == Some(id), state.split())
        };
        if lifted {
            return div()
                .id(ElementId::NamedInteger("dock-slot".into(), id.as_u64()))
                .test_support()
                .size_full()
                .bg(dock_drop)
                .border_1()
                .border_color(ring)
                .into_any_element();
        }
        let split = split && !group.is_zoomed();
        let dimmed = split
            && !group
                .active_panel()
                .is_some_and(|panel| panel.focus_handle(cx).contains_focused(window, cx));
        let unfocused = cx.theme().dock_unfocused;
        let handle = split.then(|| self.render_handle(id, group.node(), cx));
        div()
            .relative()
            .size_full()
            .child(LayoutMotion::new(
                id,
                self.state.clone(),
                div()
                    .relative()
                    .size_full()
                    .bg(background)
                    .child(panel)
                    .when(dimmed, |this| {
                        this.child(
                            div()
                                .id(ElementId::NamedInteger("dock-dim".into(), id.as_u64()))
                                .test_support()
                                .absolute()
                                .inset_0()
                                .bg(unfocused),
                        )
                    }),
            ))
            .children(handle)
            .into_any_element()
    }
}
