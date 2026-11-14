//! Where a `Select` menu sits against its trigger.

use gpui_cn::{ReduceMotion, Select, SelectItem, SelectState, Theme};
use gpui_kit::base::{Align, Root};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Pixels, Render,
    Styled as _, TestAppContext, Window, WindowHandle, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<SelectState<&'static str>>,
    width: Pixels,
    align: Align,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().pt_8().pl(px(160.)).child(
            Select::new("pick", &self.state)
                .align(self.align)
                .w(self.width),
        )
    }
}

fn open(
    cx: &mut TestAppContext,
    width: f32,
    align: Align,
    search: bool,
) -> (WindowHandle<Root>, ElementId, ElementId) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        // The final place, not a frame of the opening motion.
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let handle = cx.open_window(size(px(700.), px(600.)), |window, cx| {
        let state = cx.new(|cx| {
            let state = SelectState::new(
                [
                    SelectItem::new("default", "Ghostty default"),
                    SelectItem::new("dracula", "Dracula"),
                    SelectItem::new("nord", "Nord"),
                ],
                cx,
            )
            .with_selected(["default"]);
            if search {
                state.with_search("Search themes", window, cx)
            } else {
                state
            }
        });
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Harness {
                state,
                width: px(width),
                align,
            }
        });
        Root::new(harness, window, cx)
    });
    let child = |name: &str| {
        ElementId::NamedChild(
            ElementId::Name("pick".into()).into(),
            name.to_owned().into(),
        )
    };
    cx.update_window(handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
        window.click(child("trigger"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    (handle, child("trigger"), child("menu"))
}

fn bounds(
    handle: WindowHandle<Root>,
    trigger: ElementId,
    menu: ElementId,
    cx: &mut TestAppContext,
) -> (gpui_kit::Bounds<Pixels>, gpui_kit::Bounds<Pixels>) {
    cx.update_window(handle.into(), |_, window, _| {
        (window.find(trigger).bounds(), window.find(menu).bounds())
    })
    .unwrap()
}

#[gpui_kit::test]
fn a_menu_as_wide_as_its_trigger_lines_up_with_both_edges(cx: &mut TestAppContext) {
    for search in [false, true] {
        let (handle, trigger, menu) = open(cx, 280., Align::End, search);
        let (trigger, menu) = bounds(handle, trigger, menu, cx);
        assert_eq!(menu.right(), trigger.right(), "search {search}");
        assert_eq!(menu.left(), trigger.left(), "search {search}");
    }
}

#[gpui_kit::test]
fn an_end_aligned_menu_wider_than_its_trigger_grows_to_the_left(cx: &mut TestAppContext) {
    // A search field makes the menu at least 240px wide.
    let (handle, trigger, menu) = open(cx, 120., Align::End, true);
    let (trigger, menu) = bounds(handle, trigger, menu, cx);
    assert_eq!(menu.right(), trigger.right());
    assert!(menu.left() < trigger.left(), "{menu:?} vs {trigger:?}");
    assert!(menu.left() >= px(0.), "inside the window");
}

#[gpui_kit::test]
fn a_start_aligned_menu_lines_up_with_the_leading_edge(cx: &mut TestAppContext) {
    let (handle, trigger, menu) = open(cx, 120., Align::Start, true);
    let (trigger, menu) = bounds(handle, trigger, menu, cx);
    assert_eq!(menu.left(), trigger.left());
    assert!(menu.right() > trigger.right());
}
