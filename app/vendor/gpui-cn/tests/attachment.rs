//! UI integration tests for the attachment strip and its tiles.

use std::{cell::RefCell, rc::Rc};

use gpui_cn::{Attachment, AttachmentStrip, ReduceMotion, Theme};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, Image, ImageFormat, ImageSource, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, TestAppContext, Window, div, px, size,
    test::TestWindowExt as _,
};

const PIXEL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 2 2"><rect width="2" height="2" fill="#4a90d9"/></svg>"##;

struct Harness {
    attachments: Vec<Attachment>,
    dismissed: Rc<RefCell<Vec<String>>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dismissed = self.dismissed.clone();
        div().size_full().p_4().child(
            div().w(px(240.)).child(
                AttachmentStrip::new("files")
                    .attachments(self.attachments.clone())
                    .on_dismiss(move |id: &SharedString, _, _| {
                        dismissed.borrow_mut().push(id.to_string())
                    }),
            ),
        )
    }
}

fn tile(id: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::from("files").into(), id.into())
}

fn part(id: &'static str, name: &'static str) -> ElementId {
    ElementId::NamedChild(tile(id).into(), name.into())
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    view: Entity<Harness>,
    dismissed: Rc<RefCell<Vec<String>>>,
}

fn setup(cx: &mut TestAppContext, motion: ReduceMotion) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = motion);
    });
    let dismissed = Rc::new(RefCell::new(Vec::new()));
    let mut view = None;
    let image = ImageSource::from(Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        PIXEL.as_bytes().to_vec(),
    )));
    let handle = cx.open_window(size(px(400.), px(300.)), |window, cx| {
        let harness = cx.new(|_| Harness {
            attachments: vec![
                Attachment::new("notes", "notes.md"),
                Attachment::new("shot", "shot.png").image(image),
                Attachment::new("sheet", "q3.xlsx").progress(55.),
                Attachment::new("deck", "deck.pptx").progress(0.),
            ],
            dismissed: dismissed.clone(),
        });
        view = Some(harness.clone());
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    Setup {
        handle,
        view: view.unwrap(),
        dismissed,
    }
}

use std::sync::Arc;

#[gpui_kit::test]
fn tiles_are_the_spec_size_and_wrap_in_the_strip_width(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        for id in ["notes", "shot", "sheet", "deck"] {
            assert_eq!(window.find(tile(id)).bounds().size, size(px(56.), px(56.)));
        }
        let first = window.find(tile("notes")).bounds();
        let second = window.find(tile("shot")).bounds();
        assert_eq!(second.left() - first.right(), px(8.), "the strip gap");
        let fourth = window.find(tile("deck")).bounds();
        assert!(
            fourth.top() > first.top(),
            "four tiles do not fit 240px, so the last wraps"
        );
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "an idle strip asks for no frame"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn only_an_uploading_tile_shows_the_percent_and_it_has_no_dismiss_yet(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(part("sheet", "percent")).is_some());
        assert!(window.try_find(part("sheet", "dismiss")).is_none());
        assert!(window.try_find(part("notes", "percent")).is_none());
        assert!(window.try_find(part("notes", "dismiss")).is_some());
        assert!(
            window.try_find(part("deck", "percent")).is_none(),
            "a queued tile has no counter"
        );
        assert!(window.try_find(part("deck", "dismiss")).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_dismiss_button_reports_the_attachment_id(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("shot", "dismiss"), cx);
        window.click(part("notes", "dismiss"), cx);
    })
    .unwrap();
    assert_eq!(setup.dismissed.borrow().as_slice(), ["shot", "notes"]);
}

#[gpui_kit::test]
fn finishing_an_upload_swaps_the_percent_for_the_dismiss_button(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    setup.view.update(cx, |harness, cx| {
        harness.attachments[2].set_progress(None);
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(part("sheet", "percent")).is_none());
        assert!(window.try_find(part("sheet", "dismiss")).is_some());
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "reduced motion lands at once"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn with_motion_the_swap_and_the_arc_take_frames(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::Off);
    setup.view.update(cx, |harness, cx| {
        harness.attachments[2].set_progress(None);
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find(part("sheet", "percent")).is_some(),
            "the counter is still fading out"
        );
        assert!(window.simulate_next_frame(cx) > 0);
    })
    .unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find(part("sheet", "percent")).is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_changing_percent_moves_the_arc_over_frames(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::Off);
    setup.view.update(cx, |harness, cx| {
        harness.attachments[2].set_progress(Some(90.));
        cx.notify();
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.simulate_next_frame(cx) > 0,
            "the arc is still drawing"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_queued_tile_shows_its_ring_so_the_wait_is_visible(cx: &mut TestAppContext) {
    let setup = setup(cx, ReduceMotion::On);
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(
            window.try_find(part("deck", "arc")).is_some(),
            "queued: the ring shows"
        );
        assert!(
            window.try_find(part("deck", "percent")).is_none(),
            "but no counter"
        );
        assert!(
            window.try_find(part("deck", "dismiss")).is_some(),
            "and it can be removed"
        );
        assert!(
            window.try_find(part("notes", "arc")).is_none(),
            "a ready tile has no ring"
        );
    })
    .unwrap();
}
