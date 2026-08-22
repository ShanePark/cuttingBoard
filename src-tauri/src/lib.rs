mod docker;
mod launch;
mod models;
mod scanner;
mod storage;

use crate::{
    launch::LaunchManager,
    models::{
        AppInfo, ContainerActionResult, ContainerListing, ContainerLogSnapshot, ContainerRequest,
        LaunchProfile, ManagedTaskSnapshot, ServiceIdentity, ServiceLogSnapshot, ServiceRequest,
        ServiceSnapshot, TaskRequest, TerminateRequest, TerminationResult, UiSettings,
        WorkspaceSnapshot,
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
use sysinfo::{Pid, System};
use tauri::{Manager, PhysicalSize, State};

#[derive(Clone)]
struct AppState(Arc<AppStateInner>);

struct AppStateInner {
    demo: bool,
    settings_path: PathBuf,
    profiles_path: PathBuf,
    logs_dir: PathBuf,
    last_workspace: Mutex<Option<WorkspaceSnapshot>>,
    service_index: Mutex<HashMap<String, ServiceIdentity>>,
    launch: Mutex<LaunchManager>,
}

#[derive(Debug, Default)]
struct CliOptions {
    demo: bool,
    auto_close_seconds: Option<f64>,
    show_help: bool,
    show_version: bool,
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        demo: state.0.demo,
        settings_path: state.0.settings_path.to_string_lossy().into_owned(),
        profiles_path: state.0.profiles_path.to_string_lossy().into_owned(),
    }
}

#[tauri::command]
async fn scan_workspace(state: State<'_, AppState>) -> Result<WorkspaceSnapshot, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let (snapshot, index) = scanner::scan_workspace(state.0.demo)?;
        *lock(&state.0.last_workspace)? = Some(snapshot.clone());
        *lock(&state.0.service_index)? = index;
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

    let workspace = lock(&state.0.last_workspace)?
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
    let identity = service_identity(&service)?;
    validate_service_log_identity(&identity)?;
    let profiles = read_profiles(&state.0.profiles_path)?;
    lock(&state.0.launch)?.service_logs(&profiles, &service, &state.0.logs_dir)
}

#[tauri::command]
async fn start_container(
    state: State<'_, AppState>,
    request: ContainerRequest,
) -> Result<ContainerActionResult, String> {
    reject_demo(&state)?;
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
    reject_demo(&state)?;
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
    if state.0.demo {
        Ok(demo_profiles())
    } else {
        read_profiles(&state.0.profiles_path)
    }
}

#[tauri::command]
fn save_profile(
    state: State<'_, AppState>,
    profile: LaunchProfile,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(&state)?;
    let current_profiles = read_profiles(&state.0.profiles_path)?;
    let workspace = lock(&state.0.last_workspace)?.clone();
    if lock(&state.0.launch)?.profile_is_active(&current_profiles, &profile.id, workspace.as_ref()) {
        return Err("Stop every task in this profile before editing it.".into());
    }
    persist_profile(&state.0.profiles_path, profile)
}

#[tauri::command]
fn delete_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<LaunchProfile>, String> {
    reject_demo(&state)?;
    let profiles = read_profiles(&state.0.profiles_path)?;
    let workspace = lock(&state.0.last_workspace)?.clone();
    if lock(&state.0.launch)?.profile_is_active(&profiles, &profile_id, workspace.as_ref()) {
        return Err("Stop every task in this profile before deleting it.".into());
    }
    remove_profile(&state.0.profiles_path, &profile_id)
}

#[tauri::command]
fn task_snapshots(state: State<'_, AppState>) -> Result<Vec<ManagedTaskSnapshot>, String> {
    let profiles = profiles_for_state(&state)?;
    let workspace = lock(&state.0.last_workspace)?.clone();
    Ok(lock(&state.0.launch)?.snapshots(
        &profiles,
        workspace.as_ref(),
        &state.0.logs_dir,
    ))
}

#[tauri::command]
async fn start_task(
    state: State<'_, AppState>,
    request: TaskRequest,
) -> Result<ManagedTaskSnapshot, String> {
    reject_demo(&state)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = read_profiles(&state.0.profiles_path)?;
        let workspace = lock(&state.0.last_workspace)?.clone();
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
    reject_demo(&state)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = read_profiles(&state.0.profiles_path)?;
        let workspace = lock(&state.0.last_workspace)?.clone();
        lock(&state.0.launch)?.stop_task(&profiles, &request, workspace.as_ref())
    })
    .await
    .map_err(|error| format!("Stop task failed: {error}"))?
}

