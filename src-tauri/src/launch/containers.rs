use crate::docker;
use crate::models::{
    ContainerActionResult, ContainerDetail, ContainerInfo, ContainerListing, LaunchProfile,
    LaunchTask, ManagedTaskSnapshot, TaskRequest,
};
use std::fmt;

/// Why a container task action failed. `Display` is the text the UI shows; the control socket reads
/// the fields instead so that it can apply its own masking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerError {
    Missing {
        container: String,
    },
    DockerUnavailable {
        detail: Option<String>,
    },
    ActionFailed {
        container: String,
        action: &'static str,
        message: String,
    },
}

impl fmt::Display for ContainerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { container } => {
                write!(formatter, "Container {container} is no longer available.")
            }
            Self::DockerUnavailable { detail } => {
                formatter.write_str(detail.as_deref().unwrap_or("Docker is unavailable."))
            }
            Self::ActionFailed { message, .. } => formatter.write_str(message),
        }
    }
}

/// The container behind a task request, when that task is a container task.
pub fn container_for(profiles: &[LaunchProfile], request: &TaskRequest) -> Option<String> {
    profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)?
        .tasks
        .iter()
        .find(|task| task.name == request.task_name)
        .and_then(LaunchTask::container_name)
        .map(str::to_owned)
}

/// Snapshots for every container task in these profiles, read from a single Docker listing. The
/// daemon is only asked when a profile actually holds a container task.
pub fn snapshots(profiles: &[LaunchProfile], demo: bool) -> Vec<ManagedTaskSnapshot> {
    let tasks = container_tasks(profiles);
    if tasks.is_empty() {
        return Vec::new();
    }
    let listing = docker::list_containers(demo);
    tasks
        .iter()
        .map(|(profile_id, task_name, container)| {
            snapshot(profile_id, task_name, container, &listing)
        })
        .collect()
}

pub fn task_snapshot(request: &TaskRequest, container: &str, demo: bool) -> ManagedTaskSnapshot {
    snapshot(
        &request.profile_id,
        &request.task_name,
        container,
        &docker::list_containers(demo),
    )
}

