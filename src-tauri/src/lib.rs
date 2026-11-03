mod cli;
mod control;
mod control_service;
mod docker;
mod launch;
mod models;
mod process_control;
mod scanner;
mod storage;
mod system_metrics;
mod update;
mod window;

use crate::{
    launch::{containers as launch_containers, LaunchManager},
    models::{
        AppInfo, ComposeProjectRequest, ContainerActionResult, ContainerListing,
        ContainerLogSnapshot, ContainerRequest, LaunchProfile, ManagedTaskSnapshot,
        RestartServiceRequest, ServiceIdentity, ServiceLogSnapshot, ServiceRequest, SystemMetrics,
        TaskRequest, TerminateRequest, TerminationResult, UiSettings, WorkspaceSnapshot,
    },
    storage::{
        delete_profile as remove_profile, demo_profiles, load_profiles as read_profiles,
        load_settings as read_settings, save_profile as persist_profile,
        save_settings as persist_settings,
    },
    system_metrics::SystemMetricsState,
};
use std::{
    collections::HashMap,
    net::{Ipv4Addr, Ipv6Addr, TcpListener},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    thread,
    time::Duration,
};
use tauri::{Manager, State};

#[derive(Clone)]
struct AppState(Arc<AppStateInner>);

struct AppStateInner {
    demo: bool,
    settings_path: PathBuf,
    profiles_path: PathBuf,
    logs_dir: PathBuf,
    settings_io: Arc<Mutex<()>>,
    window_geometry: window::WindowGeometryPersistence,
    scan: Mutex<ScanState>,
    launch: Mutex<LaunchManager>,
    control: control_service::ControlState,
    system_metrics: Mutex<SystemMetricsState>,
    compose_down_logs: docker::ComposeDownLogs,
}

impl AppState {
    fn control_service_state(&self) -> &control_service::ControlState {
        &self.0.control
    }
}

#[derive(Debug, Default)]
struct ScanState {
    workspace: Option<WorkspaceSnapshot>,
    service_index: HashMap<String, ServiceIdentity>,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        demo: state.0.demo,
        settings_path: state.0.settings_path.to_string_lossy().into_owned(),
        profiles_path: state.0.profiles_path.to_string_lossy().into_owned(),
        update_supported: update::is_supported(),
    }
}

#[tauri::command]
async fn scan_workspace(state: State<'_, AppState>) -> Result<WorkspaceSnapshot, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || refresh_control_scan(&state))
        .await
        .map_err(|error| format!("Service scan task failed: {error}"))?
}

#[tauri::command]
async fn list_containers(state: State<'_, AppState>) -> Result<ContainerListing, String> {
    let demo = state.0.demo;
    tauri::async_runtime::spawn_blocking(move || Ok(docker::list_containers(demo)))
        .await
        .map_err(|error| format!("Docker task failed: {error}"))?
}

#[tauri::command]
async fn system_metrics(state: State<'_, AppState>) -> Result<SystemMetrics, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || Ok(lock(&state.0.system_metrics)?.refresh()))
        .await
        .map_err(|error| format!("System metrics task failed: {error}"))?
}

#[tauri::command]
async fn container_logs(
    state: State<'_, AppState>,
    request: ContainerRequest,
) -> Result<ContainerLogSnapshot, String> {
    let demo = state.0.demo;
    let container_id = request.container_id;
    tauri::async_runtime::spawn_blocking(move || docker::container_logs(&container_id, demo))
        .await
        .map_err(|error| format!("Docker logs task failed: {error}"))?
}

#[tauri::command]
async fn service_logs(
    state: State<'_, AppState>,
    request: ServiceRequest,
) -> Result<ServiceLogSnapshot, String> {
    let state = state.inner().clone();
    let service_id = request.service_id;
    tauri::async_runtime::spawn_blocking(move || read_service_logs(&state, &service_id))
        .await
        .map_err(|error| format!("Service log task failed: {error}"))?
}

fn read_service_logs(state: &AppState, service_id: &str) -> Result<ServiceLogSnapshot, String> {
    if state.0.demo {
        return Ok(ServiceLogSnapshot {
            logs: String::new(),
            source_path: None,
            available: false,
            message: Some("Service logs are unavailable in demonstration mode.".into()),
        });
    }

    let workspace = lock(&state.0.scan)?
        .workspace
        .clone()
        .ok_or_else(|| "Scan the workspace before requesting service logs.".to_string())?;
    let service = workspace
        .services
        .iter()
        .find(|service| service.id == service_id)
        .cloned()
        .ok_or_else(|| {
            "The service changed since the last scan. Refresh and try again.".to_string()
        })?;
    if service.process.is_none() {
        return Ok(ServiceLogSnapshot {
            logs: String::new(),
            source_path: None,
            available: false,
            message: Some("Process details were unavailable during the last scan.".into()),
        });
    }
    let identity = process_control::service_identity(&service)?;
    process_control::validate_service_log_identity(&identity)?;
    let profiles = read_profiles(&state.0.profiles_path)?;
    lock(&state.0.launch)?.service_logs(&profiles, &service, &state.0.logs_dir)
}

#[tauri::command]
async fn start_container(
    state: State<'_, AppState>,
    request: ContainerRequest,
) -> Result<ContainerActionResult, String> {
    reject_demo(state.inner())?;
    let container_id = request.container_id;
    tauri::async_runtime::spawn_blocking(move || Ok(docker::start_container(&container_id)))
        .await
        .map_err(|error| format!("Docker start task failed: {error}"))?
}

#[tauri::command]
async fn stop_container(
    state: State<'_, AppState>,
    request: ContainerRequest,
) -> Result<ContainerActionResult, String> {
    reject_demo(state.inner())?;
    let container_id = request.container_id;
    tauri::async_runtime::spawn_blocking(move || Ok(docker::stop_container(&container_id)))
        .await
        .map_err(|error| format!("Docker stop task failed: {error}"))?
}

#[tauri::command]
async fn compose_down(
    state: State<'_, AppState>,
    request: ComposeProjectRequest,
) -> Result<ContainerActionResult, String> {
    reject_demo(state.inner())?;
    let project = request.project;
    docker::begin_compose_down(&project, &state.0.compose_down_logs)?;
    let state = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || {
        Ok(docker::compose_down(&project, &state.compose_down_logs))
    })
    .await
    .map_err(|error| format!("Docker compose down task failed: {error}"))?
}

#[tauri::command]
fn compose_down_log(
    state: State<'_, AppState>,
    request: ComposeProjectRequest,
) -> Result<String, String> {
    reject_demo(state.inner())?;
    docker::compose_down_log(&request.project, &state.0.compose_down_logs)
}

#[tauri::command]
fn load_settings(state: State<'_, AppState>) -> Result<UiSettings, String> {
    read_settings(&state.0.settings_path)
}

#[tauri::command]
fn save_settings(
    state: State<'_, AppState>,
    mut settings: UiSettings,
) -> Result<UiSettings, String> {
    let _settings_io = lock(&state.0.settings_io)?;
    if let Ok(current) = read_settings(&state.0.settings_path) {
        preserve_saved_window_geometry(&mut settings, &current);
    }
    persist_settings(&state.0.settings_path, settings)
}

