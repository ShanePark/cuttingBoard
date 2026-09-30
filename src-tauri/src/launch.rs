use crate::models::{
    now_epoch, LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceLogSnapshot, ServiceSnapshot,
    TaskRequest, WorkspaceSnapshot,
};
pub mod containers;
mod external;
mod logs;
mod prepare;
mod shell;

use external::{
    external_log_path, external_snapshot, external_task_info, service_belongs_to_runtime,
    stop_external_task, task_matches_service, ExternalTaskInfo,
};
use logs::{
    read_log_tail, recovered_external_task_log_path, recovered_managed_log_path, safe_name,
    LogSource, LogSourceCache, LogSourceKind,
};
use prepare::{
    append_log, apply_environment, apply_java_home, prepare_task, PrepareCache, PrepareResult,
};
pub(crate) use shell::shell_command;

use std::{
    collections::{HashMap, HashSet},
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
    process_group_id: Option<u32>,
    state: String,
    started_at: Option<u64>,
    exit_code: Option<i32>,
    exit_success: Option<bool>,
    message: Option<String>,
    log_path: PathBuf,
}

#[derive(Debug, Default)]
pub struct LaunchManager {
    tasks: HashMap<String, RuntimeTask>,
    log_sources: LogSourceCache,
    prepare_cache: PrepareCache,
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
                    self.attach_external_log(
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
                        exit_code: None,
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

    /// Read a task's log directly from disk without taking the manager mutex. This is used while
    /// a long Maven/Gradle preparation owns the manager for its lifecycle operation: callers can
    /// still display the preparation command and build output before the operation completes.
    pub fn task_log_tail(
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        logs_dir: &Path,
    ) -> Result<String, String> {
        let (profile, task) = find_task(profiles, request)?;
        let path = task_log_path(logs_dir, profile, task);
        match read_log_tail(&path) {
            Ok(logs) => Ok(logs),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(error) => Err(format!("Could not read {}: {error}", path.display())),
        }
    }

    /// Attach the output of a process Cutting Board did not start: a file it writes to, or the
    /// task log of an earlier Cutting Board session that started it.
    fn attach_external_log(
        &mut self,
        external: &mut ExternalTaskInfo,
        logs_dir: &Path,
        profiles: &[LaunchProfile],
        profile: &LaunchProfile,
        task: &LaunchTask,
        workspace: Option<&WorkspaceSnapshot>,
    ) {
        let (Some(pid), Some(started_at)) = (external.pid, external.started_at) else {
            return;
        };
        let source = self.log_sources.resolve(pid, started_at, || {
            external_log_path(pid)
                .map(|path| LogSource {
                    kind: LogSourceKind::Process,
                    path,
                })
                .or_else(|| {
                    recovered_external_task_log_path(
                        logs_dir, profiles, profile, task, workspace, started_at,
                    )
                    .map(|path| LogSource {
                        kind: LogSourceKind::Managed,
                        path,
                    })
                })
        });
        let Some(source) = source else {
            return;
        };
        if let Ok(tail) = read_log_tail(&source.path) {
            external.log_path = Some(source.path);
            external.log_tail = tail;
        }
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
        let source = self.log_sources.resolve(pid, start_time, || {
            external_log_path(pid)
                .map(|path| LogSource {
                    kind: LogSourceKind::Process,
                    path,
                })
                .or_else(|| {
                    recovered_managed_log_path(logs_dir, profiles, service, start_time).map(
                        |path| LogSource {
                            kind: LogSourceKind::Managed,
                            path,
                        },
                    )
                })
        });
        ensure_process_identity(pid, start_time)?;
        let Some(source) = source else {
            return Ok(unavailable_service_logs(
                "No readable log file is connected to this service. Output sent to a terminal or an IDE cannot be recovered after launch; a log file the process writes to, such as logging.file.name for a Spring Boot app, appears here.",
            ));
        };
        match read_log_tail(&source.path) {
            Ok(logs) => {
                ensure_process_identity(pid, start_time)?;
                Ok(ServiceLogSnapshot {
                    logs,
                    source_path: Some(source.path.to_string_lossy().into_owned()),
                    available: true,
                    message: None,
                })
            }
            Err(_) => Ok(unavailable_service_logs(match source.kind {
                LogSourceKind::Process => {
                    "A log file the process writes to was found, but it could not be read."
                }
                LogSourceKind::Managed => {
                    "A Cutting Board task log was found, but it could not be read."
                }
            })),
        }
    }

    /// Count the registered task-to-service matches that could identify this task's external
    /// process. A task is safe to take over only when this returns exactly one.
    pub fn external_task_match_count(
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<usize, String> {
        let (target_profile, target_task) = find_task(profiles, request)?;
        let Some(workspace) = workspace else {
            return Ok(0);
        };
        let process_identities = workspace
            .services
            .iter()
            .filter(|service| {
                service.process.is_some()
                    && task_matches_service(target_profile, target_task, service)
            })
            .filter_map(|service| {
                service
                    .process
                    .as_ref()
                    .map(|process| (process.pid, process.create_time))
            })
            .collect::<HashSet<_>>();
        if process_identities.is_empty() {
            return Ok(0);
        }

        let candidate_services = workspace
            .services
            .iter()
            .filter(|service| {
                service.process.as_ref().is_some_and(|process| {
                    process_identities.contains(&(process.pid, process.create_time))
                })
            })
            .collect::<Vec<_>>();
        let mut matching_tasks = HashSet::new();
        for profile in profiles {
            for task in &profile.tasks {
                if candidate_services
                    .iter()
                    .any(|service| task_matches_service(profile, task, service))
                {
                    matching_tasks.insert((profile.id.clone(), task.name.clone()));
                }
            }
        }

        // Multiple scan entries for the same PID/start time are one process. Ambiguity can also
        // come from multiple registered tasks that match that process, so report the larger set.
        Ok(process_identities.len().max(matching_tasks.len()))
    }

    pub fn start_task(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        logs_dir: &Path,
        workspace: Option<&WorkspaceSnapshot>,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.start_task_inner(profiles, request, logs_dir, workspace, None, false)
    }

    fn start_task_inner(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
        logs_dir: &Path,
        workspace: Option<&WorkspaceSnapshot>,
        prepared: Option<PrepareResult>,
        preparation_attempted: bool,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.refresh();
        let (profile, task) = find_task(profiles, request)?;
        let key = task_key(&profile.id, &task.name);
        if let Some(runtime) = self.tasks.get(&key) {
            if is_active(&runtime.state) {
                return Ok(snapshot_from(&profile.id, &task.name, runtime));
            }
        }
        if let Some(mut external) = external_task_info(profile, task, workspace) {
            self.attach_external_log(&mut external, logs_dir, profiles, profile, task, workspace);
            return Ok(external_snapshot(&profile.id, &task.name, external));
        }

        let cwd = resolve_cwd(profile, task)?;
        fs::create_dir_all(logs_dir)
            .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;
        let log_path = logs_dir.join(format!(
            "{}-{}.log",
            safe_name(&profile.id),
            safe_name(&task.name)
        ));
        // Preparation is deliberately completed before the launch log is opened and the new
        // process is spawned. A failed build therefore leaves an existing process untouched.
        let preparation = if preparation_attempted {
            prepared
        } else {
            prepare_task(&mut self.prepare_cache, profile, task, logs_dir, &log_path)?
        };
        let launch_description = preparation
            .as_ref()
            .and_then(|result| result.launch.as_ref())
            .map(prepared_launch_display)
            .unwrap_or_else(|| format!("saved command · {}", task.command));
        let _ = append_log(
            &log_path,
            &format!(
                "=== Cutting Board launch command selected · {} ===",
                launch_description
            ),
        );
        let _ = append_log(
            &log_path,
            &format!(
                "=== Cutting Board starting new process · task={} ===",
                task.name
            ),
        );
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

        let mut command = if let Some(prepared) = preparation.and_then(|result| result.launch) {
            let mut command = std::process::Command::new(&prepared.program);
            command.args(&prepared.args).current_dir(&prepared.cwd);
            apply_java_home(&mut command, prepared.java_home.as_deref());
            apply_environment(&mut command, &prepared.environment);
            command
        } else {
            let mut command = shell_command(&task.command);
            command.current_dir(&cwd);
            command
        };
        command
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
            let message = format!(
                "Could not start {} in {}: {error}",
                task.name,
                cwd.display()
            );
            let _ = append_log(
                &log_path,
                &format!("=== Cutting Board process start failed · {} ===", message),
            );
            message
        })?;
        let pid = child.id();
        let runtime = RuntimeTask {
            child: Some(child),
            pid: Some(pid),
            process_group_id: Some(pid),
            state: "starting".into(),
            started_at: Some(now_epoch()),
            exit_code: None,
            exit_success: None,
            message: Some(format!("Started {} as PID {pid}.", task.name)),
            log_path: log_path.clone(),
        };
        self.tasks.insert(key, runtime);
        let _ = append_log(
            &log_path,
            &format!(
                "=== Cutting Board new process started · task={} · pid={} ===",
                task.name, pid
            ),
        );
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
        logs_dir: &Path,
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
            if let Err(error) = terminate_runtime(runtime) {
                runtime.message = Some(error.clone());
                return Err(error);
            }
            runtime.state = "stopped".into();
            runtime.message = Some(format!("Stopped {}.", task.name));
            return Ok(snapshot_from(&profile.id, &task.name, runtime));
        }

