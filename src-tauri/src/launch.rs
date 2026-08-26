use crate::models::{
    now_epoch, LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceLogSnapshot, ServiceSnapshot,
    TaskRequest, WorkspaceSnapshot,
};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};
use sysinfo::{Pid, System};

#[cfg(unix)]
use std::{env, ffi::CStr};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(unix)]
use std::time::Instant;

#[derive(Debug)]
struct RuntimeTask {
    child: Option<Child>,
    pid: Option<u32>,
    state: String,
    started_at: Option<u64>,
    message: Option<String>,
    log_path: PathBuf,
}

#[derive(Debug)]
struct ExternalTaskInfo {
    pid: Option<u32>,
    started_at: Option<u64>,
    uid: Option<u32>,
    working_directory: Option<String>,
    log_path: Option<PathBuf>,
    log_tail: String,
}

#[derive(Debug, Default)]
pub struct LaunchManager {
    tasks: HashMap<String, RuntimeTask>,
}

impl LaunchManager {
    pub fn snapshots(
        &mut self,
        profiles: &[LaunchProfile],
        workspace: Option<&WorkspaceSnapshot>,
        logs_dir: &Path,
    ) -> Vec<ManagedTaskSnapshot> {
        self.refresh();
        let mut snapshots = Vec::new();
        for profile in profiles {
            for task in &profile.tasks {
                let key = task_key(&profile.id, &task.name);
                if let Some(runtime) = self
                    .tasks
                    .get(&key)
                    .filter(|runtime| is_active(&runtime.state))
                {
                    snapshots.push(snapshot_from(&profile.id, &task.name, runtime));
                } else if let Some(mut external) = external_task_info(profile, task, workspace) {
                    recover_external_task_log(
                        &mut external,
                        logs_dir,
                        profiles,
                        profile,
                        task,
                        workspace,
                    );
                    snapshots.push(external_snapshot(&profile.id, &task.name, external));
                } else if let Some(runtime) = self.tasks.get(&key) {
                    snapshots.push(snapshot_from(&profile.id, &task.name, runtime));
                } else {
                    snapshots.push(ManagedTaskSnapshot {
                        profile_id: profile.id.clone(),
                        task_name: task.name.clone(),
                        state: "stopped".into(),
                        main_pid: None,
                        started_at: None,
                        message: None,
                        log_tail: String::new(),
                        external_pid: None,
                        external_working_directory: None,
                        external_log_path: None,
                    });
                }
            }
        }
        snapshots
    }

    /// Read output for a discovered service, preferring an active managed task
    /// because child processes can inherit the task shell's log file.
    pub fn service_logs(
        &mut self,
        profiles: &[LaunchProfile],
        service: &ServiceSnapshot,
        logs_dir: &Path,
    ) -> Result<ServiceLogSnapshot, String> {
        self.refresh();

        for profile in profiles {
            for task in &profile.tasks {
                if !task_matches_service(profile, task, service) {
                    continue;
                }
                let Some(runtime) = self.tasks.get(&task_key(&profile.id, &task.name)) else {
                    continue;
                };
                if !is_active(&runtime.state) {
                    continue;
                }
                if !service_belongs_to_runtime(service, runtime.pid) {
                    continue;
                }
                if let Ok(logs) = read_log_tail(&runtime.log_path) {
                    return Ok(ServiceLogSnapshot {
                        logs,
                        source_path: Some(runtime.log_path.to_string_lossy().into_owned()),
                        available: true,
                        message: None,
                    });
                }
            }
        }

        let Some(pid) = service.process.as_ref().map(|process| process.pid) else {
            return Ok(unavailable_service_logs(
                "Process details were unavailable during the last scan.",
            ));
        };
        let start_time = service
            .process
            .as_ref()
            .map(|process| process.create_time)
            .unwrap_or_default();
        ensure_process_identity(pid, start_time)?;
        if let Some(path) = external_log_path(pid) {
            ensure_process_identity(pid, start_time)?;
            match read_log_tail(&path) {
                Ok(logs) => {
                    ensure_process_identity(pid, start_time)?;
                    return Ok(ServiceLogSnapshot {
                        logs,
                        source_path: Some(path.to_string_lossy().into_owned()),
                        available: true,
                        message: None,
                    });
                }
                Err(_) => {
                    return Ok(unavailable_service_logs(
                        "A stdout/stderr log file was found, but it could not be read.",
                    ));
                }
            }
        }

        if let Some(path) = recovered_managed_log_path(logs_dir, profiles, service, start_time) {
            ensure_process_identity(pid, start_time)?;
            match read_log_tail(&path) {
                Ok(logs) => {
                    ensure_process_identity(pid, start_time)?;
                    return Ok(ServiceLogSnapshot {
                        logs,
                        source_path: Some(path.to_string_lossy().into_owned()),
                        available: true,
                        message: None,
                    });
                }
                Err(_) => {
                    return Ok(unavailable_service_logs(
                        "A Cutting Board task log was found, but it could not be read.",
                    ));
                }
            }
        }

        ensure_process_identity(pid, start_time)?;
        Ok(unavailable_service_logs(
            "No readable log output is connected to this service. Terminal and pipe output cannot be recovered after launch.",
        ))
    }

