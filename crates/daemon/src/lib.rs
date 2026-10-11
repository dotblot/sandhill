//! `sandhilld`: the always-running process on the analysis machine (the
//! Ubuntu server, or the Mac in local mode). It owns kernel sessions and
//! journals every output so the Mac app can disconnect and catch up later.

pub mod blobs;
pub mod journal;
pub mod kernel;
pub mod paths;
pub mod server;
pub mod session;

pub use paths::StateDir;
pub use server::Daemon;