fn preserve_saved_window_geometry(settings: &mut UiSettings, current: &UiSettings) {
    settings.window_width = current.window_width;
    settings.window_height = current.window_height;
    settings.window_x = current.window_x;
    settings.window_y = current.window_y;
    settings.window_geometry_logical = current.window_geometry_logical;
}

pub(crate) const SAVE_PROFILE_STORAGE_ERROR: &str = "configuration_error";

#[tauri::command]
fn load_profiles(state: State<'_, AppState>) -> Result<Vec<LaunchProfile>, String> {
    profiles_for_state(state.inner())
}

/// `baseline` is the profile as an edit form opened with it; a new profile has none.
#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    profile: LaunchProfile,
    baseline: Option<LaunchProfile>,
) -> Result<Vec<LaunchProfile>, String> {
    save_profile_from_form(state.inner(), profile, baseline)
}

fn save_profile_from_form(
    state: &AppState,
    profile: LaunchProfile,
    baseline: Option<LaunchProfile>,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(state)?;
    save_profile_guarded(state, |current_profiles| {
        // The form saves the whole profile, so a change made after it opened, by the control CLI
        // for instance, would silently be reverted.
        if let Some(baseline) = &baseline {
            let latest = current_profiles.iter().find(|item| item.id == profile.id);
            if latest != Some(baseline) {
                return Err(control::ControlError {
                    code: "profile_changed".into(),
                    message: "This profile changed after this form was opened, and saving would overwrite that change. Close the form and open it again to edit the latest values.".into(),
                });
            }
        }
        ensure_profile_inactive(
            state,
            current_profiles,
            &profile.id,
            "Stop every task in this profile before editing it.",
        )
        .map_err(|message| control::ControlError {
            code: "profile_active".into(),
            message,
        })?;
        Ok(profile)
    })
    .map_err(|error| error.message)
}

/// The single path that writes a profile: `build` decides what to save from the profiles on disk,
/// under the lease that keeps task and profile operations out until the file is written. It is
/// responsible for refusing the edit of anything still running.
pub(crate) fn save_profile_guarded(
    state: &AppState,
    build: impl FnOnce(&[LaunchProfile]) -> Result<LaunchProfile, control::ControlError>,
) -> Result<Vec<LaunchProfile>, control::ControlError> {
    let storage_error = |message| control::ControlError {
        code: SAVE_PROFILE_STORAGE_ERROR.into(),
        message,
    };
    let _profile_operation = state.0.control.begin_profile_edit()?;
    let current_profiles = read_profiles(&state.0.profiles_path).map_err(storage_error)?;
    let profile = build(&current_profiles)?;
    persist_profile(&state.0.profiles_path, profile).map_err(storage_error)
}

#[tauri::command]
async fn delete_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _profile_operation = state
            .0
            .control
            .begin_profile_edit()
            .map_err(|error| error.message)?;
        // Deleting is allowed while the profile runs. The tasks Cutting Board started are stopped
        // first so the delete never leaves a process running that no view can reach any more.
        lock(&state.0.launch)?.discard_profile(&profile_id);
        remove_profile(&state.0.profiles_path, &profile_id)
    })
    .await
    .map_err(|error| format!("Delete profile failed: {error}"))?
}

#[tauri::command]
async fn task_snapshots(state: State<'_, AppState>) -> Result<Vec<ManagedTaskSnapshot>, String> {
    let state = state.inner().clone();
    // Reading container states asks Docker, so the poll runs off the UI thread.
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = profiles_for_state(&state)?;
        let workspace = current_workspace(&state)?;
        let mut snapshots =
            lock(&state.0.launch)?.snapshots(&profiles, workspace.as_ref(), &state.0.logs_dir);
        snapshots.extend(launch_containers::snapshots(&profiles, state.0.demo));
        Ok(snapshots)
    })
    .await
    .map_err(|error| format!("Task snapshot failed: {error}"))?
}

#[tauri::command]
async fn task_log_tail(state: State<'_, AppState>, request: TaskRequest) -> Result<String, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = profiles_for_state(&state)?;
        LaunchManager::task_log_tail(&profiles, &request, &state.0.logs_dir)
    })
    .await
    .map_err(|error| format!("Task log read failed: {error}"))?
}

fn run_ui_task_action(
    state: &AppState,
    request: &TaskRequest,
    action: control_service::TaskAction,
) -> Result<ManagedTaskSnapshot, String> {
    let lease = state
        .0
        .control
        .begin_task_action(request, action)
        .map_err(|error| error.message)?;
    let result = execute_control_task_action(state, request, action);
    lease.finish(&result);
    result.map_err(|error| error.to_string())
}

/// The single native task-action path used by both Tauri and external control requests.
pub(crate) fn execute_control_task_action(
    state: &AppState,
    request: &TaskRequest,
    action: control_service::TaskAction,
) -> Result<ManagedTaskSnapshot, control_service::TaskActionError> {
    reject_demo(state)?;
    let profiles = profiles_for_state(state)?;
    let profile = profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .ok_or_else(|| "The launch profile no longer exists.".to_string())?;
    let task = profile
        .tasks
        .iter()
        .find(|task| task.name == request.task_name)
        .ok_or_else(|| "The task no longer exists in this profile.".to_string())?;

    if let Some(container) = launch_containers::container_for(&profiles, request) {
        return match action {
            control_service::TaskAction::Start => {
                launch_containers::start(request, &container, state.0.demo)
            }
            control_service::TaskAction::Stop => {
                launch_containers::stop(request, &container, state.0.demo)
            }
            control_service::TaskAction::Restart => {
                launch_containers::restart(request, &container, state.0.demo)
            }
        }
        .map_err(Into::into);
    }

    let workspace = refresh_control_scan(state)?;

    let match_count =
        LaunchManager::external_task_match_count(&profiles, request, Some(&workspace))?;
    let mut manager = lock(&state.0.launch)?;
    let snapshots = manager.snapshots(&profiles, Some(&workspace), &state.0.logs_dir);
    let current = snapshots
        .iter()
        .find(|snapshot| {
            snapshot.profile_id == request.profile_id && snapshot.task_name == request.task_name
        })
        .cloned();
    let managed_active = current.as_ref().is_some_and(|snapshot| {
        matches!(snapshot.state.as_str(), "starting" | "running" | "stopping")
            && snapshot.external_pid.is_none()
    });
    if current
        .as_ref()
        .is_some_and(|snapshot| snapshot.external_pid.is_some())
        && match_count > 1
    {
        return Err(
            "The external process matches more than one registered task and cannot be controlled safely."
                .into(),
        );
    }

    if action == control_service::TaskAction::Start && !managed_active {
        if let Some(port) = task.expected_port {
            let matched_external = current
                .as_ref()
                .is_some_and(|snapshot| snapshot.external_pid.is_some() && match_count == 1);
            if !matched_external && port_is_occupied(port) {
                return Err(control_service::TaskActionError::PortInUse(port));
            }
        }
    }

    match action {
        control_service::TaskAction::Start => {
            manager.start_task(&profiles, request, &state.0.logs_dir, Some(&workspace))
        }
        control_service::TaskAction::Stop => {
            manager.stop_task(&profiles, request, &state.0.logs_dir, Some(&workspace))
        }
        control_service::TaskAction::Restart => {
            manager.restart_task(&profiles, request, &state.0.logs_dir, Some(&workspace))
        }
    }
    .map_err(Into::into)
}

