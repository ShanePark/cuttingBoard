mod cli;
mod docker;
mod launch;
mod models;
mod process_control;
mod scanner;
mod storage;
mod update;
mod window;

use crate::{
    launch::{containers as launch_containers, LaunchManager},
    models::{
        AppInfo, ContainerActionResult, ContainerListing, ContainerLogSnapshot, ContainerRequest,
        LaunchProfile, ManagedTaskSnapshot, RestartServiceRequest, ServiceIdentity,
        ServiceLogSnapshot, ServiceRequest, TaskRequest, TerminateRequest, TerminationResult,
        UiSettings, WorkspaceSnapshot,
    },
    storage::{
        delete_profile as remove_profile, demo_profiles, load_profiles as read_profiles,
        load_settings as read_settings, save_profile as persist_profile,
        save_settings as persist_settings,
    },
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
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
    scan: Mutex<ScanState>,
    launch: Mutex<LaunchManager>,
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
    tauri::async_runtime::spawn_blocking(move || {
        let (snapshot, index) = scanner::scan_workspace(state.0.demo)?;
        let mut scan = lock(&state.0.scan)?;
        scan.workspace = Some(snapshot.clone());
        scan.service_index = index;
        Ok(snapshot)
    })
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
fn load_settings(state: State<'_, AppState>) -> Result<UiSettings, String> {
    read_settings(&state.0.settings_path)
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: UiSettings) -> Result<UiSettings, String> {
    persist_settings(&state.0.settings_path, settings)
}

#[tauri::command]
fn load_profiles(state: State<'_, AppState>) -> Result<Vec<LaunchProfile>, String> {
    profiles_for_state(state.inner())
}

#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    profile: LaunchProfile,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(state.inner())?;
    let current_profiles = read_profiles(&state.0.profiles_path)?;
    ensure_profile_inactive(
        state.inner(),
        &current_profiles,
        &profile.id,
        "Stop every task in this profile before editing it.",
    )?;
    persist_profile(&state.0.profiles_path, profile)
}

#[tauri::command]
async fn delete_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
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

#[tauri::command]
async fn start_task(
    state: State<'_, AppState>,
    request: TaskRequest,
) -> Result<ManagedTaskSnapshot, String> {
    reject_demo(state.inner())?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (profiles, workspace) = launch_context(&state)?;
        if let Some(container) = launch_containers::container_for(&profiles, &request) {
            return launch_containers::start(&request, &container, state.0.demo);
        }
        lock(&state.0.launch)?.start_task(
            &profiles,
            &request,
            &state.0.logs_dir,
            workspace.as_ref(),
        )
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
        let (profiles, workspace) = launch_context(&state)?;
        if let Some(container) = launch_containers::container_for(&profiles, &request) {
            return launch_containers::stop(&request, &container, state.0.demo);
        }
        lock(&state.0.launch)?.stop_task(&profiles, &request, &state.0.logs_dir, workspace.as_ref())
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
        let (profiles, workspace) = launch_context(&state)?;
        if let Some(container) = launch_containers::container_for(&profiles, &request) {
            return launch_containers::restart(&request, &container, state.0.demo);
        }
        lock(&state.0.launch)?.restart_task(
            &profiles,
            &request,
            &state.0.logs_dir,
            workspace.as_ref(),
        )
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
        let (profiles, workspace) = launch_context(&state)?;
        let mut snapshots = launch_containers::stop_profile(&profiles, &profile_id, state.0.demo)?;
        snapshots.extend(lock(&state.0.launch)?.stop_profile(
            &profiles,
            &profile_id,
            &state.0.logs_dir,
            workspace.as_ref(),
        )?);
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
            eprintln!("{error}\n\nRun with --help for usage.");
            std::process::exit(2);
        }
    };
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
            app.manage(AppState(Arc::new(AppStateInner {
                demo,
                settings_path,
                profiles_path,
                logs_dir,
                scan: Mutex::new(ScanState::default()),
                launch: Mutex::new(LaunchManager::default()),
            })));
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
                    window::persist_window_geometry(window, &state.0.settings_path);
                }
                tauri::WindowEvent::CloseRequested { .. } => {
                    window::persist_window_geometry(window, &state.0.settings_path);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            scan_workspace,
            list_containers,
            container_logs,
            service_logs,
            start_container,
            stop_container,
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
                    window::persist_window_geometry(&window, &state.0.settings_path);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServiceSnapshot;

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
        let state = AppState(Arc::new(AppStateInner {
            demo: false,
            settings_path: PathBuf::new(),
            profiles_path: PathBuf::new(),
            logs_dir: PathBuf::new(),
            scan: Mutex::new(ScanState {
                workspace: Some(workspace),
                service_index: HashMap::new(),
            }),
            launch: Mutex::new(LaunchManager::default()),
        }));

        let snapshot = read_service_logs(&state, "service").unwrap();

        assert!(!snapshot.available);
        assert!(snapshot.logs.is_empty());
        assert_eq!(snapshot.source_path, None);
        assert!(snapshot.message.is_some());
    }
}
