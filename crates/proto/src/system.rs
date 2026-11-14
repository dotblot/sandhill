//! The daemon and its machine: status, log, resource use, tunnels, served pages and language servers.

use super::*;

/// What came back through a tunnel: `data` (base64), or `closed` once the
/// connection on the daemon's machine has ended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TunnelData {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LspStarted {
    pub lsp: String,
    /// The merged LSP InitializeResult. Position encoding is always UTF-16.
    pub capabilities: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LspMessage {
    pub lsp: String,
    pub message: Value,
}

/// Where the daemon serves a page: `http://127.0.0.1:{port}{path}` on its
/// machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageUrl {
    pub port: u16,
    /// Starts with `/`, percent-encoded.
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PingReply {
    pub protocol_version: u32,
    pub daemon_version: String,
    /// Home directory of the user the daemon runs as.
    #[serde(default)]
    pub home: String,
    #[serde(default)]
    pub hostname: String,
    /// Identifies the daemon's executable, so `attach --start` can replace
    /// a daemon an older app left running. Empty from older daemons.
    #[serde(default)]
    pub build: String,
    /// The user the daemon runs as. Empty from older daemons.
    #[serde(default)]
    pub user: String,
    /// When the daemon started, in milliseconds since the Unix epoch; 0
    /// from older daemons.
    #[serde(default)]
    pub started_ms: i64,
    /// What started the daemon: `app` (`attach --start`), `launchd` or
    /// `systemd`. Empty when it was started by hand, and from older daemons.
    #[serde(default)]
    pub started_by: String,
    /// `ready`, `stopped` or `stopping`; empty from older daemons.
    #[serde(default)]
    pub lifecycle: String,
}

/// The end of the daemon's log file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogTail {
    pub path: String,
    /// Whole lines from the end of the file.
    pub text: String,
    /// The file's size in bytes.
    pub size: u64,
}

/// A snapshot of the machine the daemon runs on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemStats {
    pub cpu: CpuStats,
    #[serde(default)]
    pub memory: Option<MemoryStats>,
    /// Devices discovered on this host, including devices with unreadable metrics.
    #[serde(default)]
    pub gpus: Vec<GpuStats>,
    /// Older daemons did not distinguish absent devices from failed discovery.
    #[serde(default)]
    pub gpu_status: GpuStatus,
    /// Bounded source error, even if another GPU source succeeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<DiskStats>,
    /// Memory held by each running session's kernel, with the processes it started.
    #[serde(default)]
    pub sessions: Vec<SessionMemory>,
    /// What each running job uses, with the processes it started.
    #[serde(default)]
    pub jobs: Vec<JobUsage>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GpuStatus {
    #[default]
    Unknown,
    Available,
    Absent,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CpuStats {
    /// Share of all cores busy, 0 to 100; absent when sampling failed.
    #[serde(default)]
    pub percent: Option<f64>,
    pub cores: u32,
    /// The processor's name, like "Apple M3 Pro", when the system says.
    #[serde(default)]
    pub model: String,
    /// One-minute load average, when readable.
    #[serde(default)]
    pub load: Option<f64>,
    /// Hottest CPU sensor in Celsius, when exposed without elevated privileges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryStats {
    pub total: u64,
    /// Bytes in use: what programs hold, not counting caches the system can drop.
    pub used: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuStats {
    pub name: String,
    /// Utilization, 0 to 100. Missing is not an idle device.
    #[serde(default)]
    pub percent: Option<f64>,
    #[serde(default)]
    pub memory_used: Option<u64>,
    /// Dedicated VRAM only; unified-memory GPUs do not invent a VRAM capacity.
    #[serde(default)]
    pub memory_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskStats {
    /// The folder asked about.
    pub path: String,
    pub total: u64,
    pub used: u64,
    /// Bytes free for ordinary users (less than total minus used when some
    /// space is reserved for root).
    pub available: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionMemory {
    pub session: String,
    /// Resident memory, in bytes.
    pub rss: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobUsage {
    pub terminal: String,
    /// Share of one core busy since the last `system_stats` (200 is two
    /// cores' worth); none the first time a job is measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f64>,
    /// Resident memory, in bytes.
    pub rss: u64,
}
