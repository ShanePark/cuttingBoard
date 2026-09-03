use crate::docker;
use crate::models::{
    ContainerActionResult, ContainerInfo, ContainerListing, LaunchProfile, LaunchTask,
    ManagedTaskSnapshot, TaskRequest,
};

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

pub fn start(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, String> {
    act(request, container, demo, docker::start_container)
}

pub fn stop(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, String> {
    act(request, container, demo, docker::stop_container)
}

pub fn restart(
    request: &TaskRequest,
    container: &str,
    demo: bool,
) -> Result<ManagedTaskSnapshot, String> {
    act(request, container, demo, docker::restart_container)
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
    for (task_name, container) in tasks {
        if !listing
            .containers
            .iter()
            .any(|info| info.name == container && state_of(info) != "stopped")
        {
            continue;
        }
        let request = TaskRequest {
            profile_id: profile_id.into(),
            task_name,
        };
        snapshots.push(stop(&request, &container, demo)?);
    }
    Ok(snapshots)
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
    action: fn(&str) -> ContainerActionResult,
) -> Result<ManagedTaskSnapshot, String> {
    let listing = docker::list_containers(demo);
    let info = listing
        .containers
        .iter()
        .find(|info| info.name == container)
        .ok_or_else(|| missing_message(container, &listing))?;
    // Docker is addressed with the identifier from its own listing, so a saved container name
    // never reaches the command line.
    let result = action(&info.id);
    if !result.success {
        return Err(result.message);
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
    let (state, message) = match info {
        Some(info) => (state_of(info), Some(info.status.clone())),
        None => ("stopped", Some(missing_message(container, listing))),
    };
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: state.into(),
        main_pid: None,
        started_at: None,
        message,
        log_tail: String::new(),
        external_pid: None,
        external_working_directory: None,
        external_log_path: None,
    }
}

fn state_of(info: &ContainerInfo) -> &'static str {
    match info.state.as_str() {
        "running" => "running",
        "created" | "restarting" => "starting",
        _ => "stopped",
    }
}

fn missing_message(container: &str, listing: &ContainerListing) -> String {
    if listing.available {
        format!("Container {container} is no longer available.")
    } else {
        listing
            .message
            .clone()
            .unwrap_or_else(|| "Docker is unavailable.".into())
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