    pub fn start_task(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        logs_dir: &Path,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.refresh();
        let (profile, task) = find_task(profiles, request)?;
        if let Some(external) = external_task_info(profile, task, workspace) {
            return Ok(external_snapshot(&profile.id, &task.name, external));
        }
        let key = task_key(&profile.id, &task.name);
        if let Some(runtime) = self.tasks.get(&key) {
            if is_active(&runtime.state) {
                return Ok(snapshot_from(&profile.id, &task.name, runtime));
            }
        }

        let cwd = resolve_cwd(profile, task)?;
        fs::create_dir_all(logs_dir)
            .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;
        let log_path = logs_dir.join(format!(
            "{}-{}.log",
            safe_name(&profile.id),
            safe_name(&task.name)
        ));
        let mut log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| format!("Could not open {}: {error}", log_path.display()))?;
        writeln!(
            log_file,
            "\n=== Cutting Board start {} · {} ===",
            now_epoch(),
            task.command
        )
        .map_err(|error| format!("Could not write {}: {error}", log_path.display()))?;
        let metadata = serde_json::json!({
            "profile_id": profile.id,
            "task_name": task.name,
            "cwd": cwd.to_string_lossy(),
        });
        writeln!(log_file, "=== Cutting Board task metadata {} ===", metadata)
            .map_err(|error| format!("Could not write {}: {error}", log_path.display()))?;
        let stderr_file = log_file
            .try_clone()
            .map_err(|error| format!("Could not duplicate {}: {error}", log_path.display()))?;

        let mut command = shell_command(&task.command);
        command
            .current_dir(&cwd)
            .env("CUTTING_BOARD_MANAGED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file))
            .stderr(Stdio::from(stderr_file));
        #[cfg(unix)]
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn().map_err(|error| {
            format!(
                "Could not start {} in {}: {error}",
                task.name,
                cwd.display()
            )
        })?;
        let pid = child.id();
        let runtime = RuntimeTask {
            child: Some(child),
            pid: Some(pid),
            state: "starting".into(),
            started_at: Some(now_epoch()),
            message: Some(format!("Started {} as PID {pid}.", task.name)),
            log_path,
        };
        self.tasks.insert(key, runtime);
        thread::sleep(Duration::from_millis(80));
        self.refresh();
        Ok(snapshot_from(
            &profile.id,
            &task.name,
            self.tasks
                .get(&task_key(&profile.id, &task.name))
                .expect("inserted runtime task"),
        ))
    }

    pub fn stop_task(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.refresh();
        let (profile, task) = find_task(profiles, request)?;
        let key = task_key(&profile.id, &task.name);
        if self
            .tasks
            .get(&key)
            .is_some_and(|runtime| is_active(&runtime.state))
        {
            let runtime = self.tasks.get_mut(&key).expect("active runtime task");
            runtime.state = "stopping".into();
            runtime.message = Some(format!("Stopping {}…", task.name));
            terminate_runtime(runtime)?;
            runtime.state = "stopped".into();
            runtime.message = Some(format!("Stopped {}.", task.name));
            return Ok(snapshot_from(&profile.id, &task.name, runtime));
        }

        if let Some(external) = external_task_info(profile, task, workspace) {
            return stop_external_task(&profile.id, &task.name, external);
        }

        self.tasks
            .get(&key)
            .map(|runtime| snapshot_from(&profile.id, &task.name, runtime))
            .ok_or_else(|| format!("{} is not running.", task.name))
    }

    pub fn stop_profile(
        &mut self,
        profiles: &[LaunchProfile],
        profile_id: &str,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<Vec<ManagedTaskSnapshot>, String> {
        let profile = profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| "The launch profile no longer exists.".to_string())?;
        let requests = profile
            .tasks
            .iter()
            .map(|task| TaskRequest {
                profile_id: profile.id.clone(),
                task_name: task.name.clone(),
            })
            .collect::<Vec<_>>();
        let mut snapshots = Vec::new();
        for request in requests {
            let key = task_key(&request.profile_id, &request.task_name);
            if self
                .tasks
                .get(&key)
                .is_some_and(|runtime| is_active(&runtime.state))
                || profile
                    .tasks
                    .iter()
                    .find(|task| task.name == request.task_name)
                    .and_then(|task| external_task_info(profile, task, workspace))
                    .is_some()
            {
                snapshots.push(self.stop_task(profiles, &request, workspace)?);
            }
        }
        Ok(snapshots)
    }

    pub fn profile_is_active(
        &mut self,
        profiles: &[LaunchProfile],
        profile_id: &str,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> bool {
        self.refresh();
        let prefix = format!("{profile_id}\0");
        if self
            .tasks
            .iter()
            .any(|(key, runtime)| key.starts_with(&prefix) && is_active(&runtime.state))
        {
            return true;
        }
        let Some(profile) = profiles.iter().find(|profile| profile.id == profile_id) else {
            return false;
        };
        profile
            .tasks
            .iter()
            .any(|task| external_task_info(profile, task, workspace).is_some())
    }

    pub fn stop_all(&mut self) {
        self.refresh();
        for runtime in self.tasks.values_mut() {
            if is_active(&runtime.state) {
                runtime.state = "stopping".into();
                let _ = terminate_runtime(runtime);
                runtime.state = "stopped".into();
                runtime.message = Some("Stopped while Cutting Board closed.".into());
            }
        }
    }

    fn refresh(&mut self) {
        for runtime in self.tasks.values_mut() {
            let Some(child) = runtime.child.as_mut() else {
                continue;
            };
            match child.try_wait() {
                Ok(Some(status)) => {
                    runtime.child = None;
                    runtime.pid = None;
                    if status.success() {
                        runtime.state = "stopped".into();
                        runtime.message = Some(format!("Process exited with {status}."));
                    } else {
                        runtime.state = "failed".into();
                        runtime.message = Some(format!("Process exited with {status}."));
                    }
                }
                Ok(None) => {
                    if runtime.state == "starting" {
                        runtime.state = "running".into();
                    }
                }
                Err(error) => {
                    runtime.state = "failed".into();
                    runtime.message =
                        Some(format!("Could not inspect the managed process: {error}"));
                    runtime.child = None;
                    runtime.pid = None;
                }
            }
        }
    }
}

fn find_task<'a>(
    profiles: &'a [LaunchProfile],
    request: &TaskRequest,
) -> Result<(&'a LaunchProfile, &'a LaunchTask), String> {
    let profile = profiles
        .iter()
        .find(|profile| profile.id == request.profile_id)
        .ok_or_else(|| "The launch profile no longer exists.".to_string())?;
    let task = profile
        .tasks
        .iter()
        .find(|task| task.name == request.task_name)
        .ok_or_else(|| "The launch task no longer exists.".to_string())?;
    Ok((profile, task))
}