        if let Some(mut external) = external_task_info(profile, task, workspace) {
            self.attach_external_log(&mut external, logs_dir, profiles, profile, task, workspace);
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

        // Build first. In particular, Maven's reactor must install changed sibling modules before
        // the project-native launcher resolves its runtime class path; stopping first would turn
        // a compile failure into unnecessary downtime.
        let log_path = task_log_path(logs_dir, profile, task);
        append_log(
            &log_path,
            &format!(
                "=== Cutting Board restart requested · task={} ===",
                task.name
            ),
        )?;
        let preparation =
            prepare_task(&mut self.prepare_cache, profile, task, logs_dir, &log_path)?;
        append_log(
            &log_path,
            &format!(
                "=== Cutting Board restart preparation ready · cache={} ===",
                preparation
                    .as_ref()
                    .map(|result| if result.skipped { "hit" } else { "miss" })
                    .unwrap_or("not-required")
            ),
        )?;
        let previous_pid = self
            .tasks
            .get(&key)
            .and_then(|runtime| runtime.pid)
            .or_else(|| external_task_info(profile, task, workspace).and_then(|task| task.pid));
        append_log(
            &log_path,
            &format!(
                "=== Cutting Board stopping previous process · pid={} ===",
                previous_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ),
        )?;
        self.stop_task(profiles, request, logs_dir, workspace)?;
        append_log(
            &log_path,
            &format!(
                "=== Cutting Board previous process stopped · task={} ===",
                task.name
            ),
        )?;
        self.start_task_inner(profiles, request, logs_dir, None, preparation, true)
    }

