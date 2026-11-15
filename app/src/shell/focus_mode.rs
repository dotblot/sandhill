//! A temporary editor-only layout. Never written to project preferences.
use gpui_kit::*;

use crate::shell::pane_sizes::Vertical;
use crate::workbench::Workbench;

gpui_kit::actions!(sandhill, [FocusEditor]);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Layout {
    pub files: bool,
    pub agents: bool,
    pub console: bool,
    pub editor: bool,
    pub maximized: Option<crate::shell::pane_sizes::Panel>,
    pub left: Vertical,
    pub middle: Vertical,
    pub left_share: f32,
    pub agents_share: f32,
    pub variables_share: f32,
    pub console_share: f32,
}

#[derive(Default)]
pub(crate) struct FocusMode {
    previous: Option<Layout>,
}

impl FocusMode {
    pub(crate) fn active(&self) -> bool {
        self.previous.is_some()
    }

    fn toggle(&mut self, current: Layout) -> Layout {
        if let Some(previous) = self.previous.take() {
            previous
        } else {
            self.previous = Some(current);
            Layout {
                files: false,
                agents: false,
                console: false,
                editor: true,
                maximized: None,
                middle: Vertical::Top,
                ..current
            }
        }
    }
}

impl Workbench {
    pub(crate) fn toggle_focus_editor(&mut self, cx: &mut Context<Self>) {
        let layout = self.focus_mode.toggle(Layout {
            files: self.show_files,
            agents: self.show_agents,
            console: self.show_console,
            editor: self.show_middle,
            maximized: self.maximized_panel,
            left: self.left_vertical,
            middle: self.middle_vertical,
            left_share: self.left_share,
            agents_share: self.agents_share,
            variables_share: self.variables_share,
            console_share: self.console_share,
        });
        self.show_files = layout.files;
        self.show_agents = layout.agents;
        self.show_console = layout.console;
        self.show_middle = layout.editor;
        self.maximized_panel = layout.maximized;
        self.left_vertical = layout.left;
        self.middle_vertical = layout.middle;
        self.left_share = layout.left_share;
        self.agents_share = layout.agents_share;
        self.variables_share = layout.variables_share;
        self.console_share = layout.console_share;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{FocusMode, Layout, Vertical};

    #[test]
    fn restores_exact_layout_even_after_intervening_changes() {
        let original = Layout {
            files: false,
            agents: true,
            console: true,
            editor: false,
            maximized: Some(crate::shell::pane_sizes::Panel::Console),
            left: Vertical::Bottom,
            middle: Vertical::Bottom,
            left_share: 0.19,
            agents_share: 0.31,
            variables_share: 0.42,
            console_share: 0.61,
        };
        let mut mode = FocusMode::default();
        let focused = mode.toggle(original);
        assert!(mode.active());
        assert!(!focused.files && !focused.agents && !focused.console);
        assert_eq!(focused.middle, Vertical::Top);
        assert_eq!(
            mode.toggle(Layout {
                files: true,
                console_share: 0.2,
                ..focused
            }),
            original
        );
        assert!(!mode.active());
        assert_eq!(mode.toggle(original), focused);
    }
}
