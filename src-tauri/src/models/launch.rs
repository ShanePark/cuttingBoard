use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchTask {
    pub name: String,
    pub cwd: String,
    pub command: String,
    pub expected_port: Option<u16>,
    /// Name of the Docker container this task stands for. Container tasks are started and stopped
    /// through Docker instead of a shell command, so a project's containers can sit in the same
    /// launch profile as the services they support.
    #[serde(default)]
    pub container: Option<String>,
}

impl LaunchTask {
    /// The Docker container this task stands for, when it was saved from a container card rather
    /// than from a process command.
    pub fn container_name(&self) -> Option<&str> {
        self.container
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
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