fn resolve_cwd(profile: &LaunchProfile, task: &LaunchTask) -> Result<PathBuf, String> {
    let resolved = task_cwd(profile, task);
    if !resolved.is_dir() {
        return Err(format!(
            "The task directory does not exist: {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn task_cwd(profile: &LaunchProfile, task: &LaunchTask) -> PathBuf {
    let cwd = PathBuf::from(task.cwd.trim());
    if cwd.is_absolute() {
        cwd
    } else {
        Path::new(&profile.project_root).join(cwd)
    }
}

fn shell_command(value: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C", value]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(configured_shell());
        command.args(["-l", "-i", "-c", value]);
        command
    }
}

#[cfg(unix)]
fn configured_shell() -> PathBuf {
    let shell = env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| is_usable_shell(path))
        .or_else(login_shell_from_passwd);
    shell_path_or_fallback(shell)
}

#[cfg(unix)]
fn login_shell_from_passwd() -> Option<PathBuf> {
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    if passwd.is_null() || unsafe { (*passwd).pw_shell.is_null() } {
        return None;
    }
    let shell = unsafe { CStr::from_ptr((*passwd).pw_shell) }
        .to_string_lossy()
        .into_owned();
    (!shell.is_empty()).then(|| PathBuf::from(shell))
}

#[cfg(unix)]
fn shell_path_or_fallback(candidate: Option<PathBuf>) -> PathBuf {
    candidate
        .filter(|path| is_usable_shell(path))
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

#[cfg(unix)]
fn is_usable_shell(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
}

fn terminate_runtime(runtime: &mut RuntimeTask) -> Result<(), String> {
    let Some(pid) = runtime.pid else {
        runtime.child = None;
        return Ok(());
    };
    #[cfg(unix)]
    unsafe {
        if libc::kill(-(pid as i32), libc::SIGTERM) == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(format!("Could not signal process group {pid}: {error}"));
            }
        }
    }
    #[cfg(not(unix))]
    if let Some(child) = runtime.child.as_mut() {
        child
            .kill()
            .map_err(|error| format!("Could not stop PID {pid}: {error}"))?;
    }

    for _ in 0..25 {
        if let Some(child) = runtime.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    runtime.child = None;
                    runtime.pid = None;
                    return Ok(());
                }
                Ok(None) => {}
                Err(error) => return Err(format!("Could not wait for PID {pid}: {error}")),
            }
        } else {
            runtime.pid = None;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(80));
    }

    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    if let Some(child) = runtime.child.as_mut() {
        let _ = child.kill();
    }
    if let Some(child) = runtime.child.as_mut() {
        let _ = child.wait();
    }
    runtime.child = None;
    runtime.pid = None;
    Ok(())
}

fn stop_external_task(
    profile_id: &str,
    task_name: &str,
    external: ExternalTaskInfo,
) -> Result<ManagedTaskSnapshot, String> {
    let pid = external
        .pid
        .ok_or_else(|| format!("{task_name} has no current process identity."))?;
    let started_at = external
        .started_at
        .ok_or_else(|| format!("{task_name} has no current process start time."))?;
    validate_external_process_identity(pid, started_at, external.uid)?;

    send_external_signal(pid, external_term_signal())?;
    for _ in 0..25 {
        thread::sleep(Duration::from_millis(80));
        if !external_process_is_current(pid, started_at)? {
            return Ok(external_stopped_snapshot(
                profile_id,
                task_name,
                started_at,
                external.log_tail,
            ));
        }
    }

    // Revalidate immediately before escalating. A reused PID must never receive
    // a signal belonging to the previous launch task.
    validate_external_process_identity(pid, started_at, external.uid)?;
    send_external_signal(pid, external_kill_signal())?;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(60));
        if !external_process_is_current(pid, started_at)? {
            return Ok(external_stopped_snapshot(
                profile_id,
                task_name,
                started_at,
                external.log_tail,
            ));
        }
    }

    Err(format!("{task_name} did not stop."))
}

fn external_stopped_snapshot(
    profile_id: &str,
    task_name: &str,
    started_at: u64,
    log_tail: String,
) -> ManagedTaskSnapshot {
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: "stopped".into(),
        main_pid: None,
        started_at: Some(started_at),
        message: Some(format!("Stopped {task_name}.")),
        log_tail,
        external_pid: None,
        external_working_directory: None,
        external_log_path: None,
    }
}

