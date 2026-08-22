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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<u16>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    #[serde(default)]
    pub compose_working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerListing {
    pub available: bool,
    pub containers: Vec<ContainerInfo>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRequest {
    pub container_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerActionResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerLogSnapshot {
    pub logs: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub theme_mode: String,
    pub scan_interval_ms: u64,
    pub window_width: u32,
    pub window_height: u32,
    pub window_x: Option<i32>,
    pub window_y: Option<i32>,
    #[serde(default)]
    pub window_geometry_logical: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme_mode: "dark".into(),
            scan_interval_ms: 2_000,
            window_width: 1_080,
            window_height: 720,
            window_x: None,
            window_y: None,
            window_geometry_logical: false,
        }
    }
}

impl UiSettings {
    pub fn normalized(mut self) -> Self {
        if !matches!(self.theme_mode.as_str(), "dark" | "light" | "system") {
            self.theme_mode = "dark".into();
        }
        self.scan_interval_ms = self.scan_interval_ms.clamp(500, 60_000);
        self.window_width = self.window_width.max(560);
        self.window_height = self.window_height.max(420);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchTask {
    pub name: String,
    pub cwd: String,
    pub command: String,
    pub expected_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchProfile {
    pub id: String,
    pub name: String,
    pub project_root: String,
    pub tasks: Vec<LaunchTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedTaskSnapshot {
    pub profile_id: String,
    pub task_name: String,
    pub state: String,
    pub main_pid: Option<u32>,
    pub started_at: Option<u64>,
    pub message: Option<String>,
    pub log_tail: String,
    /// PID of a process detected outside Cutting Board for this launch task.
    #[serde(default)]
    pub external_pid: Option<u32>,
    /// Working directory reported for a detected external process.
    #[serde(default)]
    pub external_working_directory: Option<String>,
    /// Readable regular file connected to the external process's stdout/stderr.
    #[serde(default)]
    pub external_log_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo {
    pub version: String,
    pub demo: bool,
    pub settings_path: String,
    pub profiles_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminationResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminateRequest {
    pub service_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRequest {
    pub service_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceLogSnapshot {
    pub logs: String,
    pub source_path: Option<String>,
    pub available: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    pub profile_id: String,
    pub task_name: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_process_payloads_default_missing_launch_command() {
        let process: ProcessInfo = serde_json::from_str(
            r#"{
                "pid": 1,
                "parent_pid": null,
                "name": "node",
                "executable": null,
                "working_directory": null,
                "command": "node server.js",
                "create_time": 0,
                "uptime_seconds": 0,
                "cpu_percent": null,
                "memory_bytes": null,
                "uid": null
            }"#,
        )
        .unwrap();

        assert_eq!(process.launch_command, None);
    }

    #[test]
    fn older_service_payloads_default_missing_active_profiles() {
        let service: ServiceSnapshot = serde_json::from_str(
            r#"{
                "id": "service-1",
                "display_name": "example",
                "tech": "spring",
                "category": "api",
                "relevance": "dev",
                "endpoints": [],
                "process": null,
                "project": null,
                "status": "healthy",
                "warnings": [],
                "origin_kind": "unknown",
                "origin_label": null,
                "can_terminate": false,
                "browser_url": null
            }"#,
        )
        .unwrap();

        assert!(service.active_profiles.is_empty());
    }

    #[test]
    fn older_task_snapshot_payloads_default_external_metadata() {
        let snapshot: ManagedTaskSnapshot = serde_json::from_str(
            r#"{
                "profile_id": "profile",
                "task_name": "web",
                "state": "running",
                "main_pid": 42,
                "started_at": 1,
                "message": null,
                "log_tail": "ready"
            }"#,
        )
        .unwrap();

        assert_eq!(snapshot.external_pid, None);
        assert_eq!(snapshot.external_working_directory, None);
        assert_eq!(snapshot.external_log_path, None);
    }
}
