//! The folder picker and the file picker end in the same footer: the same
//! buttons, the same sizes, the same gap, and the same alignment.

use gpui_cn::{
    FileEntry, FilePicker, FilePickerState, FolderEntry, FolderPage, FolderPicker,
    FolderPickerState, FolderSource, ListError, MemoryFiles, PageToken, ReduceMotion, SourcePath,
    Theme,
};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, TestAppContext, Window, base::Root, div, px, size, test::TestWindowExt as _,
};

struct Folders;

impl FolderSource for Folders {
    fn list(
        &self,
        _: &SourcePath,
        _: Option<PageToken>,
        cx: &mut gpui_kit::App,
    ) -> gpui_kit::Task<Result<FolderPage, ListError>> {
        cx.background_spawn(async { Ok(FolderPage::new([FolderEntry::new("a")])) })
    }
}

struct FolderHarness(Entity<FolderPickerState>);
struct FileHarness(Entity<FilePickerState>);

impl Render for FolderHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(FolderPicker::new("fp", &self.0).open(true))
    }
}

impl Render for FileHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(FilePicker::new("fp", &self.0).open(true))
    }
}

type Rect = gpui_kit::Bounds<gpui_kit::Pixels>;

fn part(name: &str) -> ElementId {
    ElementId::NamedChild(ElementId::from("fp").into(), name.to_string().into())
}

fn measure(
    cx: &mut TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    confirm: &str,
) -> (Rect, Rect, Rect) {
    cx.run_until_parked();
    for _ in 0..2 {
        cx.update_window(handle, |_, window, cx| {
            window.activate_window();
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }
    cx.update_window(handle, |_, window, _| {
        (
            window.find(part("cancel")).bounds(),
            window.find(part(confirm)).bounds(),
            window.find(part("popup")).bounds(),
        )
    })
    .unwrap()
}

#[gpui_kit::test]
fn both_dialogs_end_in_the_same_footer(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let folder = cx.open_window(size(px(900.), px(900.)), |window, cx| {
        let state = cx.new(|cx| {
            FolderPickerState::new(window, cx)
                .with_source(Folders, window, cx)
                .with_initial("/x", window, cx)
        });
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            FolderHarness(state)
        });
        Root::new(harness, window, cx)
    });
    let file = cx.open_window(size(px(900.), px(900.)), |window, cx| {
        let state = cx.new(|cx| {
            FilePickerState::new(window, cx)
                .with_source(
                    MemoryFiles::new().with_dir("/x", [FileEntry::file("a.txt")]),
                    window,
                    cx,
                )
                .with_initial("/x", window, cx)
        });
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            FileHarness(state)
        });
        Root::new(harness, window, cx)
    });
    let (fc, fu, fd) = measure(cx, folder.into(), "use");
    let (ic, io_, id) = measure(cx, file.into(), "open");
    assert_eq!(fc.size.height, ic.size.height, "Cancel is as tall");
    assert_eq!(
        fu.size.height, io_.size.height,
        "the confirm button is as tall"
    );
    let gap = |cancel: Rect, confirm: Rect| f32::from(confirm.left() - cancel.right());
    assert_eq!(gap(fc, fu), gap(ic, io_), "the same gap");
    let inset = |confirm: Rect, popup: Rect| f32::from(popup.right() - confirm.right());
    assert_eq!(inset(fu, fd), inset(io_, id), "the same trailing inset");
    let below = |confirm: Rect, popup: Rect| f32::from(popup.bottom() - confirm.bottom());
    assert_eq!(below(fu, fd), below(io_, id), "the same bottom inset");
}
