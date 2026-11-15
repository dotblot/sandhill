//! UI integration tests for `Tag`.

use gpui_cn::{ActiveTheme as _, Tag};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, WindowHandle, assets::IconName, div, px, size, test::TestWindowExt as _,
};

struct Harness;

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(Tag::new("short").label("New"))
            .child(Tag::new("long").label("A much longer label").outline())
            .child(
                Tag::new("icon")
                    .icon(IconName::Check)
                    .label("New")
                    .success(),
            )
    }
}

fn setup(cx: &mut TestAppContext) -> WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let view = cx.new(|_| Harness);
        Root::new(view, window, cx)
    });
    cx.run_until_parked();
    handle
}

#[gpui_kit::test]
fn height_comes_from_the_theme_and_width_from_the_content(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let height = cx.update(|cx| cx.theme().metrics.tag_height);
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let short = window.find("short").bounds();
        let long = window.find("long").bounds();
        let icon = window.find("icon").bounds();
        for bounds in [short, long, icon] {
            assert_eq!(bounds.size.height, height);
        }
        assert!(long.size.width > short.size.width, "hugs its label");
        assert!(icon.size.width > short.size.width, "the icon adds width");
    })
    .unwrap();
}
