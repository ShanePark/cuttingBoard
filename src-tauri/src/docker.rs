use std::thread;

use crate::models::{ContainerActionResult, ContainerListing, ContainerLogSnapshot};

pub use compose_log::ComposeDownLogs;

mod cli;
mod compose_log;
mod demo;
mod parser;

const CONTAINER_LIST_FORMAT: &str = concat!(
    "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t",
    "{{.Label \"com.docker.compose.project\"}}\t",
    "{{.Label \"com.docker.compose.service\"}}\t",
    "{{.Label \"com.docker.compose.project.working_dir\"}}"
);

pub fn list_containers(demo_mode: bool) -> ContainerListing {
    if demo_mode {
        return demo::container_listing();
    }

    let Some(executable) = cli::resolve_executable() else {
        return ContainerListing {
            available: false,
            containers: vec![],
            message: Some(cli::not_found_message().into()),
        };
    };
    let output = match cli::run(
        &executable,
        ["ps", "-a", "--no-trunc", "--format", CONTAINER_LIST_FORMAT],
    ) {
        Ok(output) => output,
        Err(error) => {
            return ContainerListing {
                available: false,
                containers: vec![],
                message: Some(format!("Could not start Docker: {error}")),
            };
        }
    };
    if !output.status.success() {
        return ContainerListing {
            available: false,
            containers: vec![],
            message: Some(
                cli::stderr_detail(&output).unwrap_or_else(|| "Docker returned an error.".into()),
            ),
        };
    }

    ContainerListing {
        available: true,
        containers: parser::parse_containers(&String::from_utf8_lossy(&output.stdout)),
        message: None,
    }
}

pub fn container_logs(container_id: &str, demo_mode: bool) -> Result<ContainerLogSnapshot, String> {
    validate_container_id(container_id)?;
    if demo_mode {
        return Ok(ContainerLogSnapshot {
            logs: demo::container_logs(container_id)
                .unwrap_or_default()
                .into(),
        });
    }

    let Some(executable) = cli::resolve_executable() else {
        return Err(cli::not_found_message().into());
    };
    let output = cli::run(
        &executable,
        ["logs", "--tail", "200", "--timestamps", container_id],
    )
    .map_err(|error| format!("Could not run Docker logs for container {container_id}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not read logs for container {container_id}: {}",
            cli::error_detail(&output)
        ));
    }

    Ok(ContainerLogSnapshot {
        logs: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

pub fn start_container(container_id: &str) -> ContainerActionResult {
    run_container_action(container_id, "start", "Started")
}

pub fn stop_container(container_id: &str) -> ContainerActionResult {
    run_container_action(container_id, "stop", "Stopped")
}

pub fn restart_container(container_id: &str) -> ContainerActionResult {
    run_container_action(container_id, "restart", "Restarted")
}

/// Runs `docker compose down` for a project without removing its volumes or images. Its output is
/// collected into `logs` as it arrives so it can be shown while the command runs.
pub fn compose_down(project: &str, logs: &ComposeDownLogs) -> ContainerActionResult {
    if let Err(message) = validate_compose_project(project) {
        return ContainerActionResult {
            success: false,
            message,
        };
    }
    let Some(executable) = cli::resolve_executable() else {
        return ContainerActionResult {
            success: false,
            message: cli::not_found_message().into(),
        };
    };
    let mut child = match cli::spawn_compose_by_label(&executable, compose_down_args(project)) {
        Ok(child) => child,
        Err(error) => {
            return ContainerActionResult {
                success: false,
                message: format!(
                    "Could not run Docker compose down for project {project}: {error}"
                ),
            };
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let status = thread::scope(|scope| {
        if let Some(stdout) = stdout {
            scope.spawn(|| compose_log::read_lines(stdout, |line| logs.append(project, line)));
        }
        if let Some(stderr) = stderr {
            scope.spawn(|| compose_log::read_lines(stderr, |line| logs.append(project, line)));
        }
        child.wait()
    });
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return ContainerActionResult {
                success: false,
                message: format!(
                    "Could not wait for Docker compose down for project {project}: {error}"
                ),
            };
        }
    };
    if !status.success() {
        let log = logs.read(project);
        let detail = log
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                status.code().map_or_else(
                    || "Docker exited without a status code.".into(),
                    |code| format!("Docker exited with status code {code}."),
                )
            });
        return ContainerActionResult {
            success: false,
            message: format!("Could not stop compose project {project}: {detail}"),
        };
    }
    ContainerActionResult {
        success: true,
        message: format!("Stopped compose project {project}."),
    }
}