#[tauri::command]
async fn stop_profile(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<Vec<ManagedTaskSnapshot>, String> {
    reject_demo(&state)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profiles = read_profiles(&state.0.profiles_path)?;
        let workspace = lock(&state.0.last_workspace)?.clone();
        lock(&state.0.launch)?.stop_profile(&profiles, &profile_id, workspace.as_ref())
    })
    .await
    .map_err(|error| format!("Stop profile failed: {error}"))?
}

#[tauri::command]
async fn terminate_service(
    state: State<'_, AppState>,
    request: TerminateRequest,
) -> Result<TerminationResult, String> {
    reject_demo(&state)?;
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || terminate_discovered_service(&state, &request))
        .await
        .map_err(|error| format!("Termination task failed: {error}"))?
}

#[tauri::command]
fn shutdown(state: State<'_, AppState>) -> Result<(), String> {
    lock(&state.0.launch)?.stop_all();
    Ok(())
}

fn persist_window_geometry<R: tauri::Runtime>(window: &tauri::Window<R>, state: &AppState) {
    let (Ok(size), Ok(position), Ok(scale_factor)) = (
        window.inner_size(),
        window.outer_position(),
        window.scale_factor(),
    ) else {
        return;
    };
    let logical_size = size.to_logical::<u32>(scale_factor);
    let settings = read_settings(&state.0.settings_path).unwrap_or_default();
    let updated = UiSettings {
        window_width: logical_size.width.max(560),
        window_height: logical_size.height.max(420),
        window_x: Some(position.x),
        window_y: Some(position.y),
        window_geometry_logical: true,
        ..settings
    };
    if let Err(error) = persist_settings(&state.0.settings_path, updated) {
        eprintln!("Could not persist window geometry: {error}");
    }
}

fn migrate_legacy_window_size(mut settings: UiSettings, scale_factor: f64) -> Option<UiSettings> {
    if settings.window_geometry_logical
        || settings.window_x.is_none()
        || settings.window_y.is_none()
        || !scale_factor.is_finite()
        || scale_factor <= 1.0
    {
        return None;
    }

    let logical_size = PhysicalSize::new(settings.window_width, settings.window_height)
        .to_logical::<u32>(scale_factor);
    settings.window_width = logical_size.width.max(560);
    settings.window_height = logical_size.height.max(420);
    settings.window_geometry_logical = true;
    Some(settings)
}

fn migrate_startup_window_settings<R: tauri::Runtime>(
    window: &tauri::Window<R>,
    settings_path: &std::path::Path,
    settings: UiSettings,
) -> UiSettings {
    let Ok(scale_factor) = window.scale_factor() else {
        return settings;
    };
    let Some(migrated) = migrate_legacy_window_size(settings.clone(), scale_factor) else {
        return settings;
    };
    if let Err(error) = persist_settings(settings_path, migrated.clone()) {
        eprintln!("Could not persist migrated window geometry: {error}");
    }
    migrated
}

fn stop_managed_tasks(state: &AppState) {
    if let Ok(mut launch) = state.0.launch.lock() {
        launch.stop_all();
    }
}

