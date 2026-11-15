//! UI integration tests for `ScrollArea`.

use gpui_cn::ScrollArea;
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, PlatformInput, Render, ScrollDelta, ScrollEvent, Styled as _,
    TestAppContext, TouchPhase, Window, base::TestSupportExt as _, div, point, px, size,
    test::TestWindowExt as _,
};

/// A 200px viewport over forty 30px rows.
struct Harness;

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h(px(200.))
            .child(
                ScrollArea::new("rows")
                    .size_full()
                    .children((0..40usize).map(|n| {
                        div()
                            .id(("row", n))
                            .test_support()
                            .h(px(30.))
                            .child(format!("Row {n}"))
                    })),
            )
    }
}

#[gpui_kit::test]
fn the_region_scrolls_its_rows_and_keeps_its_position_across_frames(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness);
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find("rows").bounds().size.height, px(200.));
        assert_eq!(window.find(("row", 0usize)).bounds().origin.y, px(0.));
        window.scroll("rows", ScrollDelta::Pixels(point(px(0.), px(-90.))), cx);
        window.render_frame(cx);
        assert_eq!(window.find(("row", 0usize)).bounds().origin.y, px(-90.));
        // The position lives in window state, not in the element, so a
        // fresh render of the view keeps it.
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(("row", 0usize)).bounds().origin.y, px(-90.));
    })
    .unwrap();
}

/// A page that scrolls, holding a region that scrolls (forty rows in a
/// 120px box) and a region whose content fits (three rows in a 200px box).
struct Nested;

impl Render for Nested {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_full().h(px(300.)).child(
            ScrollArea::new("page")
                .size_full()
                .child(div().id("top").test_support().h(px(20.)))
                .child(
                    div()
                        .h(px(120.))
                        .child(
                            ScrollArea::new("inner")
                                .size_full()
                                .children((0..40usize).map(|n| {
                                    div()
                                        .id(("inner-row", n))
                                        .test_support()
                                        .h(px(30.))
                                        .child(format!("Inner {n}"))
                                })),
                        ),
                )
                .child(
                    div()
                        .h(px(200.))
                        .child(
                            ScrollArea::new("fits")
                                .size_full()
                                .children((0..3usize).map(|n| {
                                    div()
                                        .id(("fits-row", n))
                                        .test_support()
                                        .h(px(30.))
                                        .child(format!("Fits {n}"))
                                })),
                        ),
                )
                .child(div().h(px(600.))),
        )
    }
}

#[gpui_kit::test]
fn a_region_with_room_to_scroll_keeps_the_wheel_and_one_that_fits_passes_it_on(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Nested);
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let top = window.find("top").bounds().origin.y;
        // A wheel step over the inner region scrolls it and not the page.
        window.scroll(
            ("inner-row", 1usize),
            ScrollDelta::Pixels(point(px(0.), px(-40.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(
            window.find(("inner-row", 0usize)).bounds().origin.y,
            px(-40.) + px(20.)
        );
        assert_eq!(window.find("top").bounds().origin.y, top, "the page stayed");
        // A wheel step over the region whose content fits scrolls the page.
        window.scroll(
            ("fits-row", 1usize),
            ScrollDelta::Pixels(point(px(0.), px(-40.))),
            cx,
        );
        window.render_frame(cx);
        assert_eq!(window.find("top").bounds().origin.y, top - px(40.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_step_past_the_top_stretches_the_region(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness);
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert_eq!(window.find(("row", 0usize)).bounds().origin.y, px(0.));
        // A finger pulling down at the top: the rows follow it past the
        // edge instead of staying put.
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: window.find("rows").bounds().center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(60.))),
                touch_phase: TouchPhase::Started,
                ..Default::default()
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::Scroll(ScrollEvent {
                position: window.find("rows").bounds().center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(60.))),
                touch_phase: TouchPhase::Moved,
                ..Default::default()
            }),
            cx,
        );
        window.render_frame(cx);
        let stretched = window.find(("row", 0usize)).bounds().origin.y;
        assert!(stretched > px(0.), "stretched past the top: {stretched:?}");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_scrollbar_shows_after_a_wheel_step_and_drags_the_content(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};

    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let harness = cx.new(|_| Harness);
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        window.scroll("rows", ScrollDelta::Pixels(point(px(0.), px(-90.))), cx);
        window.render_frame(cx);
        assert_eq!(window.find(("row", 0usize)).bounds().origin.y, px(-90.));
        let viewport = window.find("rows").bounds();
        let thumb = point(viewport.right() - px(4.), px(30.));
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: thumb,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position: thumb + point(px(0.), px(50.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: thumb + point(px(0.), px(50.)),
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
        window.render_frame(cx);
        let row = window.find(("row", 0usize)).bounds().origin.y;
        assert!(row < px(-200.), "the thumb dragged the rows to {row:?}");
    })
    .unwrap();
}