fn port_is_occupied(port: u16) -> bool {
    match TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)) {
        Ok(listener) => drop(listener),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::Unsupported
                    | std::io::ErrorKind::InvalidInput
            ) => {}
        Err(_) => return true,
    }

    match TcpListener::bind((Ipv6Addr::UNSPECIFIED, port)) {
        Ok(listener) => drop(listener),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::Unsupported
                    | std::io::ErrorKind::InvalidInput
            ) => {}
        Err(_) => return true,
    }
    false
}

pub(crate) fn control_profiles(state: &AppState) -> Result<Vec<LaunchProfile>, String> {
    profiles_for_state(state)
}

pub(crate) fn control_is_demo(state: &AppState) -> bool {
    state.0.demo
}

pub(crate) fn control_task_log(
    state: &AppState,
    profiles: &[LaunchProfile],
    request: &TaskRequest,
    workspace: Option<&WorkspaceSnapshot>,
) -> Result<launch::TaskLog, control_service::TaskActionError> {
    if let Some(container) = launch_containers::container_for(profiles, request) {
        let text = launch_containers::log_tail(&container, state.0.demo)?;
        return Ok(launch::TaskLog {
            truncated: text.lines().count() >= 200,
            text,
            ..Default::default()
        });
    }
    let logs_dir = &state.0.logs_dir;
    Ok(match state.0.launch.try_lock() {
        Ok(mut launch) => launch.task_log(profiles, request, workspace, logs_dir),
        Err(TryLockError::WouldBlock) => {
            LaunchManager::task_log_while_busy(profiles, request, workspace, logs_dir)
        }
        Err(TryLockError::Poisoned(_)) => Err("Internal state lock was poisoned.".into()),
    }?)
}

pub(crate) fn control_task_prepare_run(
    state: &AppState,
    profile: &LaunchProfile,
    task: &models::LaunchTask,
) -> std::io::Result<launch::PrepareRun> {
    launch::task_prepare_run(&state.0.logs_dir, profile, task)
}

pub(crate) fn control_task_is_active(
    state: &AppState,
    profile: &LaunchProfile,
    task: &models::LaunchTask,
    workspace: &WorkspaceSnapshot,
) -> Result<bool, control_service::TaskActionError> {
    if let Some(container) = task.container_name() {
        return Ok(launch_containers::is_active(container, state.0.demo)?);
    }
    Ok(lock(&state.0.launch)?.task_is_active(profile, task, Some(workspace)))
}

pub(crate) fn control_task_snapshots(
    state: &AppState,
    profiles: &[LaunchProfile],
) -> Result<Option<Vec<ManagedTaskSnapshot>>, String> {
    let Some(mut snapshots) = control_manager_snapshots(state, profiles)? else {
        return Ok(None);
    };
    snapshots.extend(launch_containers::snapshots(profiles, state.0.demo));
    Ok(Some(snapshots))
}

/// Snapshots of every task except container tasks, so Docker is not asked.
fn control_manager_snapshots(
    state: &AppState,
    profiles: &[LaunchProfile],
) -> Result<Option<Vec<ManagedTaskSnapshot>>, String> {
    let mut launch = match state.0.launch.try_lock() {
        Ok(launch) => launch,
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Poisoned(_)) => return Err("Internal state lock was poisoned.".into()),
    };
    let workspace = current_workspace(state)?;
    Ok(Some(launch.snapshots(
        profiles,
        workspace.as_ref(),
        &state.0.logs_dir,
    )))
}

/// One task's snapshot, judged against the last scan. Docker is only asked about a container task,
/// and `None` means an operation holds the manager.
pub(crate) fn control_task_snapshot(
    state: &AppState,
    profiles: &[LaunchProfile],
    request: &TaskRequest,
) -> Result<Option<ManagedTaskSnapshot>, String> {
    if let Some(container) = launch_containers::container_for(profiles, request) {
        return Ok(Some(launch_containers::task_snapshot(
            request,
            &container,
            state.0.demo,
        )));
    }
    let mut launch = match state.0.launch.try_lock() {
        Ok(launch) => launch,
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Poisoned(_)) => return Err("Internal state lock was poisoned.".into()),
    };
    let workspace = current_workspace(state)?;
    launch
        .task_snapshot(profiles, request, workspace.as_ref(), &state.0.logs_dir)
        .map(Some)
}

pub(crate) fn refresh_control_scan(state: &AppState) -> Result<WorkspaceSnapshot, String> {
    let (snapshot, index) = scanner::scan_workspace(state.0.demo)?;
    let mut scan = lock(&state.0.scan)?;
    scan.workspace = Some(snapshot.clone());
    scan.service_index = index;
    Ok(snapshot)
}

pub(crate) fn control_cached_scan(state: &AppState) -> Result<Option<WorkspaceSnapshot>, String> {
    Ok(lock(&state.0.scan)?.workspace.clone())
}

#[tauri::command]
async fn start_task(
    state: State<'_, AppState>,
    request: TaskRequest,
) -> Result<ManagedTaskSnapshot, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_ui_task_action(&state, &request, control_service::TaskAction::Start)
    })
    .await
    .map_err(|error| format!("Launch task failed: {error}"))?
}

#[tauri::command]
async fn stop_task(
    state: State<'_, AppState>,
    request: TaskRequest,
) -> Result<ManagedTaskSnapshot, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_ui_task_action(&state, &request, control_service::TaskAction::Stop)
    })
    .await
    .map_err(|error| format!("Stop task failed: {error}"))?
}

#[tauri::command]
async fn restart_task(
    state: State<'_, AppState>,
    request: TaskRequest,
) -> Result<ManagedTaskSnapshot, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        run_ui_task_action(&state, &request, control_service::TaskAction::Restart)
    })
    .await
    .map_err(|error| format!("Restart task failed: {error}"))?
}

#[tauri::command]
async fn stop_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<ManagedTaskSnapshot>, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _profile_operation = state
            .0
            .control
            .begin_profile_action(&profile_id)
            .map_err(|error| error.message)?;
        let (profiles, workspace) = launch_context(&state)?;
        // Trouble with Docker is reported, but must not keep the other tasks running.
        let containers = launch_containers::stop_profile(&profiles, &profile_id, state.0.demo);
        let managed = lock(&state.0.launch)?.stop_profile(
            &profiles,
            &profile_id,
            &state.0.logs_dir,
            workspace.as_ref(),
        );
        let mut snapshots = containers?;
        snapshots.extend(managed?);
        Ok(snapshots)
    })
    .await
    .map_err(|error| format!("Stop profile failed: {error}"))?
}

#[tauri::command]
async fn terminate_service(
    state: State<'_, AppState>,
    request: TerminateRequest,
) -> Result<TerminationResult, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || terminate_discovered_service(&state, &request))
        .await
        .map_err(|error| format!("Termination task failed: {error}"))?
}

#[tauri::command]
async fn restart_service(
    state: State<'_, AppState>,
    request: RestartServiceRequest,
) -> Result<(), String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || restart_discovered_service(&state, &request))
        .await
        .map_err(|error| format!("Restart task failed: {error}"))?
}

#[tauri::command]
fn shutdown(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.0.launch)?.stop_all();
    Ok(())
}

fn terminate_discovered_service(
    state: &AppState,
    request: &TerminateRequest,
) -> Result<TerminationResult, String> {
    let identity = lock(&state.0.scan)?
        .service_index
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| {
            "The service changed since the last scan. Refresh and try again.".to_string()
        })?;
    process_control::terminate_discovered_service(identity)
}

