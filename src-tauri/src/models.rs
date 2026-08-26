mod api;
mod container;
mod launch;
mod settings;
mod workspace;

pub use api::*;
pub use container::*;
pub use launch::*;
pub use settings::*;
pub use workspace::*;

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
