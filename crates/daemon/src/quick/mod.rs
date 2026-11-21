//! Explore a Table (named Quick Tools in the code): session folders, the
//! What we know card, the R packages it needs, and its fixed R templates.
//! See `docs/design/quick-tools-plan.md`.

pub(crate) mod archive;
pub(crate) mod env_actions;
mod env_check;
pub(crate) mod env_updates;
pub(crate) mod environments;
pub mod folder;
pub mod install;
pub(crate) mod own;
mod package_plan;
pub(crate) mod package_search;
pub mod python;
pub mod recognise;
pub mod record;
pub mod run;
mod runtime_plan;
pub mod setup;
mod undo;