    pub fn stop_profile(
        &mut self,
        profiles: &[LaunchProfile],
        profile_id: &str,
        logs_dir: &Path,
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
                snapshots.push(self.stop_task(profiles, &request, logs_dir, workspace)?);
            }
        }
        Ok(snapshots)
    }

    /// Forgets a profile's tasks, stopping the ones Cutting Board started. A profile can be
    /// deleted while it runs, and this keeps that from leaving a managed process behind with no
    /// card left to stop it from. Processes Cutting Board merely recognised are left running.
    pub fn discard_profile(&mut self, profile_id: &str) {
        self.refresh();
        let prefix = format!("{profile_id}\0");
        let keys = self
            .tasks
            .keys()
            .filter(|key| key.starts_with(&prefix))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let Some(mut runtime) = self.tasks.remove(&key) else {
                continue;
            };
            if is_active(&runtime.state) {
                let _ = terminate_runtime(&mut runtime);
            }
        }
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
                match terminate_runtime(runtime) {
                    Ok(()) => {
                        runtime.state = "stopped".into();
                        runtime.message = Some("Stopped while Cutting Board closed.".into());
                    }
                    Err(error) => runtime.message = Some(error),
                }
            }
        }
    }

    fn refresh(&mut self) {
        for runtime in self.tasks.values_mut() {
            let child_status = match runtime.child.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(status) => status,
                    Err(error) => {
                        runtime.state = "running".into();
                        runtime.message = Some(format!(
                            "Could not inspect the managed process; keeping it active: {error}"
                        ));
                        continue;
                    }
                },
                None => None,
            };
            if let Some(status) = child_status {
                runtime.child = None;
                runtime.pid = None;
                runtime.exit_code = status.code();
                runtime.exit_success = Some(status.success());
                runtime.state = if status.success() {
                    "stopped".into()
                } else {
                    "failed".into()
                };
                runtime.message = Some(format!("Process exited with {status}."));
            } else if runtime.child.is_some() {
                if runtime.state == "starting" {
                    runtime.state = "running".into();
                }
                continue;
            }

            let Some(group_id) = runtime.process_group_id else {
                continue;
            };
            match process_group_has_live_members(group_id) {
                Ok(true) => {
                    if runtime.child.is_none() {
                        runtime.state = "running".into();
                        runtime.message = Some(match runtime.exit_code {
                            Some(code) => format!(
                                "The launch process exited with code {code}; a child process is still running."
                            ),
                            None => "The launch process exited; a child process is still running."
                                .into(),
                        });
                    }
                }
                Ok(false) => {
                    runtime.process_group_id = None;
                    runtime.pid = None;
                    if runtime.child.is_none() {
                        runtime.state = match runtime.exit_success {
                            Some(true) => "stopped".into(),
                            Some(false) => "failed".into(),
                            None => runtime.state.clone(),
                        };
                        runtime.message = match (runtime.exit_success, runtime.exit_code) {
                            (Some(true), Some(code)) => {
                                Some(format!("Process exited with code {code}."))
                            }
                            (Some(false), Some(code)) => {
                                Some(format!("Process exited with code {code}."))
                            }
                            (Some(true), None) => Some("Process exited successfully.".into()),
                            (Some(false), None) => Some("Process exited unsuccessfully.".into()),
                            (None, _) => runtime.message.clone(),
                        };
                    }
                }
                Err(error) => {
                    // Keep the task active when process-group visibility is uncertain. Starting
                    // a replacement could otherwise overlap a child whose port is still bound.
                    runtime.state = "running".into();
                    runtime.message =
                        Some(format!("Could not verify managed child processes: {error}"));
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

fn task_log_path(logs_dir: &Path, profile: &LaunchProfile, task: &LaunchTask) -> PathBuf {
    logs_dir.join(format!(
        "{}-{}.log",
        safe_name(&profile.id),
        safe_name(&task.name)
    ))
}

fn terminate_runtime(runtime: &mut RuntimeTask) -> Result<(), String> {
    let Some(group_id) = runtime.process_group_id else {
        return if runtime.child.is_none() {
            Ok(())
        } else {
            Err("Could not verify the managed process group before stopping it.".into())
        };
    };
    #[cfg(unix)]
    send_process_group_signal(group_id, libc::SIGTERM)?;
    #[cfg(not(unix))]
    if let Some(child) = runtime.child.as_mut() {
        child
            .kill()
            .map_err(|error| format!("Could not stop PID {group_id}: {error}"))?;
    }

    const TERM_GRACE: Duration = Duration::from_secs(2);
    const KILL_GRACE: Duration = Duration::from_secs(1);
    let term_deadline = std::time::Instant::now() + TERM_GRACE;
    loop {
        reap_runtime_leader(runtime, group_id)?;
        if !process_group_has_live_members(group_id)? {
            runtime.process_group_id = None;
            runtime.pid = None;
            return Ok(());
        }
        if std::time::Instant::now() >= term_deadline {
            break;
        }
        thread::sleep(Duration::from_millis(40));
    }

    #[cfg(unix)]
    send_process_group_signal(group_id, libc::SIGKILL)?;
    #[cfg(not(unix))]
    if let Some(child) = runtime.child.as_mut() {
        child
            .kill()
            .map_err(|error| format!("Could not force PID {group_id} to stop: {error}"))?;
    }

    let kill_deadline = std::time::Instant::now() + KILL_GRACE;
    loop {
        reap_runtime_leader(runtime, group_id)?;
        if !process_group_has_live_members(group_id)? {
            runtime.process_group_id = None;
            runtime.pid = None;
            return Ok(());
        }
        if std::time::Instant::now() >= kill_deadline {
            break;
        }
        thread::sleep(Duration::from_millis(40));
    }

    Err(format!(
        "Process group {group_id} still has live processes after SIGKILL."
    ))
}

fn reap_runtime_leader(runtime: &mut RuntimeTask, group_id: u32) -> Result<(), String> {
    let status = match runtime.child.as_mut() {
        Some(child) => child
            .try_wait()
            .map_err(|error| format!("Could not wait for PID {group_id}: {error}"))?,
        None => None,
    };
    if let Some(status) = status {
        runtime.child = None;
        runtime.pid = None;
        runtime.exit_code = status.code();
        runtime.exit_success = Some(status.success());
        runtime.message = Some(format!("Process exited with {status}."));
    }
    Ok(())
}

#[cfg(unix)]
fn send_process_group_signal(group_id: u32, signal: i32) -> Result<(), String> {
    let result = unsafe { libc::kill(-(group_id as i32), signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!(
            "Could not signal process group {group_id}: {error}"
        ))
    }
}

#[cfg(target_os = "linux")]
fn process_group_has_live_members(group_id: u32) -> Result<bool, String> {
    let entries = fs::read_dir("/proc").map_err(|error| {
        format!("Could not inspect /proc while stopping process group {group_id}: {error}")
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!("Could not inspect /proc while stopping process group {group_id}: {error}")
        })?;
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let process_group = unsafe { libc::getpgid(pid as i32) };
        if process_group < 0 {
            // PIDs that exit between enumeration and inspection are harmless. An inaccessible
            // process outside the owned group must not prevent cleanup of this task's group.
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error(),
                Some(libc::ESRCH | libc::EACCES | libc::EPERM)
            ) {
                continue;
            }
            return Err(format!(
                "Could not inspect PID {pid} while stopping process group {group_id}: {error}"
            ));
        }
        if process_group as u32 != group_id {
            continue;
        }
        let stat_path = entry.path().join("stat");
        let stat = match fs::read_to_string(&stat_path) {
            Ok(stat) => stat,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Err(format!(
                    "Could not inspect {} while stopping process group {group_id}: {error}",
                    stat_path.display()
                ));
            }
            Err(_) => continue,
        };
        let Some((state, process_group)) = linux_process_state_and_group(&stat) else {
            return Err(format!(
                "Could not parse {} while stopping process group {group_id}.",
                stat_path.display()
            ));
        };
        if process_group == group_id && state != 'Z' && state != 'X' {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn linux_process_state_and_group(stat: &str) -> Option<(char, u32)> {
    let (_, fields) = stat.rsplit_once(')')?;
    let mut fields = fields.split_whitespace();
    let state = fields.next()?.chars().next()?;
    fields.next()?; // parent process ID
    let process_group = fields.next()?.parse().ok()?;
    Some((state, process_group))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_group_has_live_members(group_id: u32) -> Result<bool, String> {
    let output = std::process::Command::new("ps")
        .args(["-A", "-o", "pgid=,stat="])
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("Could not inspect process group {group_id}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not inspect process group {group_id}: ps exited with {}.",
            output.status
        ));
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(actual_group) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(status) = fields.next() else {
            continue;
        };
        if actual_group == group_id
            && status
                .chars()
                .next()
                .is_some_and(|state| state != 'Z' && state != 'X')
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(unix))]
fn process_group_has_live_members(_group_id: u32) -> Result<bool, String> {
    Ok(false)
}

fn snapshot_from(profile_id: &str, task_name: &str, runtime: &RuntimeTask) -> ManagedTaskSnapshot {
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: runtime.state.clone(),
        main_pid: runtime.pid,
        started_at: runtime.started_at,
        exit_code: runtime.exit_code,
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

fn prepared_launch_display(launch: &prepare::PreparedLaunch) -> String {
    let mut parts = vec![launch.program.to_string_lossy().into_owned()];
    parts.extend(
        launch
            .args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned()),
    );
    format!("{} (cwd={})", parts.join(" "), launch.cwd.display())
}

fn is_active(state: &str) -> bool {
    matches!(state, "starting" | "running" | "stopping")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServiceSnapshot;
    use std::ops::{Deref, DerefMut};

    struct ManagedTestGuard(LaunchManager);

    impl Deref for ManagedTestGuard {
        type Target = LaunchManager;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl DerefMut for ManagedTestGuard {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }

    impl Drop for ManagedTestGuard {
        fn drop(&mut self) {
            self.0.stop_all();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn managed_listener_process_fixture() {
        let (Ok(port), Ok(marker)) = (
            std::env::var("CUTTING_BOARD_TEST_LISTENER_PORT"),
            std::env::var("CUTTING_BOARD_TEST_LISTENER_MARKER"),
        ) else {
            return;
        };
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
        let port = port.parse::<u16>().unwrap();
        let listener = match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => listener,
            Err(error) => {
                use std::io::Write;
                writeln!(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(marker)
                        .unwrap(),
                    "bind_error:{}:{error}",
                    std::process::id()
                )
                .unwrap();
                std::process::exit(9);
            }
        };
        listener.set_nonblocking(true).unwrap();
        {
            use std::io::Write;
            writeln!(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(marker)
                    .unwrap(),
                "ready:{}",
                std::process::id()
            )
            .unwrap();
        }
        loop {
            let _ = listener.accept();
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn managed_listener_profile(root: &Path, port: u16) -> LaunchProfile {
        let marker = root.join("listener-ready.log");
        let script_path = root.join("start-listener.sh");
        let executable = std::env::current_exe().unwrap();
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let script = format!(
            "set +m\nCUTTING_BOARD_TEST_LISTENER_PORT={port} CUTTING_BOARD_TEST_LISTENER_MARKER={} {} --exact launch::tests::managed_listener_process_fixture --nocapture >/dev/null 2>&1 &\nexit 0\n",
            quote(&marker.to_string_lossy()),
            quote(&executable.to_string_lossy()),
        );
        fs::write(&script_path, script).unwrap();
        LaunchProfile {
            id: "profile".into(),
            name: "test".into(),
            project_root: root.to_string_lossy().into_owned(),
            tasks: vec![
                LaunchTask {
                    name: "api".into(),
                    cwd: ".".into(),
                    command: format!("sh {}", quote(&script_path.to_string_lossy())),
                    expected_port: Some(port),
                    container: None,
                    prepare: None,
                },
                LaunchTask {
                    name: "sibling".into(),
                    cwd: ".".into(),
                    command: "sleep 30".into(),
                    expected_port: None,
                    container: None,
                    prepare: None,
                },
            ],
        }
    }

    #[cfg(target_os = "linux")]
    fn listener_pids(marker: &Path) -> Vec<u32> {
        fs::read_to_string(marker)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("ready:"))
            .filter_map(|pid| pid.parse().ok())
            .collect()
    }

    #[cfg(target_os = "linux")]
    fn wait_for_listener_count(marker: &Path, count: usize) -> Vec<u32> {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            let contents = fs::read_to_string(marker).unwrap_or_default();
            assert!(
                !contents.contains("bind_error:"),
                "replacement listener could not bind the released port: {contents}"
            );
            let pids = listener_pids(marker);
            if pids.len() >= count {
                return pids;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "listener did not become ready"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn wait_until_process_group_has_no_live_member(group_id: u32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if !process_group_has_live_members(group_id).unwrap() {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "process group {group_id} still has live members"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    fn wait_for_leader_exit(manager: &mut LaunchManager, request: &TaskRequest) -> u32 {
        let key = task_key(&request.profile_id, &request.task_name);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            manager.refresh();
            let runtime = manager.tasks.get(&key).expect("started runtime");
            if runtime.child.is_none() {
                assert_eq!(runtime.state, "running");
                return runtime
                    .process_group_id
                    .expect("child keeps its process group");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "launch leader did not exit while its child stayed alive"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    struct ExternalListenerGuard {
        child: std::process::Child,
        listener: Option<(u32, u64)>,
        stopped: bool,
    }

    #[cfg(target_os = "linux")]
    impl Drop for ExternalListenerGuard {
        fn drop(&mut self) {
            if !self.stopped {
                match self.child.try_wait() {
                    Ok(None) => {
                        // The fixture leader created a private session, so its process group
                        // contains only this test's wrapper and descendants.
                        unsafe {
                            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                        }
                    }
                    Ok(Some(_)) => self.kill_listener_if_current(),
                    Err(_) => {
                        let _ = self.child.kill();
                        self.kill_listener_if_current();
                    }
                }
            }
            let _ = self.child.wait();
        }
    }

    #[cfg(target_os = "linux")]
    impl ExternalListenerGuard {
        fn kill_listener_if_current(&self) {
            if let Some((pid, start_time)) = self.listener {
                if process_start_time(pid) == Some(start_time) {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    struct DummyChildGuard(std::process::Child);

    #[cfg(target_os = "linux")]
    impl Drop for DummyChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    fn process_start_time(pid: u32) -> Option<u64> {
        let pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        system.process(pid).map(|process| process.start_time())
    }

    #[cfg(target_os = "linux")]
    fn wait_for_process_start_time(pid: u32) -> u64 {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(start_time) = process_start_time(pid) {
                return start_time;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "process {pid} did not appear in the process table"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn duplicate_start_and_restart_clean_the_owned_child_tree_and_keep_sibling_alive() {
        let temporary = tempfile::tempdir().unwrap();
        let port_probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = port_probe.local_addr().unwrap().port();
        drop(port_probe);
        let profile = managed_listener_profile(temporary.path(), port);
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "api".into(),
        };
        let sibling_request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "sibling".into(),
        };
        let marker = temporary.path().join("listener-ready.log");
        let mut manager = ManagedTestGuard(LaunchManager::default());

        let started = manager
            .start_task(
                std::slice::from_ref(&profile),
                &request,
                temporary.path(),
                None,
            )
            .unwrap();
        assert_eq!(started.state, "running");
        let first_pids = wait_for_listener_count(&marker, 1);
        let first_group_id = wait_for_leader_exit(&mut manager, &request);

        let duplicate = manager
            .start_task(
                std::slice::from_ref(&profile),
                &request,
                temporary.path(),
                None,
            )
            .unwrap();
        assert_eq!(duplicate.state, "running");
        assert_eq!(listener_pids(&marker), first_pids);
        assert_eq!(
            manager
                .tasks
                .get(&task_key(&profile.id, "api"))
                .and_then(|runtime| runtime.process_group_id),
            Some(first_group_id)
        );

        let sibling = manager
            .start_task(
                std::slice::from_ref(&profile),
                &sibling_request,
                temporary.path(),
                None,
            )
            .unwrap();
        let sibling_pid = sibling.main_pid.expect("sibling launch PID");
        let sibling_group_id = manager
            .tasks
            .get(&task_key(&profile.id, "sibling"))
            .and_then(|runtime| runtime.process_group_id)
            .expect("sibling process group");

        let restart_started = std::time::Instant::now();
        let restarted = manager
            .restart_task(
                std::slice::from_ref(&profile),
                &request,
                temporary.path(),
                None,
            )
            .unwrap();
        assert!(restart_started.elapsed() >= Duration::from_secs(1));
        assert_eq!(restarted.state, "running");
        let listener_pids = wait_for_listener_count(&marker, 2);
        assert_ne!(listener_pids[0], listener_pids[1]);
        wait_until_process_group_has_no_live_member(first_group_id);
        assert!(process_group_has_live_members(sibling_group_id).unwrap());
        assert!(process_group_has_live_members(sibling_pid).unwrap());

        let stopped_sibling = manager
            .stop_task(
                std::slice::from_ref(&profile),
                &sibling_request,
                temporary.path(),
                None,
            )
            .unwrap();
        assert_eq!(stopped_sibling.state, "stopped");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn external_restart_stops_only_matched_descendants_and_releases_port_before_spawn() {
        let temporary = tempfile::tempdir().unwrap();
        let port_probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = port_probe.local_addr().unwrap().port();
        drop(port_probe);
        let profile = managed_listener_profile(temporary.path(), port);
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "api".into(),
        };
        let marker = temporary.path().join("listener-ready.log");
        let executable = std::env::current_exe().unwrap();
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let external_script = format!(
            "set +m\nCUTTING_BOARD_TEST_LISTENER_PORT={port} CUTTING_BOARD_TEST_LISTENER_MARKER={} {} --exact launch::tests::managed_listener_process_fixture --nocapture >/dev/null 2>&1 &\nwait\n",
            quote(&marker.to_string_lossy()),
            quote(&executable.to_string_lossy()),
        );
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", &external_script])
            .current_dir(temporary.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = command.spawn().unwrap();
        let mut external = ExternalListenerGuard {
            child,
            listener: None,
            stopped: false,
        };
        let root_start_time = wait_for_process_start_time(external.child.id());
        let listener_pid = wait_for_listener_count(&marker, 1)[0];
        let listener_start_time = wait_for_process_start_time(listener_pid);
        external.listener = Some((listener_pid, listener_start_time));

        let mut workspace = workspace_with_project(port, temporary.path());
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid: external.child.id(),
            parent_pid: None,
            name: "sh".into(),
            executable: None,
            working_directory: Some(temporary.path().to_string_lossy().into_owned()),
            command: "external test listener wrapper".into(),
            launch_command: None,
            create_time: root_start_time,
            uptime_seconds: 0,
            cpu_percent: None,
            memory_bytes: None,
            uid: Some(unsafe { libc::geteuid() }),
        });
        let mut unrelated = DummyChildGuard(
            std::process::Command::new("sleep")
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let mut manager = ManagedTestGuard(LaunchManager::default());

        let restarted = manager
            .restart_task(
                std::slice::from_ref(&profile),
                &request,
                temporary.path(),
                Some(&workspace),
            )
            .unwrap();
        external.stopped = true;

        assert_eq!(restarted.state, "running");
        wait_for_listener_count(&marker, 2);
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn managed_task_reports_a_nonzero_exit_code() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "test".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "failing".into(),
                cwd: ".".into(),
                command: "exit 23".into(),
                expected_port: None,
                container: None,
                prepare: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "failing".into(),
        };
        let mut manager = ManagedTestGuard(LaunchManager::default());

        manager
            .start_task(
                std::slice::from_ref(&profile),
                &request,
                temporary.path(),
                None,
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let snapshot = loop {
            let snapshot = manager
                .snapshots(std::slice::from_ref(&profile), None, temporary.path())
                .into_iter()
                .next()
                .unwrap();
            if snapshot.state == "failed" {
                break snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "task did not report its failing exit"
            );
            thread::sleep(Duration::from_millis(20));
        };

        assert_eq!(snapshot.state, "failed");
        assert_eq!(snapshot.exit_code, Some(23));
        assert!(snapshot.message.unwrap().contains("code 23"));
    }

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
                prepare: None,
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
            log_sources: LogSourceCache::default(),
            prepare_cache: PrepareCache::default(),
            tasks: HashMap::from([(
                task_key("profile", "frontend"),
                RuntimeTask {
                    child: None,
                    pid: Some(123),
                    process_group_id: None,
                    state: "running".into(),
                    started_at: Some(1),
                    exit_code: None,
                    exit_success: None,
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
            prepare: None,
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
                prepare: None,
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
                prepare: None,
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
                prepare: None,
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
                prepare: None,
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        let mut manager = LaunchManager {
            log_sources: LogSourceCache::default(),
            prepare_cache: PrepareCache::default(),
            tasks: HashMap::from([(
                task_key("profile", "frontend"),
                RuntimeTask {
                    child: None,
                    pid: Some(42),
                    process_group_id: None,
                    state: "running".into(),
                    started_at: Some(1),
                    exit_code: None,
                    exit_success: None,
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
                prepare: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "web".into(),
        };

        for state in ["stopped", "failed"] {
            let mut manager = LaunchManager {
                log_sources: LogSourceCache::default(),
                prepare_cache: PrepareCache::default(),
                tasks: HashMap::from([(
                    task_key("profile", "web"),
                    RuntimeTask {
                        child: None,
                        pid: None,
                        process_group_id: None,
                        state: state.into(),
                        started_at: None,
                        exit_code: None,
                        exit_success: None,
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
                prepare: None,
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

    #[cfg(unix)]
    #[test]
    fn failed_prepare_keeps_the_existing_managed_process_alive() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("adm");
        fs::create_dir(&module).unwrap();
        fs::write(
            root.join("pom.xml"),
            "<project><modules><module>adm</module></modules></project>",
        )
        .unwrap();
        fs::write(module.join("pom.xml"), "<project/>").unwrap();
        let wrapper = root.join("mvnw");
        fs::write(&wrapper, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).unwrap();

        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: root.to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "adm".into(),
                cwd: "adm".into(),
                command: "java -cp stale-common.jar App".into(),
                expected_port: Some(8080),
                container: None,
                prepare: Some(crate::models::LaunchPrepareSpec {
                    kind: crate::models::LaunchPrepareKind::SpringBoot,
                    build_tool: Some(crate::models::LaunchBuildTool::Maven),
                    module: Some("adm".into()),
                    profiles: vec!["dev".into()],
                    main_class: Some("App".into()),
                }),
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "adm".into(),
        };
        let logs_dir = root.join("logs");
        let mut old_command = shell_command("sleep 30");
        unsafe {
            old_command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let old_child = old_command.spawn().unwrap();
        let old_pid = old_child.id();
        let mut manager = LaunchManager {
            tasks: HashMap::from([(
                task_key("profile", "adm"),
                RuntimeTask {
                    child: Some(old_child),
                    pid: Some(old_pid),
                    process_group_id: Some(old_pid),
                    state: "running".into(),
                    started_at: Some(now_epoch()),
                    exit_code: None,
                    exit_success: None,
                    message: None,
                    log_path: logs_dir.join("profile-adm.log"),
                },
            )]),
            log_sources: LogSourceCache::default(),
            prepare_cache: PrepareCache::default(),
        };

        let error = manager
            .restart_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap_err();
        assert!(error.contains("Could not prepare adm"));
        manager.refresh();
        let runtime = manager.tasks.get(&task_key("profile", "adm")).unwrap();
        assert_eq!(runtime.pid, Some(old_pid));
        assert_eq!(runtime.state, "running");
        assert!(runtime.child.is_some());

        manager.stop_all();
    }

    #[cfg(unix)]
    #[test]
    fn slow_prepare_log_is_readable_before_restart_finishes() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::mpsc;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("adm");
        fs::create_dir(&module).unwrap();
        fs::write(
            root.join("pom.xml"),
            "<project><modules><module>adm</module></modules></project>",
        )
        .unwrap();
        fs::write(module.join("pom.xml"), "<project/>").unwrap();
        fs::create_dir_all(module.join("src/main/java")).unwrap();
        fs::write(module.join("src/main/java/App.java"), "class App {}\n").unwrap();
        let wrapper = root.join("mvnw");
        fs::write(
            &wrapper,
            "#!/bin/sh\nSCRIPT_DIR=$(dirname \"$0\")\ncase \"$*\" in\n  *exec-maven-plugin:3.5.1:exec*) sleep 1 ;;\n  *) printf 'prepare stdout before sleep\\n'; printf 'prepare stderr before sleep\\n' >&2; : > \"$SCRIPT_DIR/prepare-running\"; sleep 2; printf 'prepare completed output\\n' ;;\nesac\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).unwrap();

        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: root.to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "adm".into(),
                cwd: "adm".into(),
                command: "java -cp stale-common.jar App".into(),
                expected_port: Some(8080),
                container: None,
                prepare: Some(crate::models::LaunchPrepareSpec {
                    kind: crate::models::LaunchPrepareKind::SpringBoot,
                    build_tool: Some(crate::models::LaunchBuildTool::Maven),
                    module: Some("adm".into()),
                    profiles: vec!["dev".into()],
                    main_class: Some("App".into()),
                }),
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "adm".into(),
        };
        let logs_dir = root.join("logs");
        let log_path = logs_dir.join("profile-adm.log");
        let mut old_command = shell_command("sleep 30");
        unsafe {
            old_command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let old_child = old_command.spawn().unwrap();
        let old_pid = old_child.id();
        let mut manager = LaunchManager {
            tasks: HashMap::from([(
                task_key("profile", "adm"),
                RuntimeTask {
                    child: Some(old_child),
                    pid: Some(old_pid),
                    process_group_id: Some(old_pid),
                    state: "running".into(),
                    started_at: Some(now_epoch()),
                    exit_code: None,
                    exit_success: None,
                    message: None,
                    log_path: log_path.clone(),
                },
            )]),
            log_sources: LogSourceCache::default(),
            prepare_cache: PrepareCache::default(),
        };
        let (done_sender, done_receiver) = mpsc::channel();
        let worker_profile = profile.clone();
        let worker_request = request.clone();
        let worker_logs = logs_dir.clone();
        let worker = thread::spawn(move || {
            let result = manager.restart_task(
                std::slice::from_ref(&worker_profile),
                &worker_request,
                &worker_logs,
                None,
            );
            let state = result.map(|snapshot| snapshot.state);
            manager.stop_all();
            done_sender.send(state).unwrap();
        });

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline && !root.join("prepare-running").is_file() {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            root.join("prepare-running").is_file(),
            "slow prepare did not start"
        );
        assert!(
            done_receiver.try_recv().is_err(),
            "restart completed while the deliberately slow prepare was running"
        );

        let log = LaunchManager::task_log_tail(std::slice::from_ref(&profile), &request, &logs_dir)
            .unwrap();
        assert!(log.contains("prepare detection started"));
        assert!(log.contains("prepare detected · tool=maven · module=adm"));
        assert!(log.contains("prepare cache miss"));
        assert!(log.contains("prepare command"));
        assert!(log.contains("prepare output started · stdout/stderr follow"));
        assert!(log.contains("prepare stdout before sleep"));
        assert!(log.contains("prepare stderr before sleep"));

        worker.join().unwrap();
        let result = done_receiver.recv().unwrap().unwrap();
        assert_eq!(result, "running");
        let log = LaunchManager::task_log_tail(std::slice::from_ref(&profile), &request, &logs_dir)
            .unwrap();
        assert!(log.contains("prepare completed"));
        assert!(log.contains("stopping previous process"));
        assert!(log.contains("previous process stopped"));
        assert!(log.contains("starting new process"));
        assert!(log.contains("new process started"));
    }

    #[cfg(unix)]
    #[test]
    fn prepared_spring_task_spawns_native_maven_exec_instead_of_stale_java_command() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("adm");
        fs::create_dir(&module).unwrap();
        fs::write(
            root.join("pom.xml"),
            "<project><modules><module>adm</module></modules></project>",
        )
        .unwrap();
        fs::write(module.join("pom.xml"), "<project/>").unwrap();
        fs::create_dir_all(module.join("src/main/java")).unwrap();
        fs::write(
            module.join("src/main/java/ModelWrapperService.java"),
            "class ModelWrapperService {}\n",
        )
        .unwrap();
        let wrapper = root.join("mvnw");
        fs::write(
            &wrapper,
            "#!/bin/sh\nSCRIPT_DIR=$(dirname \"$0\")\nprintf '%s\\n' \"$*\" >> \"$SCRIPT_DIR/invocation-log\"\ncase \"$*\" in\n  *exec-maven-plugin:3.5.1:exec*) printf 'native-exec\\n' > \"$SCRIPT_DIR/native-marker\"; while :; do sleep 1; done ;;\nesac\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&wrapper, permissions).unwrap();

        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: root.to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "adm".into(),
                cwd: "adm".into(),
                command: "java -cp /distinct/stale-common.jar kr.re.kisti.idr.aip.ModelWrapperService --raw-command".into(),
                expected_port: Some(8080),
                container: None,
                prepare: Some(crate::models::LaunchPrepareSpec {
                    kind: crate::models::LaunchPrepareKind::SpringBoot,
                    build_tool: Some(crate::models::LaunchBuildTool::Maven),
                    module: Some("adm".into()),
                    profiles: vec!["dev".into()],
                    main_class: Some("kr.re.kisti.idr.aip.ModelWrapperService".into()),
                }),
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "adm".into(),
        };
        let logs_dir = root.join("logs");
        let mut manager = LaunchManager::default();

        let snapshot = manager
            .start_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap();
        assert_eq!(snapshot.state, "running");
        assert!(snapshot.main_pid.is_some());
        assert!(root.join("native-marker").is_file());
        let invocations = fs::read_to_string(root.join("invocation-log")).unwrap();
        assert!(invocations.contains("install -Dmaven.test.skip=true"));
        assert!(invocations.contains("org.codehaus.mojo:exec-maven-plugin:3.5.1:exec"));
        assert!(invocations.contains("-Dexec.executable=java"));
        assert!(invocations.contains("-Dexec.classpathScope=runtime"));
        assert!(invocations.contains("-classpath %classpath"));
        assert!(invocations.contains("kr.re.kisti.idr.aip.ModelWrapperService"));
        assert!(invocations.contains("-Dspring.profiles.active=dev"));
        assert!(!invocations.contains("stale-common.jar"));

        let restarted = manager
            .restart_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap();
        assert_eq!(restarted.state, "running");
        let invocations = fs::read_to_string(root.join("invocation-log")).unwrap();
        assert_eq!(invocations.lines().count(), 3); // prepare once, then native launch twice
        let log = fs::read_to_string(logs_dir.join("profile-adm.log")).unwrap();
        assert_eq!(
            log.matches("prepare skipped (unchanged inputs)").count(),
            1,
            "restart reuses the already completed preparation"
        );

        manager.stop_all();
    }

    #[test]
    fn discarding_a_profile_stops_and_forgets_its_running_tasks() {
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
                prepare: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "web".into(),
        };
        let mut manager = LaunchManager::default();
        manager
            .start_task(
                std::slice::from_ref(&profile),
                &request,
                &temporary.path().join("logs"),
                None,
            )
            .unwrap();
        assert!(manager.profile_is_active(std::slice::from_ref(&profile), &profile.id, None));

        manager.discard_profile(&profile.id);

        assert!(manager.tasks.is_empty());
        assert!(!manager.profile_is_active(std::slice::from_ref(&profile), &profile.id, None));
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
                    prepare: None,
                },
                LaunchTask {
                    name: "app-db".into(),
                    cwd: ".".into(),
                    command: String::new(),
                    expected_port: Some(5432),
                    container: Some("app-db".into()),
                    prepare: None,
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
                prepare: None,
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

    #[cfg(unix)]
    #[test]
    fn recreated_manager_recovers_detached_process_and_task_log() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "api".into(),
                cwd: ".".into(),
                command: "printf 'detached-manager-marker\\n'; sleep 30".into(),
                expected_port: Some(18080),
                container: None,
                prepare: None,
            }],
        };
        let request = TaskRequest {
            profile_id: profile.id.clone(),
            task_name: "api".into(),
        };
        let logs_dir = temporary.path().join("logs");
        let mut manager = LaunchManager::default();
        let started = manager
            .start_task(std::slice::from_ref(&profile), &request, &logs_dir, None)
            .unwrap();
        let pid = started.main_pid.expect("managed shell PID");
        let log_path = logs_dir.join("profile-api.log");
        let canonical_log_path = log_path.canonicalize().unwrap();
        let marker_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < marker_deadline {
            if fs::read_to_string(&log_path)
                .map(|contents| contents.contains("detached-manager-marker"))
                .unwrap_or(false)
            {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        let mut system = System::new();
        let started_at = loop {
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
                true,
                ProcessRefreshKind::nothing().without_tasks(),
            );
            if let Some(process) = system.process(Pid::from_u32(pid)) {
                break process.start_time();
            }
            thread::sleep(Duration::from_millis(10));
        };
        // Dropping Child only releases Cutting Board's wait handle. The task is a new session
        // (setsid in start_task), so its shell and descendants stay alive for the next manager.
        drop(manager);

        let mut workspace = workspace_with_project(18080, temporary.path());
        workspace.services[0].process = Some(crate::models::ProcessInfo {
            pid,
            parent_pid: None,
            name: "sh".into(),
            executable: Some("/bin/sh".into()),
            working_directory: Some(temporary.path().to_string_lossy().into_owned()),
            command: "sh -c printf detached-manager-marker; sleep 30".into(),
            launch_command: Some(profile.tasks[0].command.clone()),
            create_time: started_at,
            uptime_seconds: 0,
            cpu_percent: None,
            memory_bytes: None,
            uid: Some(unsafe { libc::geteuid() }),
        });

        let mut recreated = LaunchManager::default();
        let snapshots =
            recreated.snapshots(std::slice::from_ref(&profile), Some(&workspace), &logs_dir);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].state, "running");
        assert_eq!(snapshots[0].external_pid, Some(pid));
        assert_eq!(snapshots[0].main_pid, Some(pid));
        assert_eq!(
            snapshots[0].external_log_path.as_deref(),
            Some(canonical_log_path.to_string_lossy().as_ref())
        );
        assert!(snapshots[0].log_tail.contains("detached-manager-marker"));

        // The recreated manager can observe and attach to the task, but it does not own a Child
        // handle with which to reap the detached shell. Finish process-group cleanup directly so
        // the sleeping descendant cannot leak into later test runs.
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }

    #[test]
    fn task_key_separates_profile_and_name() {
        assert_ne!(task_key("ab", "c"), task_key("a", "bc"));
    }
}