fn validate_external_process_identity(
    pid: u32,
    started_at: u64,
    uid: Option<u32>,
) -> Result<(), String> {
    if pid <= 1 || pid == std::process::id() {
        return Err("Cutting Board refused to stop that process.".into());
    }
    let current_uid = effective_uid();
    if uid.is_some() && current_uid.is_some() && uid != current_uid {
        return Err("Cutting Board only stops processes owned by the current user.".into());
    }
    let system = System::new_all();
    let process = system
        .process(Pid::from_u32(pid))
        .ok_or_else(|| "The process already exited. Refresh and try again.".to_string())?;
    if process.start_time() != started_at {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }
    Ok(())
}

fn external_process_is_current(pid: u32, started_at: u64) -> Result<bool, String> {
    let system = System::new_all();
    let Some(process) = system.process(Pid::from_u32(pid)) else {
        return Ok(false);
    };
    if process.start_time() != started_at {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }
    Ok(true)
}

#[cfg(unix)]
fn external_term_signal() -> i32 {
    libc::SIGTERM
}

#[cfg(not(unix))]
fn external_term_signal() {}

#[cfg(unix)]
fn external_kill_signal() -> i32 {
    libc::SIGKILL
}

#[cfg(not(unix))]
fn external_kill_signal() {}

#[cfg(unix)]
fn send_external_signal(pid: u32, signal: i32) -> Result<(), String> {
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!("Could not signal PID {pid}: {error}"))
    }
}

#[cfg(not(unix))]
fn send_external_signal(pid: u32, _signal: ()) -> Result<(), String> {
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

#[cfg(unix)]
fn effective_uid() -> Option<u32> {
    Some(unsafe { libc::geteuid() })
}

#[cfg(not(unix))]
fn effective_uid() -> Option<u32> {
    None
}

fn snapshot_from(profile_id: &str, task_name: &str, runtime: &RuntimeTask) -> ManagedTaskSnapshot {
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: runtime.state.clone(),
        main_pid: runtime.pid,
        started_at: runtime.started_at,
        message: runtime.message.clone(),
        log_tail: read_log_tail(&runtime.log_path).unwrap_or_default(),
        external_pid: None,
        external_working_directory: None,
        external_log_path: None,
    }
}

fn external_snapshot(
    profile_id: &str,
    task_name: &str,
    external: ExternalTaskInfo,
) -> ManagedTaskSnapshot {
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: "running".into(),
        main_pid: external.pid,
        started_at: external.started_at,
        message: None,
        log_tail: external.log_tail,
        external_pid: external.pid,
        external_working_directory: external.working_directory,
        external_log_path: external
            .log_path
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

fn recover_external_task_log(
    external: &mut ExternalTaskInfo,
    logs_dir: &Path,
    profiles: &[LaunchProfile],
    profile: &LaunchProfile,
    task: &LaunchTask,
    workspace: Option<&WorkspaceSnapshot>,
) {
    if external.log_path.is_some() {
        return;
    }
    let Some(process_start) = external.started_at else {
        return;
    };
    let Some(service) = workspace.and_then(|snapshot| {
        snapshot
            .services
            .iter()
            .find(|service| task_matches_service(profile, task, service))
    }) else {
        return;
    };
    let Some(path) = recovered_managed_log_path(logs_dir, profiles, service, process_start) else {
        return;
    };
    let Ok(logs) = read_log_tail(&path) else {
        return;
    };
    external.log_path = Some(path);
    external.log_tail = logs;
}

fn read_log_tail(path: &Path) -> io::Result<String> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

const LOG_RECOVERY_START_SKEW_SECS: u64 = 15 * 60;
const LOG_MARKER_SCAN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct LogStartMarker {
    started_at: u64,
    command: String,
    profile_id: Option<String>,
    task_name: Option<String>,
    cwd: Option<String>,
}

fn recovered_managed_log_path(
    logs_dir: &Path,
    profiles: &[LaunchProfile],
    service: &ServiceSnapshot,
    process_start: u64,
) -> Option<PathBuf> {
    let mut matches = Vec::new();
    let process_commands = process_command_provenance(service);
    for profile in profiles {
        for task in &profile.tasks {
            if !task_matches_service(profile, task, service) {
                continue;
            }
            let expected_name = format!("{}-{}.log", safe_name(&profile.id), safe_name(&task.name));
            let suffix = format!("-{}.log", safe_name(&task.name));
            let Ok(entries) = fs::read_dir(logs_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(_) => continue,
                };
                if !file_type.is_file() {
                    continue;
                }
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name != expected_name && !file_name.ends_with(&suffix) {
                    continue;
                }
                let path = entry.path();
                let Ok(markers) = read_log_start_markers(&path) else {
                    continue;
                };
                let Some((rank, distance)) = markers
                    .iter()
                    .filter_map(|marker| {
                        log_marker_match_score(
                            marker,
                            profile,
                            task,
                            service,
                            &process_commands,
                            process_start,
                            file_name == expected_name,
                        )
                    })
                    .min()
                else {
                    continue;
                };
                matches.push((rank, distance, path, profile.id.clone(), task.name.clone()));
            }
        }
    }

    matches.sort_by(|left, right| {
        (left.0, left.1, &left.2, &left.3, &left.4)
            .cmp(&(right.0, right.1, &right.2, &right.3, &right.4))
    });
    let best = matches.first()?;
    if matches
        .get(1)
        .is_some_and(|candidate| (candidate.0, candidate.1) == (best.0, best.1))
    {
        return None;
    }
    Some(best.2.clone())
}

