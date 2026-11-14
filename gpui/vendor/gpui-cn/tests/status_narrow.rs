//! A narrow status tab keeps its labels at a minimum, then turns its
//! selects into icons from the right, and never overlaps.

use gpui_cn::{
    ActiveTheme as _, ComposerStatusTab, ReduceMotion, StatusOption, StatusSelect,
    StatusSelectState, Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, assets::IconName, div, px, size, test::TestWindowExt as _,
};

struct Narrow {
    width: f32,
    selects: [Entity<StatusSelectState>; 3],
}

impl Render for Narrow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(self.width)).child(
            ComposerStatusTab::new("status")
                .select(StatusSelect::new("project", &self.selects[0]).icon(IconName::Folder))
                .select(StatusSelect::new("device", &self.selects[1]).icon(IconName::Laptop))
                .select(StatusSelect::new("branch", &self.selects[2]).icon(IconName::GitBranch))
                .context(57.),
        )
    }
}

fn child(parent: &str, name: &str) -> ElementId {
    if name == "context" {
        return ElementId::NamedChild(
            ElementId::Name(parent.to_string().into()).into(),
            name.to_string().into(),
        );
    }
    ElementId::NamedChild(
        ElementId::NamedChild(
            ElementId::Name(parent.to_string().into()).into(),
            "select".into(),
        )
        .into(),
        name.to_string().into(),
    )
}

fn start(cx: &mut TestAppContext, width: f32, touch: bool) -> gpui_kit::WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| {
            theme.reduce_motion = ReduceMotion::On;
            theme.touch = touch;
        });
    });
    let handle = cx.open_window(size(px(800.), px(300.)), |window, cx| {
        let one = |name: &'static str, cx: &mut Context<StatusSelectState>| {
            StatusSelectState::new([StatusOption::new(name, name)], cx)
        };
        let selects = [
            cx.new(|cx| one("sample-application-monorepo", cx)),
            cx.new(|cx| one("Local machine in the office", cx)),
            cx.new(|cx| one("feature/composer-redesign-with-a-long-name", cx)),
        ];
        let harness = cx.new(|_| Narrow { width, selects });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
    }
    handle
}

#[gpui_kit::test]
fn labels_keep_a_minimum_then_selects_become_icons_from_the_right(cx: &mut TestAppContext) {
    let _ = start(cx, 720., false);
    let control = cx.update(|cx| cx.theme().metrics.control_sm);
    let mut collapsed_at = Vec::new();
    for width in [200., 280., 360., 480., 720.] {
        let handle = start(cx, width, false);
        let widths = cx
            .update_window(handle.into(), |_, window, _| {
                let tab = window.find("status").bounds();
                let items: Vec<_> = ["project", "device", "branch"]
                    .iter()
                    .map(|name| window.find(child(name, "trigger")).bounds())
                    .collect();
                for (at, item) in items.iter().enumerate() {
                    assert!(
                        item.size.height >= control,
                        "hit target at {width}: {item:?}"
                    );
                    assert!(
                        item.left() >= tab.left() && item.right() <= tab.right(),
                        "inside the tab at {width}: {item:?} {tab:?}"
                    );
                    for other in &items[at + 1..] {
                        assert!(!item.intersects(other), "overlap at {width}");
                    }
                }
                let meter = window.find(child("status", "context")).bounds();
                assert!(
                    items.last().unwrap().right() <= meter.left(),
                    "meter clear at {width}"
                );
                items.iter().map(|item| item.size.width).collect::<Vec<_>>()
            })
            .unwrap();
        let icon_only: Vec<bool> = widths.iter().map(|w| *w <= control + px(0.5)).collect();
        for (at, only) in icon_only.iter().enumerate() {
            if !only {
                assert!(
                    widths[at] >= control * 2.,
                    "a label keeps its minimum at {width}"
                );
            }
            if *only {
                assert!(
                    icon_only[at..].iter().all(|later| *later),
                    "icons go from the right at {width}: {icon_only:?}"
                );
            }
        }
        collapsed_at.push((width, icon_only));
    }
    let count = |width: f32| {
        collapsed_at
            .iter()
            .find(|(w, _)| *w == width)
            .map(|(_, only)| only.iter().filter(|o| **o).count())
            .unwrap()
    };
    assert!(count(200.) >= count(280.) && count(280.) >= count(360.));
    assert!(count(200.) >= 2, "a very narrow tab keeps icons");
    assert_eq!(count(720.), 0, "a wide tab keeps every label");
}

#[gpui_kit::test]
fn the_menu_of_an_icon_item_hangs_from_the_item(cx: &mut TestAppContext) {
    let handle = start(cx, 200., false);
    cx.update_window(handle.into(), |_, window, cx| {
        window.click(child("branch", "trigger"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(handle.into(), |_, window, _| {
        let item = window.find(child("branch", "trigger")).bounds();
        let menu = window.find(child("branch", "menu")).bounds();
        assert!(
            menu.top() >= item.bottom() || menu.bottom() <= item.top(),
            "the menu does not cover its item: {item:?} {menu:?}"
        );
        assert!(
            (f32::from(menu.left() - item.left())).abs() < 1.5,
            "lined up: {item:?} {menu:?}"
        );
    })
    .unwrap();
}
