//! `sandhilld`: the always-running process on the analysis machine (the
//! Ubuntu server, or the Mac in local mode). It owns kernel sessions and
//! journals every output so the Mac app can disconnect and catch up later.
//! It also keeps terminals, where shells and coding agents run, and serves
//! the agents' tools (`mcp`).

pub mod agents;
pub mod blobs;
pub mod cells;
pub mod files;
pub mod journal;
pub mod kernel;
pub mod kernelspecs;
pub mod logfile;
pub mod mcp;
pub mod memory;
pub mod notebook;
pub mod outputs;
pub mod paths;
pub mod projects;
pub mod qmd;
pub mod server;
pub mod session;
pub mod sidecar;
pub mod system;
pub mod terminal;
pub mod version;
pub mod windows;

pub use paths::StateDir;
pub use server::{Daemon, DaemonOptions};
pub use version::build_id;