fn read_log_start_markers(path: &Path) -> io::Result<Vec<LogStartMarker>> {
    let file = OpenOptions::new().read(true).open(path)?;
    let file_length = file.metadata()?.len();
    let scan_start = file_length.saturating_sub(LOG_MARKER_SCAN_BYTES);
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(scan_start))?;
    let mut bytes_read: u64 = 0;
    if scan_start > 0 {
        reader.seek(SeekFrom::Start(scan_start - 1))?;
        let mut previous = [0_u8; 1];
        reader.read_exact(&mut previous)?;
        reader.seek(SeekFrom::Start(scan_start))?;
        if previous[0] != b'\n' {
            let mut partial_line = Vec::new();
            bytes_read =
                bytes_read.saturating_add(reader.read_until(b'\n', &mut partial_line)? as u64);
        }
    }
    let mut line = String::new();
    let mut markers = Vec::new();
    while bytes_read < LOG_MARKER_SCAN_BYTES {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(bytes as u64);
        if let Some(marker) = parse_log_start_marker(&line) {
            markers.push(marker);
        } else if let Some(metadata) = parse_log_metadata(&line) {
            if let Some(marker) = markers.last_mut() {
                marker.profile_id = metadata.0;
                marker.task_name = metadata.1;
                marker.cwd = metadata.2;
            }
        }
    }
    Ok(markers)
}

fn parse_log_start_marker(line: &str) -> Option<LogStartMarker> {
    let body = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("=== Cutting Board start ")?
        .strip_suffix(" ===")?;
    let (started_at, command) = body.split_once(" · ")?;
    Some(LogStartMarker {
        started_at: started_at.parse().ok()?,
        command: command.into(),
        profile_id: None,
        task_name: None,
        cwd: None,
    })
}

fn parse_log_metadata(line: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let json = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("=== Cutting Board task metadata ")?
        .strip_suffix(" ===")?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    Some((
        value.get("profile_id")?.as_str().map(str::to_owned),
        value.get("task_name")?.as_str().map(str::to_owned),
        value.get("cwd")?.as_str().map(str::to_owned),
    ))
}

fn log_marker_match_score(
    marker: &LogStartMarker,
    profile: &LaunchProfile,
    task: &LaunchTask,
    service: &ServiceSnapshot,
    process_commands: &[String],
    process_start: u64,
    expected_path: bool,
) -> Option<(u8, u64)> {
    if marker
        .task_name
        .as_deref()
        .is_some_and(|task_name| task_name != task.name)
    {
        return None;
    }
    let task_root = task_cwd(profile, task)
        .canonicalize()
        .unwrap_or_else(|_| task_cwd(profile, task));
    if marker
        .cwd
        .as_deref()
        .is_some_and(|cwd| !path_matches_task(cwd, &task_root))
    {
        return None;
    }

    let distance = marker.started_at.abs_diff(process_start);
    let command_matches = marker.command == task.command
        || service
            .process
            .as_ref()
            .and_then(|process| process.launch_command.as_deref())
            .is_some_and(|command| marker.command == command)
        || process_commands
            .iter()
            .any(|command| commands_match(marker.command.as_str(), command));
    let metadata_matches = marker
        .profile_id
        .as_deref()
        .is_some_and(|profile_id| profile_id == profile.id)
        && marker.task_name.as_deref() == Some(task.name.as_str())
        && marker
            .cwd
            .as_deref()
            .is_some_and(|cwd| path_matches_task(cwd, &task_root));

    if distance > LOG_RECOVERY_START_SKEW_SECS && !metadata_matches {
        return None;
    }

    let rank = if metadata_matches {
        0
    } else if command_matches {
        1
    } else if expected_path {
        2
    } else {
        3
    };
    Some((rank, distance))
}

fn process_command_provenance(service: &ServiceSnapshot) -> Vec<String> {
    let Some(process) = service.process.as_ref() else {
        return Vec::new();
    };
    let system = System::new_all();
    let mut pid = Some(Pid::from_u32(process.pid));
    let mut visited = Vec::new();
    let mut commands = Vec::new();
    while let Some(current_pid) = pid {
        if visited.contains(&current_pid) {
            break;
        }
        visited.push(current_pid);
        let Some(current) = system.process(current_pid) else {
            break;
        };
        let command = current
            .cmd()
            .iter()
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        if !command.is_empty() {
            commands.push(command);
        }
        pid = current.parent();
    }
    commands
}

fn commands_match(marker: &str, process: &str) -> bool {
    let marker = marker.trim();
    let process = process.trim();
    if marker.is_empty() || process.is_empty() {
        return false;
    }
    marker == process
        || (marker.len() >= 16 && process.contains(marker))
        || (process.len() >= 16 && marker.contains(process))
}

fn ensure_process_identity(pid: u32, start_time: u64) -> Result<(), String> {
    let system = System::new_all();
    let process = system.process(Pid::from_u32(pid)).ok_or_else(|| {
        "The process changed since the last scan. Refresh and try again.".to_string()
    })?;
    if process.start_time() != start_time {
        return Err(
            "The PID was reused by another process. Refresh before reading its logs.".into(),
        );
    }
    Ok(())
}

fn external_task_info(
    profile: &LaunchProfile,
    task: &LaunchTask,
    workspace: Option<&WorkspaceSnapshot>,
) -> Option<ExternalTaskInfo> {
    let service = workspace.and_then(|snapshot| {
        snapshot
            .services
            .iter()
            .find(|service| task_matches_service(profile, task, service))
    })?;
    let process = service.process.as_ref()?;
    let pid = Some(process.pid);
    let started_at = Some(process.create_time);
    let uid = process.uid;
    let working_directory = process.working_directory.clone();
    let (log_path, log_tail) = external_log_path(process.pid)
        .and_then(|path| read_log_tail(&path).ok().map(|tail| (Some(path), tail)))
        .unwrap_or_else(|| (None, String::new()));
    Some(ExternalTaskInfo {
        pid,
        started_at,
        uid,
        working_directory,
        log_path,
        log_tail,
    })
}

