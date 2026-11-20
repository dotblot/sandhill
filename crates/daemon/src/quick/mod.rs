//! Explore a Table (named Quick Tools in the code): session folders, the
//! What we know card, the R packages it needs, and its fixed R templates.
//! See `docs/design/quick-tools-plan.md`.

pub(crate) mod archive;
pub mod folder;
pub mod install;
mod package_plan;
pub mod python;
pub mod recognise;
pub mod record;
pub mod run;
mod runtime_plan;
pub mod setup;