fn restart_discovered_service(
    state: &AppState,
    request: &RestartServiceRequest,
) -> Result<(), String> {
    let scan = lock(&state.0.scan)?;
    let identity = scan
        .service_index
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| {
            "The service cannot be restarted safely. Refresh and try again.".to_string()
        })?;
    let service = scan
        .workspace
        .as_ref()
        .and_then(|workspace| {
            workspace
                .services
                .iter()
                .find(|service| service.id == request.service_id)
        })
        .cloned()
        .ok_or_else(|| {
            "The service changed since the last scan. Refresh and try again.".to_string()
        })?;
    drop(scan);
    process_control::restart_discovered_service(&service, identity)
}

fn profiles_for_state(state: &AppState) -> Result<Vec<LaunchProfile>, String> {
    if state.0.demo {
        Ok(demo_profiles())
    } else {
        read_profiles(&state.0.profiles_path)
    }
}

fn current_workspace(state: &AppState) -> Result<Option<WorkspaceSnapshot>, String> {
    Ok(lock(&state.0.scan)?.workspace.clone())
}

fn launch_context(
    state: &AppState,
) -> Result<(Vec<LaunchProfile>, Option<WorkspaceSnapshot>), String> {
    Ok((
        read_profiles(&state.0.profiles_path)?,
        current_workspace(state)?,
    ))
}

fn ensure_profile_inactive(
    state: &AppState,
    profiles: &[LaunchProfile],
    profile_id: &str,
    message: &str,
) -> Result<(), String> {
    let workspace = current_workspace(state)?;
    if lock(&state.0.launch)?.profile_is_active(profiles, profile_id, workspace.as_ref()) {
        return Err(message.into());
    }
    Ok(())
}

