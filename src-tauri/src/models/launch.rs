use serde::{Deserialize, Serialize};

/// The build tool used to prepare a framework-managed launch task.
///
/// The enum is intentionally small for now. Tasks without a preparation spec continue to use
/// their saved shell command unchanged, so adding this field is backward compatible with older
/// launch profile files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LaunchBuildTool {
    Maven,
    Gradle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPrepareKind {
    SpringBoot,
}

/// Optional project-aware preparation metadata for a launch task.
///
/// `build_tool` and `module` may be omitted when Cutting Board should infer them from the
/// project files. `profiles` and `main_class` are retained as launch metadata for the framework
/// runner; preparation itself only needs the build tool and module.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchPrepareSpec {
    pub kind: LaunchPrepareKind,
    #[serde(default)]
    pub build_tool: Option<LaunchBuildTool>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub main_class: Option<String>,
}

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
    /// Optional build-aware preparation. Missing metadata preserves the legacy shell-command
    /// behavior, while Spring Boot tasks can use Maven/Gradle's incremental build lifecycle.
    #[serde(default)]
    pub prepare: Option<LaunchPrepareSpec>,
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
    /// Readable file that carries the external process's output: its redirected stdout/stderr,
    /// a log file it writes to, or the task log of an earlier Cutting Board session.
    #[serde(default)]
    pub external_log_path: Option<String>,
}
