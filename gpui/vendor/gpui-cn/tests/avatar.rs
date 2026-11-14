//! UI integration tests for `Avatar` and `AvatarGroup`.

use gpui_cn::{ActiveTheme as _, Avatar, AvatarGroup, AvatarSize, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    Window, WindowHandle, div, px, size, test::TestWindowExt as _,
};

struct Harness;

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_start()
            .child(Avatar::new("sm").name("Ada Lovelace").size(AvatarSize::Sm))
            .child(Avatar::new("md").name("Ada Lovelace"))
            .child(Avatar::new("lg").size(AvatarSize::Lg))
            .child(
                AvatarGroup::new("team")
                    .child(Avatar::new("a").name("Ada"))
                    .child(Avatar::new("b").name("Alan"))
                    .child(Avatar::new("c").name("Grace"))
                    .child(Avatar::new("d").name("Linus"))
                    .limit(2),
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
fn sizes_come_from_the_theme(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let metrics = cx.update(|cx| cx.theme().metrics.clone());
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for (id, diameter) in [
            ("sm", metrics.avatar_sm),
            ("md", metrics.avatar_md),
            ("lg", metrics.avatar_lg),
        ] {
            let bounds = window.find(id).bounds();
            assert_eq!(bounds.size, size(diameter, diameter), "{id}");
        }
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_group_overlaps_and_folds_the_rest_into_a_count(cx: &mut TestAppContext) {
    let handle = setup(cx);
    let (diameter, overlap) = cx.update(|cx| {
        let metrics = &cx.theme().metrics;
        (metrics.avatar_md, metrics.avatar_group_overlap)
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let a = window.find("a").bounds();
        let b = window.find("b").bounds();
        assert_eq!(b.origin.x - a.origin.x, diameter - overlap);
        assert!(window.try_find("c").is_none(), "past the limit");
        let group = window.find("team").bounds();
        assert_eq!(group.size.width, diameter * 3. - overlap * 2.);
    })
    .unwrap();
}

#[gpui_kit::test]
fn sizes_scale_with_the_ui_font(cx: &mut TestAppContext) {
    let handle = setup(cx);
    cx.update(|cx| Theme::set_ui_font_size(cx, px(20.)));
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find("md").bounds().size.width, px(40.));
    })
    .unwrap();
}