pub fn start(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, ContainerError> {
    act(request, container, demo, "start", docker::start_container)
}

pub fn stop(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, ContainerError> {
    act(request, container, demo, "stop", docker::stop_container)
}

pub fn restart(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, ContainerError> {
    act(
        request,
        container,
        demo,
        "restart",
        docker::restart_container,
    )
}

pub fn log_tail(container: &str, demo: bool) -> Result<String, ContainerError> {
    let listing = docker::list_containers(demo);
    let info = listing
        .containers
        .iter()
        .find(|info| info.name == container)
        .ok_or_else(|| missing_error(container, &listing))?;
    docker::container_logs(&info.id, demo)
        .map(|snapshot| snapshot.logs)
        .map_err(|message| ContainerError::ActionFailed {
            container: container.into(),
            action: "read logs for",
            message,
        })
}

/// Whether the task's container is up. A container Docker no longer lists counts as inactive.
pub fn is_active(container: &str, demo: bool) -> Result<bool, ContainerError> {
    let listing = docker::list_containers(demo);
    if !listing.available {
        return Err(missing_error(container, &listing));
    }
    Ok(listing
        .containers
        .iter()
        .any(|info| info.name == container && state_of(info) != "stopped"))
}

/// Stops the running containers of a profile so stopping the whole profile reaches them too.
pub fn stop_profile(
    profiles: &[LaunchProfile],
    profile_id: &str,
    demo: bool,
) -> Result<Vec<ManagedTaskSnapshot>, String> {
    let tasks = container_tasks(profiles)
        .into_iter()
        .filter(|(owner, _, _)| *owner == profile_id)
        .map(|(_, task_name, container)| (task_name.to_owned(), container.to_owned()))
        .collect::<Vec<_>>();
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let listing = docker::list_containers(demo);
    let mut snapshots = Vec::new();
    for (task_name, container) in tasks_to_stop(tasks, &listing)? {
        let request = TaskRequest {
            profile_id: profile_id.into(),
            task_name,
        };
        snapshots.push(stop(&request, &container, demo).map_err(|error| error.to_string())?);
    }
    Ok(snapshots)
}

/// The tasks whose container is up. Without a listing nobody can tell, which must not read as
/// there being nothing to stop.
fn tasks_to_stop(
    tasks: Vec<(String, String)>,
    listing: &ContainerListing,
) -> Result<Vec<(String, String)>, String> {
    if !listing.available {
        let reason = listing
            .message
            .as_deref()
            .unwrap_or("Docker is unavailable.");
        return Err(format!(
            "The containers in this profile may still be running: {reason}"
        ));
    }
    Ok(tasks
        .into_iter()
        .filter(|(_, container)| {
            listing
                .containers
                .iter()
                .any(|info| info.name == *container && state_of(info) != "stopped")
        })
        .collect())
}

fn container_tasks(profiles: &[LaunchProfile]) -> Vec<(&str, &str, &str)> {
    profiles
        .iter()
        .flat_map(|profile| {
            profile.tasks.iter().filter_map(move |task| {
                task.container_name()
                    .map(|container| (profile.id.as_str(), task.name.as_str(), container))
            })
        })
        .collect()
}

fn act(
    request: &TaskRequest,
    container: &str,
    demo: bool,
    action_name: &'static str,
    action: fn(&str) -> ContainerActionResult,
) -> Result<ManagedTaskSnapshot, ContainerError> {
    let listing = docker::list_containers(demo);
    let info = listing
        .containers
        .iter()
        .find(|info| info.name == container)
        .ok_or_else(|| missing_error(container, &listing))?;
    // Docker is addressed with the identifier from its own listing, so a saved container name
    // never reaches the command line.
    let result = action(&info.id);
    if !result.success {
        return Err(ContainerError::ActionFailed {
            container: container.into(),
            action: action_name,
            message: result.message,
        });
    }
    let listing = docker::list_containers(demo);
    let mut snapshot = snapshot(&request.profile_id, &request.task_name, container, &listing);
    snapshot.message = Some(result.message);
    Ok(snapshot)
}

fn snapshot(
    profile_id: &str,
    task_name: &str,
    container: &str,
    listing: &ContainerListing,
) -> ManagedTaskSnapshot {
    let info = listing
        .containers
        .iter()
        .find(|info| info.name == container);
    let (state, message, detail) = match info {
        Some(info) => (
            state_of(info),
            info.status.clone(),
            ContainerDetail::Listed {
                status: info.status.clone(),
                ports: info.ports.clone(),
            },
        ),
        None => {
            let error = missing_error(container, listing);
            let detail = match &error {
                ContainerError::DockerUnavailable { detail } => {
                    ContainerDetail::DockerUnavailable {
                        detail: detail.clone(),
                    }
                }
                _ => ContainerDetail::Missing {
                    container: container.into(),
                },
            };
            ("stopped", error.to_string(), detail)
        }
    };
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: state.into(),
        main_pid: None,
        started_at: None,
        exit_code: None,
        message: Some(message),
        log_tail: String::new(),
        external_pid: None,
        external_working_directory: None,
        external_log_path: None,
        container_detail: Some(detail),
    }
}

fn state_of(info: &ContainerInfo) -> &'static str {
    match info.state.as_str() {
        "running" => "running",
        "created" | "restarting" => "starting",
        _ => "stopped",
    }
}

fn missing_error(container: &str, listing: &ContainerListing) -> ContainerError {
    if listing.available {
        ContainerError::Missing {
            container: container.into(),
        }
    } else {
        ContainerError::DockerUnavailable {
            detail: listing.message.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str, container: Option<&str>) -> LaunchTask {
        LaunchTask {
            name: name.into(),
            cwd: ".".into(),
            command: "npm run dev".into(),
            expected_port: None,
            container: container.map(str::to_owned),
            prepare: None,
        }
    }

    fn info(name: &str, state: &str) -> ContainerInfo {
        ContainerInfo {
            id: "a".repeat(64),
            name: name.into(),
            image: "postgres:16".into(),
            state: state.into(),
            status: format!("Up 2 hours ({state})"),
            ports: vec![5432],
            port_mappings: vec![],
            compose_project: None,
            compose_service: None,
            compose_working_dir: None,
        }
    }

    fn listing(containers: Vec<ContainerInfo>) -> ContainerListing {
        ContainerListing {
            available: true,
            containers,
            message: None,
        }
    }

    #[test]
    fn reads_the_container_only_from_container_tasks() {
        assert_eq!(task("db", Some("app-db")).container_name(), Some("app-db"));
        assert_eq!(task("api", None).container_name(), None);
        assert_eq!(task("db", Some("   ")).container_name(), None);
    }

    #[test]
    fn maps_docker_states_onto_launch_states() {
        assert_eq!(state_of(&info("app-db", "running")), "running");
        assert_eq!(state_of(&info("app-db", "restarting")), "starting");
        assert_eq!(state_of(&info("app-db", "exited")), "stopped");
        assert_eq!(state_of(&info("app-db", "paused")), "stopped");
    }

    #[test]
    fn snapshots_a_container_task_from_the_listing() {
        let running = snapshot(
            "profile",
            "app-db",
            "app-db",
            &listing(vec![info("app-db", "running")]),
        );
        assert_eq!(running.state, "running");
        assert_eq!(running.main_pid, None);

        let removed = snapshot("profile", "app-db", "app-db", &listing(vec![]));
        assert_eq!(removed.state, "stopped");
        assert_eq!(
            removed.message.as_deref(),
            Some("Container app-db is no longer available.")
        );
        assert_eq!(
            removed.container_detail,
            Some(ContainerDetail::Missing {
                container: "app-db".into()
            })
        );

        let unavailable = snapshot(
            "profile",
            "app-db",
            "app-db",
            &ContainerListing {
                available: false,
                containers: vec![],
                message: Some("Cannot connect to the Docker daemon".into()),
            },
        );
        assert_eq!(unavailable.state, "stopped");
        assert_eq!(
            unavailable.message.as_deref(),
            Some("Cannot connect to the Docker daemon")
        );
        assert_eq!(
            unavailable.container_detail,
            Some(ContainerDetail::DockerUnavailable {
                detail: Some("Cannot connect to the Docker daemon".into())
            })
        );
    }

    #[test]
    fn container_errors_keep_the_messages_the_ui_shows() {
        assert_eq!(
            missing_error("maven-db", &listing(vec![])).to_string(),
            "Container maven-db is no longer available."
        );
        assert_eq!(
            ContainerError::DockerUnavailable { detail: None }.to_string(),
            "Docker is unavailable."
        );
        assert_eq!(
            ContainerError::ActionFailed {
                container: "app-db".into(),
                action: "restart",
                message: "Could not restart container abc: boom".into(),
            }
            .to_string(),
            "Could not restart container abc: boom"
        );
    }

    #[test]
    fn stopping_a_profile_picks_only_containers_that_are_up() {
        let tasks = vec![
            ("db".to_string(), "app-db".to_string()),
            ("cache".to_string(), "app-cache".to_string()),
            ("gone".to_string(), "app-gone".to_string()),
        ];
        let listed = listing(vec![info("app-db", "running"), info("app-cache", "exited")]);
        assert_eq!(
            tasks_to_stop(tasks, &listed).unwrap(),
            vec![("db".to_string(), "app-db".to_string())]
        );
    }

    #[test]
    fn stopping_a_profile_reports_a_listing_docker_could_not_give() {
        let tasks = vec![("db".to_string(), "app-db".to_string())];
        let hung = ContainerListing {
            available: false,
            containers: vec![],
            message: Some("Docker did not answer within 10 seconds.".into()),
        };
        let error = tasks_to_stop(tasks.clone(), &hung).unwrap_err();
        assert!(error.contains("may still be running"));
        assert!(error.contains("did not answer"));

        let silent = ContainerListing {
            message: None,
            ..hung
        };
        assert!(tasks_to_stop(tasks, &silent)
            .unwrap_err()
            .contains("Docker is unavailable."));
    }

    #[test]
    fn finds_the_container_of_a_requested_task() {
        let profiles = vec![LaunchProfile {
            id: "profile".into(),
            name: "App".into(),
            project_root: "/tmp/app".into(),
            tasks: vec![task("api", None), task("app-db", Some("app-db"))],
        }];
        let request = |task_name: &str| TaskRequest {
            profile_id: "profile".into(),
            task_name: task_name.into(),
        };
        assert_eq!(
            container_for(&profiles, &request("app-db")),
            Some("app-db".into())
        );
        assert_eq!(container_for(&profiles, &request("api")), None);
    }
}
