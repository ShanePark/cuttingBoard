use crate::models::{
    now_epoch, LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceLogSnapshot, ServiceSnapshot,
    TaskRequest, WorkspaceSnapshot,
};
pub mod containers;
mod external;
mod logs;
mod shell;

use external::{
    external_log_path, external_snapshot, external_task_info, service_belongs_to_runtime,
    stop_external_task, task_matches_service,
};
use logs::{read_log_tail, recover_external_task_log, recovered_managed_log_path, safe_name};
use shell::shell_command;

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[derive(Debug)]
struct RuntimeTask {
    child: Option<Child>,
    pid: Option<u32>,
    state: String,
    started_at: Option<u64>,
    message: Option<String>,
    log_path: PathBuf,
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
                // A container task is answered by Docker, so the manager never speaks for it.
                if task.container_name().is_some() {
                    continue;
                }
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

    /// Restart a task that is currently running either under Cutting Board or
    /// as a strictly matched external launch task.
    ///
    /// The workspace snapshot can still contain the process that was just
    /// stopped. Starting with no workspace intentionally bypasses that stale
    /// external-process check and makes the new process a managed task.
    pub fn restart_task(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        logs_dir: &Path,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.refresh();
        let (profile, task) = find_task(profiles, request)?;
        let key = task_key(&profile.id, &task.name);
        let managed_active = self
            .tasks
            .get(&key)
            .is_some_and(|runtime| is_active(&runtime.state));
        let external_active =
            !managed_active && external_task_info(profile, task, workspace).is_some();
        if !managed_active && !external_active {
            return Err(format!("{} is not running.", task.name));
        }

        self.stop_task(profiles, request, workspace)?;
        self.start_task(profiles, request, logs_dir, None)
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

fn ensure_process_identity(pid: u32, start_time: u64) -> Result<(), String> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );
    let process = system.process(pid).ok_or_else(|| {
        "The process changed since the last scan. Refresh and try again.".to_string()
    })?;
    if process.start_time() != start_time {
        return Err(
            "The PID was reused by another process. Refresh before reading its logs.".into(),
        );
    }
    Ok(())
}

fn unavailable_service_logs(message: &str) -> ServiceLogSnapshot {
    ServiceLogSnapshot {
        logs: String::new(),
        source_path: None,
        available: false,
        message: Some(message.into()),
    }
}

fn task_key(profile_id: &str, task_name: &str) -> String {
    format!("{profile_id}\0{task_name}")
}

fn is_active(state: &str) -> bool {
    matches!(state, "starting" | "running" | "stopping")
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
                container: None,
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
            container: None,
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
                container: None,
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
                container: None,
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
                container: None,
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
                container: None,
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
    fn restart_rejects_stopped_and_failed_tasks() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "web".into(),
                cwd: ".".into(),
                command: "echo ok".into(),
                expected_port: None,
                container: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "web".into(),
        };

        for state in ["stopped", "failed"] {
            let mut manager = LaunchManager {
                tasks: HashMap::from([(
                    task_key("profile", "web"),
                    RuntimeTask {
                        child: None,
                        pid: None,
                        state: state.into(),
                        started_at: None,
                        message: None,
                        log_path: temporary.path().join("web.log"),
                    },
                )]),
            };

            let error = manager
                .restart_task(
                    std::slice::from_ref(&profile),
                    &request,
                    temporary.path(),
                    None,
                )
                .unwrap_err();
            assert_eq!(error, "web is not running.");
        }
    }

    #[cfg(unix)]
    #[test]
    fn restart_replaces_managed_task_and_appends_to_its_log() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "web".into(),
                cwd: ".".into(),
                command: "sleep 30".into(),
                expected_port: None,
                container: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "web".into(),
        };
        let logs_dir = temporary.path().join("logs");
        let mut manager = LaunchManager::default();

        let first = manager
            .start_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap();
        let first_pid = first.main_pid.expect("managed task PID");

        let restarted = manager
            .restart_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap();
        let restarted_pid = restarted.main_pid.expect("restarted task PID");

        assert_eq!(restarted.state, "running");
        assert_ne!(restarted_pid, first_pid);
        let log = fs::read_to_string(logs_dir.join("profile-web.log")).unwrap();
        assert_eq!(log.matches("=== Cutting Board start ").count(), 2);

        manager.stop_all();
    }

    #[test]
    fn a_container_task_gets_no_snapshot_from_the_manager() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![
                LaunchTask {
                    name: "web".into(),
                    cwd: ".".into(),
                    command: "npm run dev".into(),
                    expected_port: None,
                    container: None,
                },
                LaunchTask {
                    name: "app-db".into(),
                    cwd: ".".into(),
                    command: String::new(),
                    expected_port: Some(5432),
                    container: Some("app-db".into()),
                },
            ],
        };
        let mut manager = LaunchManager::default();

        let snapshots = manager.snapshots(
            std::slice::from_ref(&profile),
            None,
            &temporary.path().join("logs"),
        );

        // Docker answers for the container task, so a stopped snapshot here would shadow its state.
        let names = snapshots
            .iter()
            .map(|snapshot| snapshot.task_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["web"]);
    }

    #[cfg(unix)]
    #[test]
    fn restart_replaces_a_strict_external_task_using_a_fresh_managed_runtime() {
        use std::process::{Command, Stdio};

        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "web".into(),
                cwd: ".".into(),
                command: "sleep 30".into(),
                expected_port: Some(5173),
                container: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "web".into(),
        };
        let mut external_child = Command::new("sleep")
            .arg("30")
            .current_dir(temporary.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let external_pid = external_child.id();
        let mut system = System::new();
        let external_start = loop {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[Pid::from_u32(external_pid)]),
                true,
                ProcessRefreshKind::nothing().without_tasks(),
            );
            if let Some(process) = system.process(Pid::from_u32(external_pid)) {
                break process.start_time();
            }
            thread::sleep(Duration::from_millis(10));
        };
        let reaper = thread::spawn(move || external_child.wait().unwrap());

        let mut workspace = workspace_with_project(5173, temporary.path());
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid: external_pid,
            parent_pid: None,
            name: "sleep".into(),
            executable: None,
            working_directory: Some(temporary.path().to_string_lossy().into_owned()),
            command: "sleep 30".into(),
            launch_command: Some("sleep 30".into()),
            create_time: external_start,
            uptime_seconds: 0,
            cpu_percent: None,
            memory_bytes: None,
            uid: None,
        });
        let logs_dir = temporary.path().join("logs");
        let mut manager = LaunchManager::default();

        let restarted = manager
            .restart_task(
                std::slice::from_ref(&profile),
                &request,
                &logs_dir,
                Some(&workspace),
            )
            .unwrap();

        reaper.join().unwrap();
        assert_eq!(restarted.state, "running");
        assert!(restarted.main_pid.is_some());
        assert_eq!(restarted.external_pid, None);
        manager.stop_all();
    }

    #[test]
    fn task_key_separates_profile_and_name() {
        assert_ne!(task_key("ab", "c"), task_key("a", "bc"));
    }
}