/// A field above a 200px region over a field, 600px of filler, and a
/// second field.
struct Form {
    outside: gpui_kit::Entity<gpui_cn::InputState>,
    top: gpui_kit::Entity<gpui_cn::InputState>,
    bottom: gpui_kit::Entity<gpui_cn::InputState>,
}

impl Render for Form {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(gpui_cn::Input::new(&self.outside).id("outside"))
            .child(
                div().h(px(200.)).child(
                    ScrollArea::new("form")
                        .size_full()
                        .child(gpui_cn::Input::new(&self.top).id("top"))
                        .child(div().h(px(600.)))
                        .child(gpui_cn::Input::new(&self.bottom).id("bottom")),
                ),
            )
    }
}

#[gpui_kit::test]
fn focusing_a_field_scrolls_it_into_view(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let mut fields = None;
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let outside = cx.new(|cx| gpui_cn::InputState::new(window, cx));
        let top = cx.new(|cx| gpui_cn::InputState::new(window, cx));
        let bottom = cx.new(|cx| gpui_cn::InputState::new(window, cx));
        fields = Some((outside.clone(), top.clone(), bottom.clone()));
        let form = cx.new(|_| Form {
            outside,
            top,
            bottom,
        });
        Root::new(form, window, cx)
    });
    let (outside, top, bottom) = fields.unwrap();
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        let region = window.find("form").bounds();
        assert!(
            window.find("bottom").bounds().top() > region.bottom(),
            "starts below the fold"
        );
        top.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    settle(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        assert_eq!(
            window.find("top").bounds().top(),
            window.find("form").bounds().top(),
            "a field in view does not scroll the region"
        );
        bottom.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    settle(cx);
    let revealed = cx
        .update_window(handle.into(), |_, window, cx| {
            let region = window.find("form").bounds();
            let field = window.find("bottom").bounds();
            assert!(
                field.top() >= region.top() && field.bottom() <= region.bottom(),
                "the focused field is inside the region: {field:?} in {region:?}"
            );
            outside.read(cx).focus_handle(cx).focus(window, cx);
            field.top()
        })
        .unwrap();
    settle(cx);
    cx.update_window(handle.into(), |_, window, _| {
        assert_eq!(
            window.find("bottom").bounds().top(),
            revealed,
            "a field outside the region does not scroll it"
        );
    })
    .unwrap();
}

/// A 200px region that holds 400px of filler and a 100px region with
/// 300px of filler and a field.
struct NestedForm {
    deep: gpui_kit::Entity<gpui_cn::InputState>,
}

impl Render for NestedForm {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_full().flex().flex_col().child(
            div().h(px(200.)).child(
                ScrollArea::new("outer")
                    .size_full()
                    .child(div().id("filler").test_support().h(px(400.)))
                    .child(
                        div().h(px(100.)).child(
                            ScrollArea::new("inner")
                                .size_full()
                                .child(div().h(px(300.)))
                                .child(gpui_cn::Input::new(&self.deep).id("deep")),
                        ),
                    ),
            ),
        )
    }
}

#[gpui_kit::test]
fn nested_regions_both_scroll_to_reveal_a_field(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let mut fields = None;
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let deep = cx.new(|cx| gpui_cn::InputState::new(window, cx));
        fields = Some(deep.clone());
        let nested = cx.new(|_| NestedForm { deep });
        Root::new(nested, window, cx)
    });
    let deep = fields.unwrap();
    let settle = |cx: &mut TestAppContext| {
        for _ in 0..3 {
            cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.run_until_parked();
        }
    };
    settle(cx);
    cx.update_window(handle.into(), |_, window, cx| {
        deep.read(cx).focus_handle(cx).focus(window, cx);
    })
    .unwrap();
    settle(cx);
    cx.update_window(handle.into(), |_, window, _| {
        let field = window.find("deep").bounds();
        let outer = window.find("outer").bounds();
        let inner = window.find("inner").bounds();
        assert!(
            field.top() >= inner.top() && field.bottom() <= inner.bottom(),
            "the inner region shows the field: {field:?} in {inner:?}"
        );
        assert!(
            field.top() >= outer.top() && field.bottom() <= outer.bottom(),
            "the outer region shows the field: {field:?} in {outer:?}"
        );
    })
    .unwrap();
}