fn terminate_discovered_service(
    state: &AppState,
    request: &TerminateRequest,
) -> Result<TerminationResult, String> {
    let identity = lock(&state.0.service_index)?
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| {
            "The service changed since the last scan. Refresh and try again.".to_string()
        })?;
    if identity.pid <= 1 || identity.pid == std::process::id() {
        return Err("Cutting Board refused to stop that process.".into());
    }
    let current_uid = effective_uid();
    if identity.uid.is_some() && current_uid.is_some() && identity.uid != current_uid {
        return Err("Cutting Board only stops processes owned by the current user.".into());
    }

    let system = System::new_all();
    let process = system
        .process(Pid::from_u32(identity.pid))
        .ok_or_else(|| "The process already exited.".to_string())?;
    if process.start_time() != identity.start_time {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }

    send_signal(identity.pid, libc::SIGTERM)?;
    for _ in 0..25 {
        thread::sleep(Duration::from_millis(80));
        if !process_exists(identity.pid) {
            return Ok(TerminationResult {
                success: true,
                message: format!("Stopped {}.", identity.display_name),
            });
        }
    }
    send_signal(identity.pid, libc::SIGKILL)?;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(60));
        if !process_exists(identity.pid) {
            return Ok(TerminationResult {
                success: true,
                message: format!(
                    "Forced {} to stop after it ignored SIGTERM.",
                    identity.display_name
                ),
            });
        }
    }
    Ok(TerminationResult {
        success: false,
        message: format!("{} did not stop.", identity.display_name),
    })
}

fn service_identity(service: &ServiceSnapshot) -> Result<ServiceIdentity, String> {
    let process = service.process.as_ref().ok_or_else(|| {
        "The service has no current process identity. Refresh and try again.".to_string()
    })?;
    Ok(ServiceIdentity {
        pid: process.pid,
        start_time: process.create_time,
        uid: process.uid,
        display_name: service.display_name.clone(),
    })
}

fn validate_service_log_identity(identity: &ServiceIdentity) -> Result<(), String> {
    if identity.pid <= 1 || identity.pid == std::process::id() {
        return Err("Cutting Board refused to inspect that process.".into());
    }
    let current_uid = effective_uid();
    if identity.uid.is_some() && current_uid.is_some() && identity.uid != current_uid {
        return Err(
            "Cutting Board only reads logs from processes owned by the current user.".into(),
        );
    }

    let system = System::new_all();
    let process = system
        .process(Pid::from_u32(identity.pid))
        .ok_or_else(|| "The process already exited. Refresh and try again.".to_string())?;
    if process.start_time() != identity.start_time {
        return Err(
            "The PID was reused by another process. Refresh before reading its logs.".into(),
        );
    }
    Ok(())
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: i32) -> Result<(), String> {
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!("Could not signal PID {pid}: {error}"))
    }
}

#[cfg(not(unix))]
fn send_signal(pid: u32, _signal: i32) -> Result<(), String> {
    let system = System::new_all();
    let process = system
        .process(Pid::from_u32(pid))
        .ok_or_else(|| "The process already exited.".to_string())?;
    if process.kill() {
        Ok(())
    } else {
        Err(format!("Could not stop PID {pid}."))
    }
}

fn process_exists(pid: u32) -> bool {
    System::new_all().process(Pid::from_u32(pid)).is_some()
}

#[cfg(unix)]
fn effective_uid() -> Option<u32> {
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn effective_uid() -> Option<u32> {
    None
}

fn profiles_for_state(state: &State<'_, AppState>) -> Result<Vec<LaunchProfile>, String> {
    if state.0.demo {
        Ok(demo_profiles())
    } else {
        read_profiles(&state.0.profiles_path)
    }
}

fn reject_demo(state: &State<'_, AppState>) -> Result<(), String> {
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

fn parse_cli() -> Result<CliOptions, String> {
    let mut options = CliOptions::default();
    let mut arguments = std::env::args().skip(1).peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--demo" => options.demo = true,
            "--help" | "-h" => options.show_help = true,
            "--version" | "-V" => options.show_version = true,
            "--auto-close-seconds" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| "--auto-close-seconds requires a value".to_string())?;
                options.auto_close_seconds = Some(parse_positive_seconds(&value)?);
            }
            value if value.starts_with("--auto-close-seconds=") => {
                options.auto_close_seconds = Some(parse_positive_seconds(
                    value.trim_start_matches("--auto-close-seconds="),
                )?);
            }
            value => return Err(format!("Unknown option: {value}")),
        }
    }
    Ok(options)
}

