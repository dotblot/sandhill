//! shadcn-style components for GPUI, built on `gpui-base`.
//!
//! gpui-cn depends on `gpui-kit` with the component layer off, so it adds
//! GPUI and `gpui-base` and nothing else.
//!
//! Initialize `gpui-base` first (through `gpui_kit::init`), then call
//! [`init`] once before opening windows. gpui-cn adds itself to every
//! window whose first level is gpui-base's `Root`, which
//! `gpui_kit::open_window` puts there:
//!
//! ```no_run
//! use gpui_kit::*;
//!
//! struct Hello;
//!
//! impl Render for Hello {
//!     fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
//!         div().p_4().child(
//!             gpui_cn::Button::new("save")
//!                 .primary()
//!                 .label("Save")
//!                 .on_click(|_, window, cx| {
//!                     let _ = window.prompt(PromptLevel::Info, "Saved", None, &["OK"], cx);
//!                 }),
//!         )
//!     }
//! }
//!
//! fn main() {
//!     gpui_kit::application().run(|cx| {
//!         gpui_kit::init(cx);
//!         gpui_cn::init(cx);
//!         gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| cx.new(|_| Hello))
//!             .expect("failed to open window");
//!     });
//! }
//! ```

mod avatar;
mod badge;
mod button;
mod collapse;
mod command;
mod command_dialog;
mod composer;
mod dialog;
pub mod dock;
mod file_picker;
mod folder_picker;
pub mod fuzzy;
mod glide;
mod icon;
mod input;
mod label;
mod looping;
mod menu;
mod middle_text;
mod nav;
mod path_browser;
mod plural;
mod popover;
mod progress;
mod radio;
pub mod remote;
mod root;
mod scroll_area;
mod select;
pub mod shell;
mod sidebar;
mod skeleton;
mod slider;
mod spinner;
mod switch;
mod tabs;
mod tag;
#[cfg(feature = "ghostty")]
pub mod terminal;
pub mod theme;
mod theme_mode_picker;
mod title_bar;
mod tooltip;
mod tooltip_host;
mod touch_selection;
#[cfg(feature = "webview")]
pub mod webview;

pub use avatar::{Avatar, AvatarGroup, AvatarSize};
pub use badge::Badge;
pub use button::{Button, ButtonSize, ButtonVariant};
pub use command::{
    Command, CommandEntry, CommandEvent, CommandGroup, CommandItem, CommandRow,
    CommandSearchHandler, CommandState,
};
pub use command_dialog::CommandDialog;
pub use composer::{
    Attachment, AttachmentKind, AttachmentStatus, AttachmentStrip, AttachmentTile, Composer,
    ComposerAssets, ComposerEvent, ComposerState, ComposerStatusTab, ContextMeter, EFFORT_LABELS,
    EffortMenu, ModelEntry, ModelPicker, ModelPickerEvent, ModelPickerState, ModelProvider,
    ModelView, PermissionEvent, PermissionMenu, PermissionMode, PermissionState, PermissionTone,
    SEARCH_MIN_OPTIONS, StatusOption, StatusSelect, StatusSelectEvent, StatusSelectState,
};
pub use dialog::Dialog;
pub use dock::DockSkin;
pub use file_picker::{
    FileEntry, FileFilter, FileKind, FileListing, FileLoaded, FilePage, FilePicker,
    FilePickerEvent, FilePickerState, FileSource, LocalFiles, MemoryFiles,
};
pub use folder_picker::{
    FolderEntry, FolderPage, FolderPicker, FolderPickerEvent, FolderPickerState, FolderSource,
    Listing, Loaded, LocalFolders, MemoryFolders, MoreState, PageToken,
};
pub use glide::{Glide, GlideState};
pub use gpui_kit;
pub use gpui_kit::base::input::{InputEvent, InputState, TextareaState};
pub use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
pub use icon::{Icon, IconSource};
pub use input::{Field, Input, Textarea};
pub use label::Label;
pub use menu::{
    ContextMenu, DropdownMenu, MenuAnchor, MenuBar, MenuBarMenu, MenuBarState, MenuEntry,
    MenuEvent, MenuItem, MenuRowState, MenuState, MenuSubmenu,
};
pub use nav::{
    NavButtons, NavMotion, NavOperation, NavPage, NavStack, NavStackEvent, NavStackExt,
    NavStackState,
};
pub use path_browser::{CreateFolderError, ListError, PathStyle, SourcePath};
pub use popover::Popover;
pub use progress::Progress;
pub use radio::{Radio, RadioGroup, RadioMark};
pub use scroll_area::ScrollArea;
pub use select::{
    SearchHandler, Select, SelectEntry, SelectEvent, SelectItem, SelectRow, SelectState,
    SelectValue,
};
pub use sidebar::{
    Sidebar, SidebarCollapsible, SidebarEvent, SidebarGroup, SidebarLayout, SidebarMenuButton,
    SidebarMenuSize, SidebarMenuSkeleton, SidebarMenuSub, SidebarSeparator, SidebarSide,
    SidebarState, SidebarTrigger,
};
pub use skeleton::Skeleton;
pub use slider::Slider;
pub use spinner::Spinner;
pub use switch::Switch;
pub use tabs::{Tab, Tabs, TabsEvent, TabsState};
pub use tag::{Tag, TagVariant};
pub use theme::{
    ActiveTheme, MetricTokens, ReduceMotion, Theme, ThemeConfig, ThemeMode, ThemeScope, ThemeTokens,
};
pub use theme_mode_picker::ThemeModePicker;
pub use title_bar::TitleBar;
pub use tooltip::{Tooltip, TooltipExt, TooltipTrigger};
pub use tooltip_host::TooltipHost;
#[cfg(feature = "webview")]
pub use webview::{BrowserEvent, BrowserState, WebFrame};

