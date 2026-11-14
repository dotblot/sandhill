//! Every list of rows draws its rows the same way: the fill has the same
//! inset from the panel on its left and its right, close to the menu
//! padding, and a row is at least the row height. One table, run against
//! the select and every consumer of its row.

use gpui_cn::{
    ActiveTheme as _, Command, CommandItem, CommandState, DropdownMenu, FileEntry, FilePicker,
    FilePickerState, FolderEntry, FolderPage, FolderPicker, FolderPickerState, FolderSource,
    ListError, MemoryFiles, MenuEntry, MenuItem, MenuState, ModelEntry, ModelPicker,
    ModelPickerState, ModelProvider, PageToken, ReduceMotion, Select, SelectItem, SelectState,
    SourcePath, StatusOption, StatusSelect, StatusSelectState, Theme,
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
        cx.background_spawn(async {
            Ok(FolderPage::new([
                FolderEntry::new("alpha"),
                FolderEntry::new("beta"),
                FolderEntry::new("gamma"),
            ]))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Select,
    Status,
    Command,
    Menu,
    Model,
    Folder,
    File,
}

struct Page {
    kind: Kind,
    select: Entity<SelectState<&'static str>>,
    status: Entity<StatusSelectState>,
    command: Entity<CommandState>,
    menu: Entity<MenuState>,
    model: Entity<ModelPickerState>,
    folder: Entity<FolderPickerState>,
    file: Entity<FilePickerState>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full().p_4().flex().items_start();
        match self.kind {
            Kind::Select => root.child(Select::new("sel", &self.select)),
            Kind::Status => root.child(StatusSelect::new("status", &self.status)),
            Kind::Command => {
                root.child(div().w(px(360.)).child(Command::new("cmd", &self.command)))
            }
            Kind::Menu => root.child(
                DropdownMenu::new("menu", &self.menu)
                    .trigger(gpui_cn::Button::new("menu-trigger").label("Open"))
                    .items(|_, _| {
                        vec![
                            MenuEntry::from(MenuItem::new("one", "One")),
                            MenuEntry::from(MenuItem::new("two", "Two")),
                            MenuEntry::from(MenuItem::new("three", "Three")),
                        ]
                    }),
            ),
            Kind::Model => root.child(ModelPicker::new("model", &self.model)),
            Kind::Folder => root.child(FolderPicker::new("fp", &self.folder).open(true)),
            Kind::File => root.child(FilePicker::new("fp", &self.file).open(true)),
        }
    }
}

fn deep(path: &[&str]) -> ElementId {
    let mut id = ElementId::Name(path[0].to_string().into());
    for part in &path[1..] {
        id = ElementId::NamedChild(id.into(), part.to_string().into());
    }
    id
}

/// (kind, the click that opens it, the panel, a row)
fn table() -> Vec<(Kind, Option<ElementId>, ElementId, ElementId)> {
    vec![
        (
            Kind::Select,
            Some(deep(&["sel", "trigger"])),
            deep(&["sel", "menu"]),
            deep(&["sel", "b"]),
        ),
        (
            Kind::Status,
            Some(deep(&["status", "select", "trigger"])),
            deep(&["status", "select", "menu"]),
            deep(&["status", "select", "main"]),
        ),
        (
            Kind::Command,
            None,
            deep(&["cmd"]),
            deep(&["cmd", "calendar"]),
        ),
        (
            Kind::Menu,
            Some(ElementId::Name("menu-trigger".into())),
            deep(&["menu", "menu"]),
            deep(&["menu", "two"]),
        ),
        (
            Kind::Model,
            Some(deep(&["model", "trigger"])),
            deep(&["model", "list"]),
            deep(&["model", "acme-fast"]),
        ),
        (
            Kind::Folder,
            None,
            deep(&["fp", "list"]),
            deep(&["fp", "entry", "beta"]),
        ),
        (
            Kind::File,
            None,
            deep(&["fp", "list"]),
            deep(&["fp", "entry", "b.txt"]),
        ),
    ]
}

fn page(cx: &mut TestAppContext, kind: Kind) -> gpui_kit::WindowHandle<Root> {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let handle = cx.open_window(size(px(900.), px(900.)), |window, cx| {
        let select = cx.new(|cx| {
            SelectState::new(
                [
                    SelectItem::new("a", "Alpha"),
                    SelectItem::new("b", "Beta"),
                    SelectItem::new("c", "Gamma"),
                ],
                cx,
            )
        });
        let status = cx.new(|cx| {
            StatusSelectState::new(
                [
                    StatusOption::new("main", "main"),
                    StatusOption::new("dev", "dev"),
                    StatusOption::new("fix", "fix"),
                ],
                cx,
            )
        });
        let command = cx.new(|cx| {
            CommandState::new("Search", window, cx).with_entries([
                CommandItem::new("calendar", "Calendar"),
                CommandItem::new("search", "Search"),
                CommandItem::new("profile", "Profile"),
            ])
        });
        let menu = cx.new(MenuState::new);
        let model = cx.new(|cx| {
            ModelPickerState::new(
                [ModelProvider::new("acme", "Acme").models([
                    ModelEntry::new("acme-fast", "Alpha Fast"),
                    ModelEntry::new("acme-deep", "Alpha Deep"),
                ])],
                window,
                cx,
            )
        });
        let folder = cx.new(|cx| {
            FolderPickerState::new(window, cx)
                .with_source(Folders, window, cx)
                .with_initial("/x", window, cx)
        });
        let file = cx.new(|cx| {
            FilePickerState::new(window, cx)
                .with_source(
                    MemoryFiles::new().with_dir(
                        "/x",
                        [
                            FileEntry::file("a.txt"),
                            FileEntry::file("b.txt"),
                            FileEntry::file("c.txt"),
                        ],
                    ),
                    window,
                    cx,
                )
                .with_initial("/x", window, cx)
        });
        let view = cx.new(|_| Page {
            kind,
            select,
            status,
            command,
            menu,
            model,
            folder,
            file,
        });
        Root::new(view, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    handle
}

fn frames(handle: &gpui_kit::WindowHandle<Root>, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window((*handle).into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn every_list_insets_its_rows_the_same_on_both_sides(cx: &mut TestAppContext) {
    for (kind, opener, panel, row) in table() {
        let handle = page(cx, kind);
        frames(&handle, cx);
        if let Some(opener) = opener {
            cx.update_window(handle.into(), |_, window, cx| window.click(opener, cx))
                .unwrap();
            frames(&handle, cx);
        }
        let padding = cx.update(|cx| cx.theme().metrics.row_sm);
        cx.update_window(handle.into(), |_, window, _| {
            let panel = window.find(panel).bounds();
            let row = window.find(row).bounds();
            let left = f32::from(row.left() - panel.left());
            let right = f32::from(panel.right() - row.right());
            assert!(
                (left - right).abs() <= 0.5,
                "{kind:?}: left inset {left}, right inset {right}"
            );
            // The menu padding, and a hairline where the panel draws one.
            let expected = if matches!(kind, Kind::Model) { 4. } else { 5. };
            assert!(
                (left - expected).abs() <= 0.5,
                "{kind:?}: the inset is {expected}, not {left}"
            );
            if !matches!(kind, Kind::Model | Kind::File | Kind::Folder) {
                assert!(row.size.height >= padding, "{kind:?}: a row is a row tall");
            }
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn the_first_and_last_row_have_the_same_inset_from_a_plain_menu(cx: &mut TestAppContext) {
    for (kind, opener, panel, rows) in [
        (
            Kind::Select,
            deep(&["sel", "trigger"]),
            deep(&["sel", "menu"]),
            (deep(&["sel", "a"]), deep(&["sel", "c"])),
        ),
        (
            Kind::Menu,
            ElementId::Name("menu-trigger".into()),
            deep(&["menu", "menu"]),
            (deep(&["menu", "one"]), deep(&["menu", "three"])),
        ),
    ] {
        let handle = page(cx, kind);
        frames(&handle, cx);
        cx.update_window(handle.into(), |_, window, cx| window.click(opener, cx))
            .unwrap();
        frames(&handle, cx);
        cx.update_window(handle.into(), |_, window, _| {
            let panel = window.find(panel).bounds();
            let top = f32::from(window.find(rows.0).bounds().top() - panel.top());
            let bottom = f32::from(panel.bottom() - window.find(rows.1).bounds().bottom());
            assert!(
                (top - bottom).abs() <= 0.5,
                "{kind:?}: top inset {top}, bottom inset {bottom}"
            );
        })
        .unwrap();
    }
}
