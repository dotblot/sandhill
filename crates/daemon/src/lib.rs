//! `sandhilld`: the always-running process on the analysis machine (the
//! Ubuntu server, or the Mac in local mode). It owns kernel sessions and
//! journals every output so the Mac app can disconnect and catch up later.

pub mod blobs;
pub mod files;
pub mod journal;
pub mod kernel;
pub mod kernelspecs;
pub mod memory;
pub mod notebook;
pub mod outputs;
pub mod paths;
pub mod projects;
pub mod server;
pub mod session;
pub mod sidecar;
pub mod system;
pub mod version;

pub use paths::StateDir;
pub use server::Daemon;
pub use version::build_id;
