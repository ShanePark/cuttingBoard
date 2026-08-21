mod docker;
mod launch;
mod models;
mod scanner;
mod storage;

use crate::{
    launch::LaunchManager,
    models::{
        AppInfo, ContainerListing, LaunchProfile, ManagedTaskSnapshot, ServiceIdentity,
        TaskRequest, TerminateRequest, TerminationResult, UiSettings, WorkspaceSnapshot,
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
use tauri::{Manager, State};

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
fn load_settings(state: State<'_, AppState>) -> Result<UiSettings, String> {
    read_settings(&state.0.settings_path)
}

#[tauri::command]
fn save_settings(
    state: State<'_, AppState>,
    settings: UiSettings,
) -> Result<UiSettings, String> {
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
    if lock(&state.0.launch)?.profile_is_active(&profile.id) {
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
    if lock(&state.0.launch)?.profile_is_active(&profile_id) {
        return Err("Stop every task in this profile before deleting it.".into());
    }
    remove_profile(&state.0.profiles_path, &profile_id)
}

#[tauri::command]
fn task_snapshots(state: State<'_, AppState>) -> Result<Vec<ManagedTaskSnapshot>, String> {
    let profiles = profiles_for_state(&state)?;
    let workspace = lock(&state.0.last_workspace)?.clone();
    Ok(lock(&state.0.launch)?.snapshots(&profiles, workspace.as_ref()))
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
        lock(&state.0.launch)?.stop_task(&profiles, &request)
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
        lock(&state.0.launch)?.stop_profile(&profiles, &profile_id)
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

fn terminate_discovered_service(
    state: &AppState,
    request: &TerminateRequest,
) -> Result<TerminationResult, String> {
    let identity = lock(&state.0.service_index)?
        .get(&request.service_id)
        .cloned()
        .ok_or_else(|| "The service changed since the last scan. Refresh and try again.".to_string())?;
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
                message: format!("Forced {} to stop after it ignored SIGTERM.", identity.display_name),
            });
        }
    }
    Ok(TerminationResult {
        success: false,
        message: format!("{} did not stop.", identity.display_name),
    })
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
    mutex.lock().map_err(|_| "Internal state lock was poisoned.".into())
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
            let config_dir = app
                .path()
                .app_config_dir()
                .map_err(|error| format!("Could not resolve the app configuration directory: {error}"))?;
            let settings_path = config_dir.join("settings.json");
            let profiles_path = config_dir.join("launch-profiles.json");
            let logs_dir = config_dir.join("logs");
            let settings = read_settings(&settings_path).unwrap_or_default();
            let window_icon = app.default_window_icon().cloned();
            if let Some(window) = app.get_webview_window("main") {
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
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let state = window.app_handle().state::<AppState>();
                if let Ok(mut launch) = state.0.launch.lock() {
                    launch.stop_all();
                };
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            scan_workspace,
            list_containers,
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
        .run(tauri::generate_context!())
        .expect("error while running Cutting Board");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auto_close_option() {
        assert_eq!(parse_positive_seconds("0.5").unwrap(), 0.5);
        assert!(parse_positive_seconds("0").is_err());
    }
}
