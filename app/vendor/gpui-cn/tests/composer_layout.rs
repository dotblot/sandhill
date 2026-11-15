//! The composer's toolbar at narrow and wide widths: its controls never
//! overlap, and long labels truncate.

use gpui_cn::{
    Composer, ComposerState, ModelEntry, ModelPickerState, ModelProvider, PermissionMode,
    PermissionState, ReduceMotion, Theme,
};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, base::Root, div, px, size, test::TestWindowExt as _,
};

struct Harness {
    state: Entity<ComposerState>,
    width: f32,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(self.width))
            .child(Composer::new("composer", &self.state).no_status())
    }
}

fn part(name: &str) -> ElementId {
    ElementId::NamedChild(ElementId::from("composer").into(), name.to_string().into())
}

#[gpui_kit::test]
fn toolbar_controls_never_overlap_at_any_width(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    for width in [240., 280., 320., 400., 640.] {
        let handle = cx.open_window(size(px(800.), px(400.)), |window, cx| {
            let models = cx.new(|cx| {
                ModelPickerState::new(
                    [ModelProvider::new("acme", "Acme").models([ModelEntry::new(
                        "long",
                        "Acme Extraordinarily Long Model Name Mini",
                    )
                    .effort(true)])],
                    window,
                    cx,
                )
                .with_selected("long")
            });
            let permission = cx.new(|cx| {
                PermissionState::new(cx).with_modes([PermissionMode::new(
                    "full",
                    "Full access with no prompts at all",
                )])
            });
            let state = cx.new(|cx| {
                ComposerState::new(window, cx)
                    .with_models(models, window, cx)
                    .with_permission(permission, window, cx)
            });
            let harness = cx.new(|_| Harness { state, width });
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
        cx.update_window(handle.into(), |_, window, _| {
            let card = window.find(part("card")).bounds();
            let found: Vec<_> = ["add", "permission", "models", "send"]
                .iter()
                .filter_map(|name| window.try_find(part(name)).map(|e| (*name, e.bounds())))
                .collect();
            assert!(found.len() >= 3, "controls found at {width}: {found:?}");
            for (at, (name, bounds)) in found.iter().enumerate() {
                assert!(
                    bounds.left() >= card.left() && bounds.right() <= card.right(),
                    "{name} inside the card at {width}: {bounds:?} {card:?}"
                );
                for (other, theirs) in &found[at + 1..] {
                    assert!(
                        !bounds.intersects(theirs),
                        "{name} overlaps {other} at {width}: {bounds:?} {theirs:?}"
                    );
                }
            }
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn triggers_keep_a_label_or_become_icons_and_the_send_button_keeps_its_size(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let mut send_size = None;
    let mut collapsed = Vec::new();
    for width in [240., 280., 320., 400., 640.] {
        let handle = cx.open_window(size(px(800.), px(400.)), |window, cx| {
            let models = cx.new(|cx| {
                ModelPickerState::new(
                    [ModelProvider::new("acme", "Acme").models([ModelEntry::new(
                        "long",
                        "Acme Extraordinarily Long Model Name Mini",
                    )
                    .effort(true)])],
                    window,
                    cx,
                )
                .with_selected("long")
            });
            let permission = cx.new(|cx| {
                PermissionState::new(cx).with_modes([PermissionMode::new(
                    "full",
                    "Full access with no prompts at all",
                )
                .icon(gpui_kit::assets::IconName::LockOpen)])
            });
            let state = cx.new(|cx| {
                ComposerState::new(window, cx)
                    .with_models(models, window, cx)
                    .with_permission(permission, window, cx)
            });
            let harness = cx.new(|_| Harness { state, width });
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
        cx.update_window(handle.into(), |_, window, _| {
            let control = px(24.);
            let trigger = |name: &str| {
                window
                    .try_find(ElementId::NamedChild(part(name).into(), "trigger".into()))
                    .map(|e| e.bounds())
            };
            let found: Vec<_> = ["permission", "effort", "models"]
                .iter()
                .map(|name| (*name, trigger(name).expect("a trigger")))
                .collect();
            for (name, bounds) in &found {
                assert!(
                    bounds.size.width >= control,
                    "{name} has a width at {width}: {bounds:?}"
                );
                let icon_only = bounds.size.width <= control + px(0.5);
                if !icon_only {
                    assert!(
                        bounds.size.width >= control * 2.,
                        "{name} keeps an icon and a label at {width}: {bounds:?}"
                    );
                }
            }
            let send = window.find(part("send")).bounds().size;
            assert_eq!(
                *send_size.get_or_insert(send),
                send,
                "the send button never shrinks"
            );
            collapsed.push((
                width,
                found
                    .iter()
                    .map(|(_, b)| b.size.width <= control + px(0.5))
                    .collect::<Vec<_>>(),
            ));
        })
        .unwrap();
    }
    let (_, narrow) = &collapsed[0];
    let (_, wide) = collapsed.last().unwrap();
    assert!(
        narrow.iter().any(|icon| *icon),
        "a narrow toolbar has icons"
    );
    assert!(
        wide.iter().all(|icon| !*icon),
        "a wide toolbar keeps its labels"
    );
}