use gpui_kit::{App, Global};

/// Everything an application normally imports from gpui-cn.
pub mod prelude {
    #[cfg(all(
        feature = "ghostty",
        not(any(target_os = "ios", target_os = "android"))
    ))]
    pub use crate::terminal::LocalTerminalOptions;
    #[cfg(feature = "ghostty")]
    pub use crate::terminal::{
        Terminal, TerminalAppearance, TerminalColors, TerminalConfig, TerminalEvent, TerminalState,
    };
    #[cfg(feature = "webview")]
    pub use crate::webview::{BrowserState, WebFrame, WebView};
    pub use crate::{
        ActiveTheme, Avatar, AvatarGroup, AvatarSize, Badge, Button, ButtonSize, ButtonVariant,
        Command, CommandDialog, CommandEntry, CommandGroup, CommandItem, CommandState, ContextMenu,
        Dialog, DockSkin, DropdownMenu, Field, FilePicker, FilePickerState, FolderPicker,
        FolderPickerState, Icon, Input, InputState, Label, MenuBar, MenuBarMenu, MenuBarState,
        MenuEntry, MenuItem, MenuState, MenuSubmenu, NavButtons, NavMotion, NavStack, NavStackExt,
        NavStackState, Popover, Progress, Radio, RadioGroup, RadioMark, ReduceMotion, ScrollArea,
        Select, SelectEntry, SelectItem, SelectState, Sidebar, SidebarCollapsible, SidebarGroup,
        SidebarLayout, SidebarMenuButton, SidebarMenuSub, SidebarSeparator, SidebarSide,
        SidebarState, SidebarTrigger, Slider, SliderEvent, SliderState, SliderValue, Spinner,
        Switch, Tab, Tabs, TabsEvent, TabsState, Tag, TagVariant, Textarea, TextareaState, Theme,
        ThemeMode, ThemeModePicker, ThemeScope, TitleBar, TooltipExt,
    };
    pub use gpui_kit::base::{Disableable, Placement, Selectable, StyledExt};
    pub use gpui_kit::prelude::FluentBuilder;
}

/// Marks that [`init`] ran, so a second call is a no-op.
struct Initialized;

impl Global for Initialized {}

/// Initializes gpui-cn. Call it once, after `gpui_base::init` (normally
/// through `gpui_kit::init`) and before opening windows.
///
/// Installs the built-in light and dark themes and projects them onto
/// `gpui_base::Theme`, as `gpui_component::init` does for its own theme.
/// With the `jetbrains-mono` feature it also registers JetBrains Mono
/// with the text system and makes it the code font. See
/// [`theme::fonts`].
/// Calling it twice changes nothing. In an application that also runs
/// `gpui_component::init`, the later of the two owns the base theme and
/// the other layer follows its colors.
///
/// # Panics
///
/// If `gpui_base::init` has not run. gpui-cn never initializes base itself:
/// the host owns that, as it does for every other layer on base.
pub fn init(cx: &mut App) {
    if cx.has_global::<Initialized>() {
        return;
    }
    assert!(
        base_initialized(cx),
        "gpui_cn::init needs gpui_base::init first; call gpui_kit::init(cx) before it"
    );
    theme::fonts::register(cx);
    Theme::init(cx);
    root::init(cx);
    select::init(cx);
    menu::init(cx);
    command::init(cx);
    composer::init_model_picker(cx);
    path_browser::init(cx);
    folder_picker::init(cx);
    file_picker::init(cx);
    cx.set_global(Initialized);
}

/// Whether `gpui_base::init` has run in this application.
///
/// `gpui_base::init` installs `GlobalState`, which nothing else creates, so
/// its presence is the signal.
fn base_initialized(cx: &App) -> bool {
    cx.has_global::<gpui_kit::base::GlobalState>()
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn init_is_idempotent_and_skips_base_when_already_initialized(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert!(!super::base_initialized(cx));
            gpui_kit::init(cx);
            assert!(super::base_initialized(cx));
            super::init(cx);
            super::init(cx);
            assert!(cx.has_global::<super::Initialized>());
        });
    }

    #[gpui_kit::test]
    #[should_panic(expected = "needs gpui_base::init first")]
    fn init_refuses_to_run_before_base(cx: &mut TestAppContext) {
        cx.update(super::init);
    }
}