/// Clears a project's previous `compose_down` output. Called before the run is scheduled so a poll
/// that races the start of the blocking task never shows the previous run's output.
pub fn begin_compose_down(project: &str, logs: &ComposeDownLogs) -> Result<(), String> {
    validate_compose_project(project)?;
    logs.begin(project);
    Ok(())
}

/// Returns the output collected so far by the latest `compose_down` run for a project.
pub fn compose_down_log(project: &str, logs: &ComposeDownLogs) -> Result<String, String> {
    validate_compose_project(project)?;
    Ok(logs.read(project))
}

fn compose_down_args(project: &str) -> [&str; 6] {
    // Plain progress writes one line per event instead of redrawing with ANSI sequences.
    ["compose", "--progress", "plain", "-p", project, "down"]
}

fn run_container_action(
    container_id: &str,
    action: &str,
    success_verb: &str,
) -> ContainerActionResult {
    if let Err(message) = validate_container_id(container_id) {
        return ContainerActionResult {
            success: false,
            message,
        };
    }
    let Some(executable) = cli::resolve_executable() else {
        return ContainerActionResult {
            success: false,
            message: cli::not_found_message().into(),
        };
    };
    let output = match cli::run(&executable, [action, container_id]) {
        Ok(output) => output,
        Err(error) => {
            return ContainerActionResult {
                success: false,
                message: format!(
                    "Could not run Docker {action} for container {container_id}: {error}"
                ),
            };
        }
    };
    if !output.status.success() {
        return ContainerActionResult {
            success: false,
            message: format!(
                "Could not {action} container {container_id}: {}",
                cli::error_detail(&output)
            ),
        };
    }
    ContainerActionResult {
        success: true,
        message: format!("{success_verb} container {container_id}."),
    }
}

fn validate_container_id(container_id: &str) -> Result<(), String> {
    if container_id.trim().is_empty() {
        return Err("Container ID cannot be empty.".into());
    }
    if !(12..=64).contains(&container_id.len())
        || !container_id
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("Container ID must be 12-64 lowercase hexadecimal characters.".into());
    }
    Ok(())
}

fn validate_compose_project(project: &str) -> Result<(), String> {
    if project.trim().is_empty() {
        return Err("Compose project name cannot be empty.".into());
    }
    let mut bytes = project.bytes();
    let valid = matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'));
    if !valid {
        return Err(
            "Compose project name must start with a lowercase letter or digit and contain only lowercase letters, digits, dashes, and underscores.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_container_ids() {
        assert!(validate_container_id("4d2a92d5c814").is_ok());
        assert!(validate_container_id(&"a".repeat(64)).is_ok());
        assert_eq!(
            validate_container_id("   ").unwrap_err(),
            "Container ID cannot be empty."
        );
        assert!(validate_container_id("4d2a92d5c81").is_err());
        assert!(validate_container_id(&"a".repeat(65)).is_err());
        assert!(validate_container_id("4d2a92d5c814\necho").is_err());
        assert!(validate_container_id("4D2A92D5C814").is_err());
    }

    #[test]
    fn validates_compose_project_names() {
        assert!(validate_compose_project("kice-mailpit").is_ok());
        assert!(validate_compose_project("oasis26").is_ok());
        assert!(validate_compose_project("0_dev-stack").is_ok());
        assert_eq!(
            validate_compose_project("  ").unwrap_err(),
            "Compose project name cannot be empty."
        );
        assert!(validate_compose_project("").is_err());
        assert!(validate_compose_project("-v").is_err());
        assert!(validate_compose_project("--project-directory").is_err());
        assert!(validate_compose_project("_dev").is_err());
        assert!(validate_compose_project("Dev").is_err());
        assert!(validate_compose_project("Standalone containers").is_err());
        assert!(validate_compose_project("dev stack").is_err());
        assert!(validate_compose_project("dev\ndown").is_err());
        assert!(validate_compose_project("dev.stack").is_err());
    }

    #[test]
    fn compose_down_keeps_volumes_and_images() {
        assert_eq!(
            compose_down_args("kice-mailpit"),
            [
                "compose",
                "--progress",
                "plain",
                "-p",
                "kice-mailpit",
                "down"
            ]
        );
    }
}
