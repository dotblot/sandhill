//! The side and bottom panes: Variables' kernels and resources, Help,
//! Packages, Files, Plots, the Viewer and the data explorer on the left;
//! the Terminal and Jobs under the editor. (The Console is the workbench's
//! own, in `workbench/console.rs`.)

pub(crate) mod explorer;
pub(crate) mod files;
pub(crate) mod help;
pub(crate) mod irkernel;
pub(crate) mod jobs;
pub(crate) mod kernel_menu;
pub(crate) mod machine;
pub(crate) mod object_explorer;
pub(crate) mod packages;
pub(crate) mod plots;
pub(crate) mod terminal;
pub(crate) mod tree;
pub(crate) mod tree_actions;
pub(crate) mod viewer;