fn task_matches_service(
    profile: &LaunchProfile,
    task: &LaunchTask,
    service: &ServiceSnapshot,
) -> bool {
    let Some(port) = task.expected_port else {
        return false;
    };
    if !service
        .endpoints
        .iter()
        .any(|endpoint| endpoint.port == port)
    {
        return false;
    }
    let task_root = task_cwd(profile, task)
        .canonicalize()
        .unwrap_or_else(|_| task_cwd(profile, task));
    service
        .process
        .as_ref()
        .and_then(|process| process.working_directory.as_deref())
        .is_some_and(|path| path_matches_task(path, &task_root))
        || service.project.as_ref().is_some_and(|project| {
            [
                project.root_path.as_str(),
                project.workspace_root_path.as_str(),
            ]
            .into_iter()
            .any(|path| path_matches_task(path, &task_root))
        })
}

fn service_belongs_to_runtime(service: &ServiceSnapshot, runtime_pid: Option<u32>) -> bool {
    let Some(process) = service.process.as_ref() else {
        return false;
    };
    let Some(runtime_pid) = runtime_pid else {
        return true;
    };
    if process.pid == runtime_pid {
        return true;
    }

    let system = System::new_all();
    let mut parent_pid = process.parent_pid;
    let mut visited = Vec::new();
    while let Some(pid) = parent_pid {
        if pid == runtime_pid {
            return true;
        }
        if visited.contains(&pid) {
            return false;
        }
        visited.push(pid);
        parent_pid = system
            .process(Pid::from_u32(pid))
            .and_then(|process| process.parent())
            .map(|pid| pid.as_u32());
    }
    false
}

fn unavailable_service_logs(message: &str) -> ServiceLogSnapshot {
    ServiceLogSnapshot {
        logs: String::new(),
        source_path: None,
        available: false,
        message: Some(message.into()),
    }
}

#[cfg(unix)]
fn external_log_path(pid: u32) -> Option<PathBuf> {
    const TIMEOUT: Duration = Duration::from_secs(2);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    let mut child = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-d", "1,2", "-Ffn"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    first_readable_external_log_path(parse_lsof_output_paths(&text))
}

