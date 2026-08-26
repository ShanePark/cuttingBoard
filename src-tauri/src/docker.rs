use crate::models::{ContainerActionResult, ContainerListing, ContainerLogSnapshot};

mod cli;
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
}