fn parse_positive_seconds(value: &str) -> Result<f64, String> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| format!("Invalid number of seconds: {value}"))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("Auto-close seconds must be greater than zero.".into());
    }
    Ok(seconds)
}

fn print_help() {
    println!(
        "Cutting Board {}\n\nUSAGE:\n    cutting-board [OPTIONS]\n\nOPTIONS:\n    --demo                       Show deterministic sample services\n    --auto-close-seconds <N>     Close automatically after N seconds\n    -h, --help                   Print help\n    -V, --version                Print version",
        env!("CARGO_PKG_VERSION")
    );
}

pub fn run() {
    let options = match parse_cli() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}\n\nRun with --help for usage.");
            std::process::exit(2);
        }
    };
    if options.show_help {
        print_help();
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
                settings = migrate_startup_window_settings(&main_window, &settings_path, settings);
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
                last_workspace: Mutex::new(None),
                service_index: Mutex::new(HashMap::new()),
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
                    persist_window_geometry(window, state.inner());
                }
                tauri::WindowEvent::CloseRequested { .. } => {
                    persist_window_geometry(window, state.inner());
                    stop_managed_tasks(state.inner());
                }
                tauri::WindowEvent::Destroyed => stop_managed_tasks(state.inner()),
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
            start_task,
            stop_task,
            stop_profile,
            terminate_service,
            shutdown
        ])
        .build(tauri::generate_context!())
        .expect("error while building Cutting Board")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. }) {
                let state = app.state::<AppState>();
                if let Some(window) = app.get_webview_window("main") {
                    let window = window.as_ref().window();
                    persist_window_geometry(&window, state.inner());
                }
                stop_managed_tasks(state.inner());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_inner_size_in_logical_pixels() {
        let logical_size = PhysicalSize::new(2_160, 1_440).to_logical::<u32>(2.0);

        assert_eq!(logical_size.width, 1_080);
        assert_eq!(logical_size.height, 720);
    }

    #[test]
    fn migrates_unmarked_legacy_physical_window_size() {
        let settings = UiSettings {
            window_width: 2_160,
            window_height: 1_440,
            window_x: Some(40),
            window_y: Some(78),
            ..UiSettings::default()
        };
        let migrated =
            migrate_legacy_window_size(settings, 2.0).expect("legacy physical size should migrate");

        assert_eq!(migrated.window_width, 1_080);
        assert_eq!(migrated.window_height, 720);
    }

    #[test]
    fn does_not_migrate_a_valid_logical_size() {
        let settings = UiSettings {
            window_width: 1_200,
            window_height: 800,
            window_x: Some(40),
            window_y: Some(78),
            window_geometry_logical: true,
            ..UiSettings::default()
        };

        assert!(migrate_legacy_window_size(settings, 2.0).is_none());
    }

    #[test]
    fn parses_auto_close_option() {
        assert_eq!(parse_positive_seconds("0.5").unwrap(), 0.5);
        assert!(parse_positive_seconds("0").is_err());
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
        let state = AppState(Arc::new(AppStateInner {
            demo: false,
            settings_path: PathBuf::new(),
            profiles_path: PathBuf::new(),
            logs_dir: PathBuf::new(),
            last_workspace: Mutex::new(Some(workspace)),
            service_index: Mutex::new(HashMap::new()),
            launch: Mutex::new(LaunchManager::default()),
        }));

        let snapshot = read_service_logs(&state, "service").unwrap();

        assert!(!snapshot.available);
        assert!(snapshot.logs.is_empty());
        assert_eq!(snapshot.source_path, None);
        assert!(snapshot.message.is_some());
    }
}
