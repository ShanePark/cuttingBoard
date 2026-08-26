use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub family: String,
    pub address: String,
    pub port: u16,
    pub scope: String,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub name: String,
    pub executable: Option<String>,
    pub working_directory: Option<String>,
    pub command: String,
    #[serde(default)]
    pub launch_command: Option<String>,
    pub create_time: u64,
    pub uptime_seconds: u64,
    pub cpu_percent: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub uid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub detection_source: String,
    #[serde(default)]
    pub workspace_root_path: String,
    #[serde(default)]
    pub workspace_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    pub id: String,
    pub display_name: String,
    pub tech: String,
    pub category: String,
    pub relevance: String,
    pub endpoints: Vec<Endpoint>,
    pub process: Option<ProcessInfo>,
    pub project: Option<ProjectInfo>,
    pub status: String,
    pub warnings: Vec<String>,
    pub origin_kind: String,
    pub origin_label: Option<String>,
    pub can_terminate: bool,
    pub browser_url: Option<String>,
    #[serde(default)]
    pub active_profiles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub services: Vec<ServiceSnapshot>,
    pub scanned_at: u64,
    pub scan_duration_ms: u128,
    pub endpoint_count: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ServiceIdentity {
    pub pid: u32,
    pub start_time: u64,
    pub uid: Option<u32>,
    pub display_name: String,
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
