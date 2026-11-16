//! Choices for every project on this Mac (Huan, 2026-11-15): saved once and
//! applied to each open project, whether made on Welcome's General page or
//! with the switch beside what they change.

use gpui_kit::*;

use crate::panes::tree::SortBy;
use crate::workbench::Workbench;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Choice {
    /// Notebooks' and Quarto documents' runs show in the Console too.
    NotebookConsole(bool),
    /// Their plots show in Plots too.
    NotebookPlots(bool),
    /// Notebooks suggest the next cell after each run.
    GhostCells(bool),
    /// Notebooks scroll to and light up the cells an agent changes.
    FollowAgent(bool),
    /// How the Files pane sorts.
    Sort(SortBy),
    /// Whether long lines wrap in a file type (`prefs::editor_soft_wrap`).
    Wrap(&'static str, bool),
    /// Whether an agent is offered (Welcome's Agents page).
    Agent(&'static str, bool),
    /// Whether the status bar shows, and the daemon measures, resources.
    Resources(bool),
    /// Whether the status bar has the device's usage at all.
    Usage(bool),
    /// Whether the window is frosted glass (`prefs::glass`).
    Glass(bool),
}

/// Saves a choice, then applies it to every open project once the current
/// event is over (the caller may be one of those projects).
pub(crate) fn choose(choice: Choice, cx: &mut App) {
    match choice {
        Choice::NotebookConsole(on) => crate::prefs::save_notebook_console(on),
        Choice::NotebookPlots(on) => crate::prefs::save_notebook_plots(on),
        Choice::GhostCells(on) => crate::prefs::save_ghost_cells(on),
        Choice::FollowAgent(on) => crate::prefs::save_follow_agent(on),
        Choice::Sort(by) => crate::prefs::save_tree_sort(by.key()),
        Choice::Wrap(kind, on) => crate::prefs::save_editor_soft_wrap(kind, on),
        Choice::Agent(kind, on) => crate::prefs::save_agent_off(kind, !on),
        Choice::Resources(shown) => crate::prefs::save_resources_shown(shown),
        Choice::Usage(on) => crate::prefs::save_usage_in_status_bar(on),
        Choice::Glass(on) => crate::prefs::save_glass(on),
    }
    if matches!(choice, Choice::Glass(_)) {
        // Every window, Welcome's too: its background, then the palette
        // that tints it.
        cx.defer(|cx| {
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, cx| {
                    window.set_background_appearance(crate::look::window_background());
                    crate::look::set_appearance(window.appearance(), cx);
                });
            }
            cx.refresh_windows();
        });
        return;
    }
    cx.defer(move |cx| {
        crate::shell::app_window::update_projects(cx, move |workbench, window, cx| {
            workbench.apply_choice(choice, window, cx)
        });
        // Welcome's General page shows it too.
        cx.refresh_windows();
    });
}

impl Workbench {
    fn apply_choice(&mut self, choice: Choice, window: &mut Window, cx: &mut Context<Self>) {
        match choice {
            Choice::NotebookConsole(on) => self.notebook_console = on,
            Choice::NotebookPlots(on) => {
                if self.notebook_plots != on {
                    self.notebook_plots = on;
                    // Plots reads its sessions' plots again, with or without
                    // notebooks'.
                    self.plot_history = Default::default();
                }
            }
            Choice::GhostCells(on) => {
                self.ghost_cells = on;
                if !on && let Some(open) = self.open.as_mut() {
                    open.ghost = None;
                }
            }
            // Notebooks read it from prefs as the agent edits them.
            Choice::FollowAgent(_) => {}
            Choice::Sort(by) => self.apply_sort(by, cx),
            Choice::Wrap(kind, on) => self.apply_editor_wrap(kind, on, window, cx),
            Choice::Agent(_, on) => {
                if let Some(agents) = &self.agents {
                    agents.update(cx, |agents, cx| {
                        // A new conversation set to an agent turned off
                        // starts with another.
                        if crate::agents::turned_off(agents.new_kind) {
                            agents.new_kind = crate::agents::default_kind().kind;
                        }
                        // Turned on, its models are read.
                        if on {
                            agents.refresh(cx);
                        }
                        cx.notify();
                    });
                }
            }
            Choice::Resources(_) | Choice::Usage(_) => {
                if self.measuring() {
                    self.read_stats(cx);
                } else {
                    self.machine.stats = None;
                    self.machine.resource = None;
                }
            }
            // Applied to every window by `choose`.
            Choice::Glass(_) => {}
        }
        cx.notify();
    }
}