fn reject_demo(state: &AppState) -> Result<(), String> {
    if state.0.demo {
        Err("Actions are disabled in demonstration mode.".into())
    } else {
        Ok(())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    mutex
        .lock()
        .map_err(|_| "Internal state lock was poisoned.".into())
}

pub fn run() {
    if update::run_helper_if_requested() {
        return;
    }

    let options = match cli::parse_cli() {
        Ok(options) => options,
        Err(error) => {
            if cli::control_was_requested() {
                std::process::exit(control::print_invalid_cli_error(&error));
            }
            eprintln!("{error}\n\nRun with --help for usage.");
            std::process::exit(2);
        }
    };
    if let Some(command) = options.control.clone() {
        let exit_code = control::run_cli(command);
        if exit_code != 0 {
            std::process::exit(exit_code);
        }
        return;
    }
    if options.show_help {
        cli::print_help();
        return;
    }
    if options.show_version {
        println!("Cutting Board {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let demo = options.demo;
    let auto_close_seconds = options.auto_close_seconds;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let config_dir = app.path().app_config_dir().map_err(|error| {
                format!("Could not resolve the app configuration directory: {error}")
            })?;
            let settings_path = config_dir.join("settings.json");
            let profiles_path = config_dir.join("launch-profiles.json");
            let logs_dir = config_dir.join("logs");
            let mut settings = read_settings(&settings_path).unwrap_or_default();
            let window_icon = app.default_window_icon().cloned();
            if let Some(window) = app.get_webview_window("main") {
                let main_window = window.as_ref().window();
                settings =
                    window::migrate_startup_window_settings(&main_window, &settings_path, settings);
                if let Some(icon) = window_icon {
                    window.set_icon(icon).map_err(|error| {
                        format!("Could not set the application window icon: {error}")
                    })?;
                }
                let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize::new(
                    settings.window_width as f64,
                    settings.window_height as f64,
                )));
                if let (Some(x), Some(y)) = (settings.window_x, settings.window_y) {
                    let _ = window.set_position(tauri::Position::Physical(
                        tauri::PhysicalPosition::new(x, y),
                    ));
                }
            }
            let settings_io = Arc::new(Mutex::new(()));
            let control = control_service::ControlState::default();
            let window_geometry = window::WindowGeometryPersistence::new(
                settings_path.clone(),
                Arc::clone(&settings_io),
            );
            let state = AppState(Arc::new(AppStateInner {
                demo,
                settings_path,
                profiles_path,
                logs_dir,
                settings_io,
                window_geometry,
                scan: Mutex::new(ScanState::default()),
                launch: Mutex::new(LaunchManager::reporting_to(control.clone())),
                control,
                system_metrics: Mutex::new(SystemMetricsState::default()),
                compose_down_logs: docker::ComposeDownLogs::default(),
            }));
            app.manage(state.clone());
            #[cfg(unix)]
            {
                let control_state = state.clone();
                match control::start_server(move |request| {
                    control_service::dispatch(&control_state, request)
                }) {
                    Ok(server) => {
                        app.manage(server);
                    }
                    Err(error) if error.contains("already active") => {
                        return Err(
                            format!("Could not start the local control server: {error}").into()
                        );
                    }
                    Err(error) => {
                        eprintln!("Local control is unavailable: {error}");
                    }
                }
            }
            if let Some(seconds) = auto_close_seconds {
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs_f64(seconds));
                    handle.exit(0);
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let state = window.app_handle().state::<AppState>();
            match event {
                tauri::WindowEvent::Resized { .. } | tauri::WindowEvent::Moved { .. } => {
                    state.0.window_geometry.record_window(window);
                }
                tauri::WindowEvent::CloseRequested { .. } => {
                    state.0.window_geometry.flush_window(window);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            scan_workspace,
            list_containers,
            system_metrics,
            container_logs,
            service_logs,
            start_container,
            stop_container,
            compose_down,
            compose_down_log,
            load_settings,
            save_settings,
            load_profiles,
            save_profile,
            delete_profile,
            task_snapshots,
            task_log_tail,
            start_task,
            stop_task,
            restart_task,
            stop_profile,
            terminate_service,
            restart_service,
            shutdown,
            update::check_for_update,
            update::update_and_restart
        ])
        .build(tauri::generate_context!())
        .expect("error while building Cutting Board")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                let state = app.state::<AppState>();
                if let Some(window) = app.get_webview_window("main") {
                    let window = window.as_ref().window();
                    state.0.window_geometry.flush_window(&window);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LaunchTask, ServiceSnapshot};
    use serde_json::Value;
    use std::time::Instant;

    pub(crate) fn test_app_state(directory: &std::path::Path) -> AppState {
        let settings_path = directory.join("settings.json");
        let profiles_path = directory.join("launch-profiles.json");
        let logs_dir = directory.join("logs");
        let settings_io = Arc::new(Mutex::new(()));
        let control = control_service::ControlState::default();
        AppState(Arc::new(AppStateInner {
            demo: false,
            settings_path: settings_path.clone(),
            profiles_path,
            logs_dir,
            settings_io: Arc::clone(&settings_io),
            window_geometry: window::WindowGeometryPersistence::new(settings_path, settings_io),
            scan: Mutex::new(ScanState::default()),
            launch: Mutex::new(LaunchManager::reporting_to(control.clone())),
            control,
            system_metrics: Mutex::new(SystemMetricsState::default()),
            compose_down_logs: docker::ComposeDownLogs::default(),
        }))
    }

    struct StopTestTasks(AppState);

    impl Drop for StopTestTasks {
        fn drop(&mut self) {
            let state = &self.0;
            if let Ok(mut manager) = state.0.launch.lock() {
                manager.stop_all();
            }
        }
    }

    #[test]
    fn settings_save_preserves_latest_window_geometry() {
        let mut requested = UiSettings {
            theme_mode: "light".into(),
            scan_interval_ms: 5_000,
            window_width: 1_080,
            window_height: 720,
            window_x: Some(10),
            window_y: Some(20),
            window_geometry_logical: true,
        };
        let current = UiSettings {
            window_width: 1_400,
            window_height: 900,
            window_x: Some(64),
            window_y: Some(96),
            window_geometry_logical: true,
            ..UiSettings::default()
        };

        preserve_saved_window_geometry(&mut requested, &current);

        assert_eq!(requested.theme_mode, "light");
        assert_eq!(requested.scan_interval_ms, 5_000);
        assert_eq!(requested.window_width, 1_400);
        assert_eq!(requested.window_height, 900);
        assert_eq!(requested.window_x, Some(64));
        assert_eq!(requested.window_y, Some(96));
        assert!(requested.window_geometry_logical);
    }

    #[test]
    fn service_without_process_returns_unavailable_logs() {
        let workspace = WorkspaceSnapshot {
            services: vec![ServiceSnapshot {
                id: "service".into(),
                display_name: "Example".into(),
                tech: "vite".into(),
                category: "web".into(),
                relevance: "dev".into(),
                endpoints: vec![],
                process: None,
                project: None,
                status: "limited".into(),
                warnings: vec![],
                origin_kind: "unknown".into(),
                origin_label: None,
                can_terminate: false,
                browser_url: None,
                active_profiles: vec![],
            }],
            scanned_at: 0,
            scan_duration_ms: 0,
            endpoint_count: 0,
            errors: vec![],
        };
        let settings_io = Arc::new(Mutex::new(()));
        let state = AppState(Arc::new(AppStateInner {
            demo: false,
            settings_path: PathBuf::new(),
            profiles_path: PathBuf::new(),
            logs_dir: PathBuf::new(),
            settings_io: Arc::clone(&settings_io),
            window_geometry: window::WindowGeometryPersistence::new(PathBuf::new(), settings_io),
            scan: Mutex::new(ScanState {
                workspace: Some(workspace),
                service_index: HashMap::new(),
            }),
            launch: Mutex::new(LaunchManager::default()),
            control: control_service::ControlState::default(),
            system_metrics: Mutex::new(SystemMetricsState::default()),
            compose_down_logs: docker::ComposeDownLogs::default(),
        }));

        let snapshot = read_service_logs(&state, "service").unwrap();

        assert!(!snapshot.available);
        assert!(snapshot.logs.is_empty());
        assert_eq!(snapshot.source_path, None);
        assert!(snapshot.message.is_some());
    }

    #[test]
    fn expected_port_probe_does_not_conflict_with_its_own_ipv4_listener() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        assert!(!port_is_occupied(port));
    }

    #[test]
    fn expected_port_probe_detects_occupied_ipv4_and_ipv6_ports() {
        let ipv4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let ipv4_port = ipv4.local_addr().unwrap().port();
        assert!(port_is_occupied(ipv4_port));
        drop(ipv4);

        let Ok(ipv6) = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)) else {
            return;
        };
        let ipv6_port = ipv6.local_addr().unwrap().port();
        assert!(port_is_occupied(ipv6_port));
    }

    #[cfg(unix)]
    #[test]
    fn ui_and_control_actions_share_launch_manager_lifecycle_and_snapshots() {
        let temporary = tempfile::tempdir().unwrap();
        let state = test_app_state(temporary.path());
        let _cleanup = StopTestTasks(state.clone());
        let profile = LaunchProfile {
            id: "integration-profile".into(),
            name: "Integration test".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "dummy".into(),
                cwd: ".".into(),
                command: "sleep 30".into(),
                expected_port: None,
                container: None,
                prepare: None,
            }],
        };
        persist_profile(&state.0.profiles_path, profile).unwrap();
        let request = TaskRequest {
            profile_id: "integration-profile".into(),
            task_name: "dummy".into(),
        };

        let first =
            run_ui_task_action(&state, &request, control_service::TaskAction::Start).unwrap();
        assert_eq!(first.state, "running");
        let profiles = profiles_for_state(&state).unwrap();
        let snapshots = control_task_snapshots(&state, &profiles).unwrap().unwrap();
        let shared = snapshots
            .iter()
            .find(|snapshot| snapshot.profile_id == request.profile_id)
            .unwrap();
        assert_eq!(shared.main_pid, first.main_pid);
        assert_eq!(shared.state, first.state);

        let accepted = control_service::dispatch(
            &state,
            control::ControlRequest::Start {
                profile_id: request.profile_id.clone(),
                task_name: request.task_name.clone(),
            },
        );
        assert!(accepted.ok && accepted.accepted && !accepted.completed);
        let operation_id = accepted.operation_id.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let repeated = loop {
            let response = control_service::dispatch(
                &state,
                control::ControlRequest::Operation {
                    operation_id: operation_id.clone(),
                },
            );
            if response.completed {
                break response;
            }
            assert!(
                Instant::now() < deadline,
                "idempotent start did not complete"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(repeated.ok);
        let repeated_task = repeated.result.unwrap()["task"].clone();
        assert_eq!(repeated_task["main_pid"], first.main_pid.unwrap());

        let restarted =
            run_ui_task_action(&state, &request, control_service::TaskAction::Restart).unwrap();
        assert_eq!(restarted.state, "running");
        let stopped =
            run_ui_task_action(&state, &request, control_service::TaskAction::Stop).unwrap();
        assert_eq!(stopped.state, "stopped");
    }

    #[cfg(unix)]
    #[test]
    fn asynchronous_control_failure_and_overlap_are_reported_without_starting_processes() {
        let temporary = tempfile::tempdir().unwrap();
        let state = test_app_state(temporary.path());
        let missing = LaunchProfile {
            id: "failure-profile".into(),
            name: "Failure test".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "missing-directory".into(),
                cwd: "not-here".into(),
                command: "sleep 30".into(),
                expected_port: None,
                container: None,
                prepare: None,
            }],
        };
        persist_profile(&state.0.profiles_path, missing).unwrap();
        let request = TaskRequest {
            profile_id: "failure-profile".into(),
            task_name: "missing-directory".into(),
        };
        let held = state
            .0
            .control
            .begin_task_action(&request, control_service::TaskAction::Start)
            .unwrap();
        let busy = control_service::dispatch(
            &state,
            control::ControlRequest::Start {
                profile_id: request.profile_id.clone(),
                task_name: request.task_name.clone(),
            },
        );
        assert_eq!(busy.error.unwrap().code, "busy");
        drop(held);

        let accepted = control_service::dispatch(
            &state,
            control::ControlRequest::Start {
                profile_id: request.profile_id,
                task_name: request.task_name,
            },
        );
        assert!(accepted.accepted && !accepted.completed);
        let operation_id = accepted.operation_id.unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let failed = loop {
            let response = control_service::dispatch(
                &state,
                control::ControlRequest::Operation {
                    operation_id: operation_id.clone(),
                },
            );
            if response.completed {
                break response;
            }
            assert!(Instant::now() < deadline, "failed start did not complete");
            thread::sleep(Duration::from_millis(20));
        };
        assert!(!failed.ok);
        assert_eq!(failed.error.unwrap().code, "missing_directory");
    }

    #[cfg(unix)]
    struct ControlFixture {
        temporary: tempfile::TempDir,
        state: AppState,
        socket: PathBuf,
        _server: control::ControlServer,
        _cleanup: StopTestTasks,
    }

    /// A real control server for temporary config, reached the way the CLI reaches it.
    #[cfg(unix)]
    fn control_fixture() -> ControlFixture {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let state = test_app_state(temporary.path());
        let control_dir = temporary.path().join("control");
        std::fs::create_dir(&control_dir).unwrap();
        std::fs::set_permissions(&control_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = control_dir.join("control.sock");
        let handler_state = state.clone();
        let server = control::start_server_at(socket.clone(), move |request| {
            control_service::dispatch(&handler_state, request)
        })
        .unwrap();
        ControlFixture {
            temporary,
            state: state.clone(),
            socket,
            _server: server,
            _cleanup: StopTestTasks(state),
        }
    }

    #[cfg(unix)]
    impl ControlFixture {
        fn root(&self) -> &std::path::Path {
            self.temporary.path()
        }

        fn save(&self, tasks: Vec<LaunchTask>) {
            persist_profile(
                &self.state.0.profiles_path,
                LaunchProfile {
                    id: "p".into(),
                    name: "Control test".into(),
                    project_root: self.root().to_string_lossy().into_owned(),
                    tasks,
                },
            )
            .unwrap();
        }

        fn saved_task(&self, name: &str) -> LaunchTask {
            read_profiles(&self.state.0.profiles_path).unwrap()[0]
                .tasks
                .iter()
                .find(|task| task.name == name)
                .unwrap()
                .clone()
        }

        fn run_action(&self, request: control::ControlRequest) -> control::ControlResponse {
            let accepted = control_service::dispatch(&self.state, request);
            let Some(operation_id) = accepted.operation_id.clone() else {
                return accepted;
            };
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                let response = control_service::dispatch(
                    &self.state,
                    control::ControlRequest::Operation {
                        operation_id: operation_id.clone(),
                    },
                );
                if response.completed {
                    return response;
                }
                assert!(Instant::now() < deadline, "operation did not complete");
                thread::sleep(Duration::from_millis(20));
            }
        }

        fn start(&self, task_name: &str) {
            let response = self.run_action(control::ControlRequest::Start {
                profile_id: "p".into(),
                task_name: task_name.into(),
            });
            assert!(response.ok, "{:?}", response.error);
        }

        fn wait(
            &self,
            task_name: &str,
            log_pattern: Option<&str>,
            timeout: Duration,
        ) -> (control::ControlResponse, i32) {
            control::test_client::wait(
                &self.socket,
                &control::ControlRequest::Readiness {
                    profile_id: "p".into(),
                    task_name: task_name.into(),
                    log_pattern: log_pattern.map(str::to_owned),
                },
                timeout,
            )
        }

        fn logs(&self, task_name: &str, current_run: bool) -> control::ControlResponse {
            control::test_client::send(
                &self.socket,
                &control::ControlRequest::Logs {
                    profile_id: "p".into(),
                    task_name: task_name.into(),
                    lines: None,
                    bytes: None,
                    current_run,
                    prepare: false,
                },
            )
            .0
        }

        fn update(
            &self,
            task_name: &str,
            change: impl FnOnce(&mut control::ControlRequest),
        ) -> (control::ControlResponse, i32) {
            let mut request = control::ControlRequest::Update {
                profile_id: "p".into(),
                task_name: task_name.into(),
                command: None,
                cwd: None,
                expected_port: None,
                clear_expected_port: false,
                clear_prepare: false,
            };
            change(&mut request);
            control::test_client::send(&self.socket, &request)
        }
    }

    #[cfg(unix)]
    fn plain_task(name: &str, command: &str) -> LaunchTask {
        LaunchTask {
            name: name.into(),
            cwd: ".".into(),
            command: command.into(),
            expected_port: None,
            container: None,
            prepare: None,
        }
    }

    #[cfg(unix)]
    fn print_response(label: &str, response: &control::ControlResponse) {
        println!("{label}: {}", serde_json::to_string(response).unwrap());
    }

    #[cfg(unix)]
    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[cfg(target_os = "linux")]
    fn listener_after(delay_seconds: u32, port: u16, directory: &std::path::Path) -> String {
        format!(
            "sleep {delay_seconds}; CUTTING_BOARD_TEST_LISTENER_PORT={port} CUTTING_BOARD_TEST_LISTENER_MARKER={} exec {} --exact launch::tests::managed_listener_process_fixture --nocapture",
            directory.join("listener.marker").display(),
            std::env::current_exe().unwrap().display(),
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wait_waits_for_a_delayed_listener_and_a_timeout_cancels_nothing() {
        let fixture = control_fixture();
        let port = free_port();
        let mut task = plain_task("api", &listener_after(5, port, fixture.root()));
        task.expected_port = Some(port);
        fixture.save(vec![task]);
        fixture.start("api");

        let (timed_out, exit) = fixture.wait("api", None, Duration::from_secs(1));
        print_response("wait timeout", &timed_out);
        assert_eq!(exit, 5);
        assert!(!timed_out.ok);
        assert_eq!(timed_out.result.as_ref().unwrap()["waiting_for"], "port");
        assert_eq!(timed_out.error.unwrap().code, "timeout");

        let (ready, exit) = fixture.wait("api", None, Duration::from_secs(30));
        print_response("wait ready", &ready);
        assert_eq!(exit, 0);
        let ready = ready.result.unwrap();
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["signal"], "port");
        assert_eq!(ready["port"], port);
        assert!(std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn old_output_does_not_satisfy_a_log_pattern_after_a_restart() {
        let fixture = control_fixture();
        fixture.save(vec![plain_task(
            "api",
            ": --password=hunter2; if [ -e second-run ]; then echo booting; sleep 6; echo 'Started App'; else touch second-run; echo 'Started App'; fi; sleep 60",
        )]);
        fixture.start("api");
        let pattern = Some("Started App");
        let (ready, exit) = fixture.wait("api", pattern, Duration::from_secs(10));
        print_response("wait log pattern", &ready);
        assert_eq!(exit, 0);
        assert_eq!(ready.result.unwrap()["signal"], "log_pattern");

        let restarted = fixture.run_action(control::ControlRequest::Restart {
            profile_id: "p".into(),
            task_name: "api".into(),
        });
        assert!(restarted.ok, "{:?}", restarted.error);
        let (not_yet, exit) = fixture.wait("api", pattern, Duration::from_secs(2));
        print_response("wait log pattern after restart", &not_yet);
        assert_eq!(exit, 5);
        assert_eq!(not_yet.result.unwrap()["waiting_for"], "log_pattern");

        let plain = fixture.logs("api", false);
        print_response("logs", &plain);
        let plain = plain.result.unwrap();
        assert!(plain["logs"].as_str().unwrap().contains("Started App"));
        assert!(plain.get("current_run").is_none());
        let current = fixture.logs("api", true);
        print_response("logs --current-run", &current);
        let current = current.result.unwrap();
        let text = current["logs"].as_str().unwrap();
        assert!(text.contains("booting"));
        assert!(!text.contains("Started App"));
        assert!(!text.contains("Cutting Board"));
        assert_eq!(current["current_run"]["boundary"], "start_marker");
        assert!(current["current_run"]["started_at"].is_u64());
        for logs in [&plain, &current] {
            let text = logs.to_string();
            assert!(!text.contains("hunter2"));
            assert!(!text.contains(&*fixture.root().to_string_lossy()));
        }

        let (ready, exit) = fixture.wait("api", pattern, Duration::from_secs(30));
        assert_eq!(exit, 0);
        assert_eq!(ready.result.unwrap()["signal"], "log_pattern");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_prepare_with_the_old_process_still_listening_is_not_ready() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = control_fixture();
        let root = fixture.root();
        let port = free_port();
        let mut task = plain_task("api", &listener_after(0, port, root));
        task.expected_port = Some(port);
        fixture.save(vec![task.clone()]);
        fixture.start("api");
        let (ready, exit) = fixture.wait("api", None, Duration::from_secs(30));
        assert_eq!(exit, 0, "{:?}", ready.error);

        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let wrapper = root.join("mvnw");
        std::fs::write(&wrapper, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        task.prepare = Some(crate::models::LaunchPrepareSpec {
            kind: crate::models::LaunchPrepareKind::SpringBoot,
            build_tool: Some(crate::models::LaunchBuildTool::Maven),
            module: None,
            profiles: vec![],
            main_class: Some("App".into()),
        });
        fixture.save(vec![task]);
        let failed = fixture.run_action(control::ControlRequest::Restart {
            profile_id: "p".into(),
            task_name: "api".into(),
        });
        assert!(!failed.ok);
        assert_eq!(failed.error.unwrap().code, "prepare_failed");
        assert!(std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_ok());

        let (result, exit) = fixture.wait("api", None, Duration::from_secs(10));
        print_response("wait after failed prepare", &result);
        assert_eq!(exit, 6);
        assert_eq!(result.error.unwrap().code, "prepare_failed");
    }

    #[cfg(unix)]
    fn prepare_logs(fixture: &ControlFixture, task_name: &str) -> (control::ControlResponse, i32) {
        control::test_client::send(
            &fixture.socket,
            &control::ControlRequest::Logs {
                profile_id: "p".into(),
                task_name: task_name.into(),
                lines: None,
                bytes: None,
                current_run: false,
                prepare: true,
            },
        )
    }

    #[cfg(unix)]
    fn maven_task(root: &std::path::Path, wrapper: &str, name: &str) -> LaunchTask {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(root.join("pom.xml"), "<project/>").unwrap();
        let path = root.join("mvnw");
        std::fs::write(&path, wrapper).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut task = plain_task(name, "sleep 60");
        task.prepare = Some(crate::models::LaunchPrepareSpec {
            kind: crate::models::LaunchPrepareKind::SpringBoot,
            build_tool: Some(crate::models::LaunchBuildTool::Maven),
            module: None,
            profiles: vec![],
            main_class: Some("App".into()),
        });
        task
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_first_start_explains_itself_through_the_control_socket() {
        let fixture = control_fixture();
        let root = fixture.root();
        let task = maven_task(
            root,
            "#!/bin/sh\necho '[ERROR] cannot find symbol'\nexit 1\n",
            "api",
        );
        fixture.save(vec![task]);

        let failed = fixture.run_action(control::ControlRequest::Start {
            profile_id: "p".into(),
            task_name: "api".into(),
        });
        print_response("failed start", &failed);
        let error = failed.error.clone().unwrap();
        assert_eq!(error.code, "prepare_failed");
        assert!(error.message.contains("logs --prepare"));
        let operation = &failed.result.unwrap()["operation"];
        assert_eq!(operation["prepare"]["state"], "failed");
        assert_eq!(operation["prepare"]["tool"], "maven");
        assert!(!operation.to_string().contains("mvnw"));

        // The run has not started, so the current run has nothing to say about why.
        assert_eq!(
            fixture.logs("api", true).result.unwrap()["current_run"]["boundary"],
            "none"
        );
        let (read, exit) = prepare_logs(&fixture, "api");
        print_response("logs --prepare", &read);
        assert_eq!(exit, 0);
        let read = read.result.unwrap();
        assert_eq!(read["logs"], "[ERROR] cannot find symbol");
        assert_eq!(read["lines"], 1);
        assert_eq!(read["prepare_run"]["state"], "failed");
        assert!(read["prepare_run"]["failure"]
            .as_str()
            .unwrap()
            .contains("exited with"));
    }

    #[cfg(unix)]
    #[test]
    fn prepare_logs_answer_for_tasks_without_a_build() {
        let fixture = control_fixture();
        let mut container = plain_task("db", "");
        container.container = Some("db".into());
        fixture.save(vec![plain_task("api", "sleep 60"), container]);

        let (read, exit) = prepare_logs(&fixture, "api");
        assert_eq!(exit, 0);
        assert_eq!(read.result.unwrap()["prepare_run"]["state"], "none");

        fixture.start("api");
        let (read, _) = prepare_logs(&fixture, "api");
        let read = read.result.unwrap();
        assert_eq!(read["prepare_run"]["state"], "not_required");
        assert_eq!(read["logs"], "");

        let (read, exit) = prepare_logs(&fixture, "db");
        assert_eq!(exit, 2);
        assert_eq!(read.error.unwrap().code, "invalid_request");

        let (combined, exit) = control::test_client::send(
            &fixture.socket,
            &control::ControlRequest::Logs {
                profile_id: "p".into(),
                task_name: "api".into(),
                lines: None,
                bytes: None,
                current_run: true,
                prepare: true,
            },
        );
        assert_eq!(exit, 2);
        assert_eq!(combined.error.unwrap().code, "invalid_request");
    }

    #[cfg(unix)]
    #[test]
    fn wait_and_status_show_a_build_while_it_runs() {
        let fixture = control_fixture();
        let root = fixture.root();
        let release = root.join("release");
        let task = maven_task(
            root,
            &format!(
                "#!/bin/sh\necho compiling\nwhile [ ! -e '{}' ]; do sleep 0.05; done\nexit 1\n",
                release.display()
            ),
            "api",
        );
        fixture.save(vec![task]);

        let accepted = control_service::dispatch(
            &fixture.state,
            control::ControlRequest::Start {
                profile_id: "p".into(),
                task_name: "api".into(),
            },
        );
        let operation_id = accepted.operation_id.unwrap();
        let building = |response: &control::ControlResponse| {
            response.result.as_ref().unwrap()["operation"]["prepare"]["state"] == "running"
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let operation = control::ControlRequest::Operation {
            operation_id: operation_id.clone(),
        };
        while !building(&control_service::dispatch(
            &fixture.state,
            operation.clone(),
        )) {
            assert!(
                Instant::now() < deadline,
                "the build never showed as running"
            );
            thread::sleep(Duration::from_millis(20));
        }

        let (waited, exit) = fixture.wait("api", None, Duration::from_secs(1));
        assert_eq!(exit, 5);
        assert_eq!(waited.result.unwrap()["waiting_for"], "prepare");
        let (status, _) = control::test_client::send(
            &fixture.socket,
            &control::ControlRequest::Status {
                profile_id: "p".into(),
                task_name: "api".into(),
            },
        );
        let status = status.result.unwrap();
        assert_eq!(
            status["task"]["last_operation"]["prepare"]["state"],
            "running"
        );
        let (read, _) = prepare_logs(&fixture, "api");
        let read = read.result.unwrap();
        assert_eq!(read["prepare_run"]["state"], "running");
        assert!(read["logs"].as_str().unwrap().contains("compiling"));

        std::fs::write(&release, "").unwrap();
        while !control_service::dispatch(&fixture.state, operation.clone()).completed {
            assert!(Instant::now() < deadline, "the build never finished");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn show_gives_the_command_a_prepared_task_really_runs() {
        let fixture = control_fixture();
        let task = maven_task(fixture.root(), "#!/bin/sh\nexit 0\n", "api");
        fixture.save(vec![task]);

        let (shown, exit) = control::test_client::send(
            &fixture.socket,
            &control::ControlRequest::Show {
                profile_id: "p".into(),
                task_name: "api".into(),
            },
        );
        print_response("show prepared", &shown);
        assert_eq!(exit, 0);
        let shown = shown.result.unwrap();
        let task = &shown["task"];
        assert_eq!(task["launch_source"], "prepare");
        assert_eq!(task["resolved"]["prepare"]["origin"], "saved");
        assert!(task["resolved"]["prepare"]["command"]
            .as_str()
            .unwrap()
            .contains("mvnw install"));
        assert!(task["resolved"]["launch"]["command"]
            .as_str()
            .unwrap()
            .contains("exec-maven-plugin"));
        assert_eq!(task["resolved"]["error"], Value::Null);
    }

    #[cfg(unix)]
    #[test]
    fn show_and_update_round_trip_and_refuse_an_active_task() {
        let fixture = control_fixture();
        fixture.save(vec![
            plain_task("api", "sleep 60"),
            plain_task("sibling", "sleep 60"),
        ]);
        fixture.start("sibling");
        let unsaved_port = free_port();

        let (updated, exit) = fixture.update("api", |request| {
            if let control::ControlRequest::Update {
                command,
                cwd,
                expected_port,
                ..
            } = request
            {
                *command = Some("  sleep 61  ".into());
                *cwd = Some(" . ".into());
                *expected_port = Some(unsaved_port);
            }
        });
        print_response("update", &updated);
        assert_eq!(exit, 0, "{:?}", updated.error);
        let updated = updated.result.unwrap();
        assert_eq!(updated["changed"]["command"]["old"], "sleep 60");
        assert_eq!(updated["changed"]["command"]["new"], "sleep 61");
        assert_eq!(updated["changed"]["expected_port"]["old"], Value::Null);
        assert!(updated["changed"].get("cwd").is_none());
        assert_eq!(updated["warnings"], serde_json::json!([]));
        assert_eq!(fixture.saved_task("api").command, "sleep 61");
        assert_eq!(fixture.saved_task("api").expected_port, Some(unsaved_port));

        let (shown, exit) = control::test_client::send(
            &fixture.socket,
            &control::ControlRequest::Show {
                profile_id: "p".into(),
                task_name: "api".into(),
            },
        );
        print_response("show", &shown);
        assert_eq!(exit, 0);
        let shown = shown.result.unwrap();
        assert_eq!(shown["task"], updated["task"]);
        assert_eq!(shown["task"]["launch_source"], "saved_command");
        assert_eq!(shown["task"]["command"], "sleep 61");
        assert!(shown["task"]["resolved_cwd"]
            .as_str()
            .unwrap()
            .starts_with(&*fixture.root().to_string_lossy()));

        let (refused, exit) = fixture.update("sibling", |request| {
            if let control::ControlRequest::Update { command, .. } = request {
                *command = Some("sleep 62".into());
            }
        });
        print_response("update of a running task", &refused);
        assert_eq!(exit, 6);
        assert_eq!(refused.error.unwrap().code, "task_active");
        assert_eq!(fixture.saved_task("sibling").command, "sleep 60");

        let (missing, exit) = fixture.update("nope", |request| {
            if let control::ControlRequest::Update { command, .. } = request {
                *command = Some("sleep 1".into());
            }
        });
        assert_eq!(exit, 4);
        assert_eq!(missing.error.unwrap().code, "unknown_task");

        let (invalid, exit) = fixture.update("api", |request| {
            if let control::ControlRequest::Update { expected_port, .. } = request {
                *expected_port = Some(0);
            }
        });
        assert_eq!(exit, 2);
        assert_eq!(invalid.error.unwrap().code, "invalid_request");
    }

    #[cfg(unix)]
    #[test]
    fn update_warns_when_the_saved_command_will_not_run_verbatim() {
        let fixture = control_fixture();
        std::fs::write(
            fixture.root().join("build.gradle"),
            "plugins { id 'org.springframework.boot' version '3.4.0' }\n",
        )
        .unwrap();
        fixture.save(vec![plain_task(
            "api",
            "java -cp stale.jar com.example.Application",
        )]);

        let (updated, exit) = fixture.update("api", |request| {
            if let control::ControlRequest::Update { expected_port, .. } = request {
                *expected_port = Some(8080);
            }
        });
        print_response("update on the prepare path", &updated);
        assert_eq!(exit, 0, "{:?}", updated.error);
        let updated = updated.result.unwrap();
        assert_eq!(updated["task"]["launch_source"], "prepare");
        assert!(updated["warnings"][0]
            .as_str()
            .unwrap()
            .contains("not run verbatim"));

        let (cleared, exit) = fixture.update("api", |request| {
            if let control::ControlRequest::Update { clear_prepare, .. } = request {
                *clear_prepare = true;
            }
        });
        assert_eq!(exit, 0, "{:?}", cleared.error);
        assert!(cleared.result.unwrap()["warnings"][0]
            .as_str()
            .unwrap()
            .contains("auto-detection"));
    }

    #[test]
    fn saving_an_edit_form_refuses_a_profile_that_changed_after_it_opened() {
        let temporary = tempfile::tempdir().unwrap();
        let state = test_app_state(temporary.path());
        let profile_with = |command: &str| LaunchProfile {
            id: "p".into(),
            name: "Profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "api".into(),
                cwd: ".".into(),
                command: command.into(),
                expected_port: None,
                container: None,
                prepare: None,
            }],
        };
        let saved_command = || {
            read_profiles(&state.0.profiles_path).unwrap()[0].tasks[0]
                .command
                .clone()
        };
        let opened = profile_with("as opened");

        // A new profile has nothing to compare against.
        save_profile_from_form(&state, opened.clone(), None).unwrap();
        assert_eq!(saved_command(), "as opened");

        save_profile_from_form(&state, profile_with("edited"), Some(opened.clone())).unwrap();
        assert_eq!(saved_command(), "edited");

        // The saved profile now differs from what this form opened with.
        let error = save_profile_from_form(&state, profile_with("overwrite"), Some(opened.clone()))
            .unwrap_err();
        assert!(error.contains("changed after this form was opened"));
        assert_eq!(saved_command(), "edited");

        // So does a profile deleted meanwhile, which saving must not bring back.
        let gone = LaunchProfile {
            id: "gone".into(),
            ..opened.clone()
        };
        let error = save_profile_from_form(&state, gone.clone(), Some(gone)).unwrap_err();
        assert!(error.contains("changed after this form was opened"));
        assert_eq!(read_profiles(&state.0.profiles_path).unwrap().len(), 1);
    }
}
