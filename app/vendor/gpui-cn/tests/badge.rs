//! UI integration tests for `Badge`.

use gpui_cn::{ActiveTheme as _, Avatar, Badge};
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
            .gap_4()
            .p_4()
            .child(
                Badge::new("one")
                    .count(3)
                    .child(Avatar::new("a").name("Ada")),
            )
            .child(
                Badge::new("many")
                    .count(120)
                    .child(Avatar::new("b").name("Alan")),
            )
            .child(
                Badge::new("none")
                    .count(0)
                    .child(Avatar::new("c").name("Grace")),
            )
            .child(
                Badge::new("dot")
                    .dot()
                    .child(Avatar::new("d").name("Linus")),
            )
            .child(
                Badge::new("icon")
                    .icon(IconName::Check)
                    .child(Avatar::new("e").name("Margaret")),
            )
    }
}

fn setup(cx: &mut TestAppContext) -> WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let handle = cx.open_window(size(px(400.), px(400.)), |window, cx| {
        let view = cx.new(|_| Harness);
        Root::new(view, window, cx)
    });
    cx.run_until_parked();
    handle
}

#[gpui_kit::test]
fn a_count_overlaps_the_top_trailing_corner(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let avatar = window.find("a").bounds();
        let mark = window.find("one").bounds();
        assert_eq!(mark.size.height, metrics.badge_count);
        assert_eq!(
            mark.size.width, metrics.badge_count,
            "one digit is a circle"
        );
        assert_eq!(mark.top(), avatar.top() - metrics.badge_count_offset);
        assert_eq!(mark.right(), avatar.right() + metrics.badge_count_offset);

        let many = window.find("many").bounds();
        assert!(many.size.width > mark.size.width, "99+ is wider");
        assert!(window.try_find("none").is_none(), "zero hides the count");
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_dot_and_an_icon_sit_inside_the_corners(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let avatar = window.find("d").bounds();
        let dot = window.find("dot").bounds();
        assert_eq!(dot.size, size(metrics.badge_dot, metrics.badge_dot));
        assert_eq!(dot.top_right(), avatar.top_right());

        let avatar = window.find("e").bounds();
        let icon = window.find("icon").bounds();
        assert_eq!(icon.size, size(metrics.badge_icon, metrics.badge_icon));
        assert_eq!(icon.bottom_right(), avatar.bottom_right());
    })
    .unwrap();
}
