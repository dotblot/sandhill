//! `sandhilld`: the always-running process on the analysis machine (the
//! Ubuntu server, or the Mac in local mode). It owns kernel sessions and
//! journals every output so the Mac app can disconnect and catch up later.
//! It also keeps terminals, where shells and coding agents run, and serves
//! the agents' tools (`mcp`).

pub mod accounts;
pub mod agent_commands;
pub mod agents;
pub mod archives;
pub mod bio;
pub mod blobs;
pub mod cells;
pub mod chats;
pub mod connectors;
pub mod fileops;
pub mod files;
pub mod ghost;
pub mod git;
#[doc(hidden)]
pub mod h5ad;
pub mod help;
pub mod inline;
pub mod irkernel;
pub mod jobs;
pub mod journal;
pub mod kernel;
pub mod kernel_versions;
pub mod kernelspecs;
pub mod lifecycle;
pub mod logfile;
pub mod lsp;
pub mod mcp;
pub mod memory;
pub mod notebook;
pub mod outputs;
pub mod packages;
pub mod pages;
pub mod patch;
pub mod paths;
pub mod preview;
pub mod projects;
pub mod qmd;
pub mod quirks;
pub mod redraw;
pub mod server;
pub mod session;
pub mod sidecar;
pub mod skills;
pub mod startup;
pub mod system;
pub mod tables;
pub mod terminal;
pub mod tunnels;
pub mod usage;
pub mod variable_search;
pub mod variables;
pub mod version;
pub mod view;
pub mod viewer;
pub mod windows;

pub use paths::StateDir;
pub use server::{Daemon, DaemonOptions};
pub use version::build_id;