#[cfg(not(unix))]
fn external_log_path(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn parse_lsof_output_paths(text: &str) -> Vec<(u8, PathBuf)> {
    let mut fd = None;
    let mut paths = Vec::new();
    for line in text.lines() {
        let Some((field, value)) = line.split_at_checked(1) else {
            continue;
        };
        match field {
            "f" => fd = parse_lsof_fd(value),
            "n" if matches!(fd, Some(1 | 2)) => {
                paths.push((fd.expect("checked above"), PathBuf::from(value)));
            }
            _ => {}
        }
    }
    paths
}

#[cfg(unix)]
fn parse_lsof_fd(value: &str) -> Option<u8> {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    matches!(digits.as_str(), "1" | "2").then(|| digits.parse().expect("known fd"))
}

fn first_readable_external_log_path(mut paths: Vec<(u8, PathBuf)>) -> Option<PathBuf> {
    paths.sort_by_key(|(fd, _)| *fd);
    let mut seen = Vec::new();
    for (_, path) in paths {
        let comparison_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.iter().any(|candidate| candidate == &comparison_path) {
            continue;
        }
        seen.push(comparison_path);
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if OpenOptions::new().read(true).open(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

fn path_matches_task(candidate: &str, task_root: &Path) -> bool {
    if candidate.trim().is_empty() {
        return false;
    }
    let candidate = Path::new(candidate)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(candidate));
    candidate == task_root || candidate.starts_with(task_root) || task_root.starts_with(&candidate)
}

fn task_key(profile_id: &str, task_name: &str) -> String {
    format!("{profile_id}\0{task_name}")
}

fn is_active(state: &str) -> bool {
    matches!(state, "starting" | "running" | "stopping")
}

fn safe_name(value: &str) -> String {
    let safe = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe.is_empty() {
        "task".into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServiceSnapshot;

    fn workspace_with_project(port: u16, project_root: &Path) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            services: vec![ServiceSnapshot {
                id: "service".into(),
                display_name: "frontend".into(),
                tech: "vite".into(),
                category: "web".into(),
                relevance: "dev".into(),
                endpoints: vec![crate::models::Endpoint {
                    family: "IPv4".into(),
                    address: "127.0.0.1".into(),
                    port,
                    scope: "loopback".into(),
                    protocol: "TCP".into(),
                }],
                process: None,
                project: Some(crate::models::ProjectInfo {
                    id: "project".into(),
                    name: "frontend".into(),
                    root_path: project_root.to_string_lossy().into_owned(),
                    detection_source: "package.json".into(),
                    workspace_root_path: project_root.to_string_lossy().into_owned(),
                    workspace_name: "frontend".into(),
                }),
                status: "healthy".into(),
                warnings: vec![],
                origin_kind: "terminal".into(),
                origin_label: None,
                can_terminate: false,
                browser_url: None,
                active_profiles: vec![],
            }],
            scanned_at: 0,
            scan_duration_ms: 0,
            endpoint_count: 1,
            errors: vec![],
        }
    }

    fn managed_service_log_fixture(
        contents: &str,
    ) -> (
        tempfile::TempDir,
        LaunchProfile,
        WorkspaceSnapshot,
        LaunchManager,
    ) {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let log_path = temporary.path().join("frontend.log");
        fs::write(&log_path, contents).unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let mut workspace = workspace_with_project(5173, &frontend);
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid: 123,
            parent_pid: None,
            name: "node".into(),
            executable: None,
            working_directory: Some(frontend.to_string_lossy().into_owned()),
            command: "node vite".into(),
            launch_command: None,
            create_time: 0,
            uptime_seconds: 0,
            cpu_percent: None,
            memory_bytes: None,
            uid: None,
        });
        let manager = LaunchManager {
            tasks: HashMap::from([(
                task_key("profile", "frontend"),
                RuntimeTask {
                    child: None,
                    pid: Some(123),
                    state: "running".into(),
                    started_at: Some(1),
                    message: None,
                    log_path,
                },
            )]),
        };
        (temporary, profile, workspace, manager)
    }

    #[test]
    fn relative_task_directory_is_anchored_to_project() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("frontend")).unwrap();
        let profile = LaunchProfile {
            id: "p".into(),
            name: "P".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![],
        };
        let task = LaunchTask {
            name: "web".into(),
            cwd: "frontend".into(),
            command: "echo ok".into(),
            expected_port: None,
        };
        assert_eq!(
            resolve_cwd(&profile, &task).unwrap(),
            temporary.path().join("frontend")
        );
    }

    #[test]
    fn task_without_process_is_not_marked_running() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);

        let snapshots =
            LaunchManager::default().snapshots(&[profile], Some(&workspace), temporary.path());

        assert_eq!(snapshots[0].state, "stopped");
    }

    #[test]
    fn task_matches_service_working_directory_when_project_root_is_coarser() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let mut workspace = workspace_with_project(5173, temporary.path());
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid: 123,
            parent_pid: None,
            name: "node".into(),
            executable: None,
            working_directory: Some(frontend.to_string_lossy().into_owned()),
            command: "node vite".into(),
            launch_command: None,
            create_time: 0,
            uptime_seconds: 0,
            cpu_percent: None,
            memory_bytes: None,
            uid: None,
        });

        let snapshots =
            LaunchManager::default().snapshots(&[profile], Some(&workspace), temporary.path());

        assert_eq!(snapshots[0].state, "running");
        assert_eq!(snapshots[0].main_pid, Some(123));
        assert_eq!(snapshots[0].started_at, Some(0));
        assert_eq!(snapshots[0].external_pid, Some(123));
        assert_eq!(
            snapshots[0].external_working_directory.as_deref(),
            Some(frontend.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn externally_running_task_counts_as_active_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let mut workspace = workspace_with_project(5173, &frontend);
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid: 123,
            parent_pid: None,
            name: "node".into(),
            executable: None,
            working_directory: Some(frontend.to_string_lossy().into_owned()),
            command: "node vite".into(),
            launch_command: None,
            create_time: 1,
            uptime_seconds: 1,
            cpu_percent: None,
            memory_bytes: None,
            uid: None,
        });

        assert!(LaunchManager::default().profile_is_active(
            &[profile],
            "profile",
            Some(&workspace)
        ));
    }

    #[test]
    fn managed_task_remains_running_when_matching_external_listener_exists() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        let mut manager = LaunchManager {
            tasks: HashMap::from([(
                task_key("profile", "frontend"),
                RuntimeTask {
                    child: None,
                    pid: Some(42),
                    state: "running".into(),
                    started_at: Some(1),
                    message: None,
                    log_path: temporary.path().join("frontend.log"),
                },
            )]),
        };

        let snapshots = manager.snapshots(&[profile], Some(&workspace), temporary.path());

        assert_eq!(snapshots[0].state, "running");
        assert_eq!(snapshots[0].main_pid, Some(42));
    }

    #[test]
    fn stopped_runtime_record_does_not_hide_matching_external_service() {
        let (temporary, profile, workspace, mut manager) =
            managed_service_log_fixture("managed output\n");
        let runtime = manager
            .tasks
            .get_mut(&task_key("profile", "frontend"))
            .unwrap();
        runtime.child = None;
        runtime.pid = None;
        runtime.state = "stopped".into();

        let snapshots = manager.snapshots(
            std::slice::from_ref(&profile),
            Some(&workspace),
            temporary.path(),
        );

        assert_eq!(snapshots[0].state, "running");
        assert_eq!(snapshots[0].external_pid, Some(123));
    }

    #[test]
    fn service_logs_prefers_matching_active_task_log() {
        let (_temporary, profile, workspace, mut manager) =
            managed_service_log_fixture("managed output\n");

        let snapshot = manager.service_logs(&[profile], &workspace.services[0], _temporary.path());
        let snapshot = snapshot.unwrap();

        assert!(snapshot.available);
        assert_eq!(snapshot.logs, "managed output\n");
        assert!(snapshot.source_path.is_some());
        assert_eq!(snapshot.message, None);
    }

    #[test]
    fn service_logs_marks_empty_managed_file_as_available() {
        let (_temporary, profile, workspace, mut manager) = managed_service_log_fixture("");

        let snapshot = manager.service_logs(&[profile], &workspace.services[0], _temporary.path());
        let snapshot = snapshot.unwrap();

        assert!(snapshot.available);
        assert!(snapshot.logs.is_empty());
        assert!(snapshot.source_path.is_some());
        assert_eq!(snapshot.message, None);
    }

    #[test]
    fn managed_task_ownership_requires_runtime_pid_or_descendant() {
        let (_temporary, _profile, workspace, _manager) =
            managed_service_log_fixture("managed output\n");
        let service = &workspace.services[0];

        assert!(service_belongs_to_runtime(service, Some(123)));
        assert!(!service_belongs_to_runtime(service, Some(42)));
        assert!(service_belongs_to_runtime(service, None));
    }

    #[test]
    fn recovered_log_matches_legacy_profile_by_task_and_start_time() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        let logs_dir = temporary.path().join("logs");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&logs_dir).unwrap();
        let profile = LaunchProfile {
            id: "current-profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        fs::write(
            logs_dir.join("legacy-profile-frontend.log"),
            format!(
                "=== Cutting Board start 1000 · npm run dev ===\n=== Cutting Board task metadata {} ===\noutput\n",
                serde_json::json!({
                    "profile_id": "legacy-profile",
                    "task_name": "frontend",
                    "cwd": frontend.to_string_lossy(),
                })
            ),
        )
        .unwrap();

        let path = recovered_managed_log_path(&logs_dir, &[profile], &workspace.services[0], 1005);

        assert_eq!(path, Some(logs_dir.join("legacy-profile-frontend.log")));
    }

    #[test]
    fn recovered_log_scans_recent_window_for_appended_current_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        let logs_dir = temporary.path().join("logs");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&logs_dir).unwrap();
        let profile = LaunchProfile {
            id: "current-profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        let metadata = serde_json::json!({
            "profile_id": "current-profile",
            "task_name": "frontend",
            "cwd": frontend.to_string_lossy(),
        });
        let mut contents = "legacy output\n"
            .repeat((LOG_MARKER_SCAN_BYTES as usize / "legacy output\n".len()) + 2_048);
        contents.push_str(&format!(
            "=== Cutting Board start 1005 · npm run dev ===\n=== Cutting Board task metadata {metadata} ===\ncurrent output\n"
        ));
        let path = logs_dir.join("legacy-profile-frontend.log");
        fs::write(&path, contents).unwrap();

        assert_eq!(
            recovered_managed_log_path(&logs_dir, &[profile], &workspace.services[0], 1005),
            Some(path)
        );
    }

    #[test]
    fn recovered_log_rejects_unrelated_stale_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        let logs_dir = temporary.path().join("logs");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&logs_dir).unwrap();
        let profile = LaunchProfile {
            id: "current-profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        fs::write(
            logs_dir.join("legacy-profile-frontend.log"),
            "=== Cutting Board start 1 · an unrelated command ===\noutput\n",
        )
        .unwrap();

        let path =
            recovered_managed_log_path(&logs_dir, &[profile], &workspace.services[0], 10_000);

        assert_eq!(path, None);
    }

    #[test]
    fn recovered_log_prefers_current_start_over_old_matching_command() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        let logs_dir = temporary.path().join("logs");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&logs_dir).unwrap();
        let profile = LaunchProfile {
            id: "current-profile".into(),
            name: "dutypark".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        fs::write(
            logs_dir.join("old-profile-frontend.log"),
            "=== Cutting Board start 1 · npm run dev ===\nold output\n",
        )
        .unwrap();
        fs::write(
            logs_dir.join("legacy-profile-frontend.log"),
            "=== Cutting Board start 1000 · a wrapper command ===\ncurrent output\n",
        )
        .unwrap();

        let path = recovered_managed_log_path(&logs_dir, &[profile], &workspace.services[0], 1005);

        assert_eq!(path, Some(logs_dir.join("legacy-profile-frontend.log")));
    }

    #[test]
    fn task_key_separates_profile_and_name() {
        assert_ne!(task_key("ab", "c"), task_key("a", "bc"));
    }

    #[cfg(unix)]
    #[test]
    fn external_log_path_prefers_stdout_and_skips_non_regular_files() {
        let temporary = tempfile::tempdir().unwrap();
        let stdout = temporary.path().join("stdout.log");
        let stderr = temporary.path().join("stderr.log");
        fs::write(&stdout, "stdout").unwrap();
        fs::write(&stderr, "stderr").unwrap();
        let output = format!(
            "f2w\nn{}\nf1w\nn{}\nf2w\nn{}\n",
            temporary.path().display(),
            stdout.display(),
            stderr.display()
        );

        let paths = parse_lsof_output_paths(&output);
        assert_eq!(first_readable_external_log_path(paths), Some(stdout));
        assert_eq!(
            first_readable_external_log_path(vec![
                (1, temporary.path().to_path_buf()),
                (2, stderr.clone()),
            ]),
            Some(stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn lsof_fd_parser_does_not_treat_fd_ten_as_stdout() {
        assert_eq!(parse_lsof_fd("10w"), None);
        assert_eq!(parse_lsof_fd("1w"), Some(1));
        assert_eq!(parse_lsof_fd("2r"), Some(2));
    }

    #[cfg(unix)]
    #[test]
    fn managed_shell_is_login_interactive() {
        let command = shell_command("echo ok");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, vec!["-l", "-i", "-c", "echo ok"]);
    }

    #[cfg(unix)]
    #[test]
    fn invalid_configured_shell_uses_safe_fallback() {
        assert_eq!(
            shell_path_or_fallback(Some(PathBuf::from("/definitely/missing/shell"))),
            PathBuf::from("/bin/sh")
        );
    }
}
