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
pub mod install_failure;
pub(crate) mod interrupted;
pub(crate) mod own;
mod own_copy;
pub(crate) mod own_lock;
mod package_inventory;
pub(crate) mod package_names;
mod package_plan;
pub(crate) mod package_search;
pub(crate) mod package_target;
pub(crate) mod project_packages;
pub mod python;
pub mod recognise;
pub mod record;
pub mod run;
mod runtime_plan;
pub mod setup;
pub(crate) mod sources;
mod space;
mod undo;
mod upgrades;

#[cfg(test)]
#[path = "../../tests/common/mod.rs"]
pub(crate) mod test_common;
