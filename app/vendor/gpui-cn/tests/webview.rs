//! UI integration tests for `WebFrame`. A native web view needs a real
//! window handle, so the tests put a plain block in the frame.

#![cfg(feature = "webview")]

use gpui_cn::WebFrame;
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, TestSupportExt as _, Window, div, px, size,
    test::TestWindowExt as _,
};

struct Harness;

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p_4().child(
            WebFrame::new("frame")
                .w(px(200.))
                .h(px(120.))
                .child(div().id("page").test_support().size_full()),
        )
    }
}

#[gpui_kit::test]
fn frame_holds_its_size_and_its_child(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let view = cx.new(|_| Harness);
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let frame = window.find("frame").bounds();
        assert_eq!((frame.size.width, frame.size.height), (px(200.), px(120.)));
        let page = window.find("page").bounds();
        assert_eq!((page.size.width, page.size.height), (px(198.), px(118.)));
    })
    .unwrap();
}
