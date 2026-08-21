use crate::models::{
    now_epoch, LaunchProfile, LaunchTask, ManagedTaskSnapshot, TaskRequest, WorkspaceSnapshot,
};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::{env, ffi::CStr};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

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
    ) -> Vec<ManagedTaskSnapshot> {
        self.refresh();
        let mut snapshots = Vec::new();
        for profile in profiles {
            for task in &profile.tasks {
                let key = task_key(&profile.id, &task.name);
                if let Some(runtime) = self.tasks.get(&key) {
                    snapshots.push(snapshot_from(&profile.id, &task.name, runtime));
                } else if listener_is_external(profile, task, workspace) {
                    snapshots.push(ManagedTaskSnapshot {
                        profile_id: profile.id.clone(),
                        task_name: task.name.clone(),
                        state: "external".into(),
                        main_pid: None,
                        started_at: None,
                        message: Some("This process is running externally and cannot be stopped by Cutting Board.".into()),
                        log_tail: String::new(),
                    });
                } else {
                    snapshots.push(ManagedTaskSnapshot {
                        profile_id: profile.id.clone(),
                        task_name: task.name.clone(),
                        state: "stopped".into(),
                        main_pid: None,
                        started_at: None,
                        message: None,
                        log_tail: String::new(),
                    });
                }
            }
        }
        snapshots
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
        if listener_is_external(profile, task, workspace) {
            return Ok(ManagedTaskSnapshot {
                profile_id: profile.id.clone(),
                task_name: task.name.clone(),
                state: "external".into(),
                main_pid: None,
                started_at: None,
                message: Some("The expected port is already owned by an external process.".into()),
                log_tail: String::new(),
            });
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
        let log_path = logs_dir.join(format!("{}-{}.log", safe_name(&profile.id), safe_name(&task.name)));
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
            self.tasks.get(&task_key(&profile.id, &task.name)).expect("inserted runtime task"),
        ))
    }

    pub fn stop_task(
        &mut self,
        profiles: &[LaunchProfile],
        request: &TaskRequest,
    ) -> Result<ManagedTaskSnapshot, String> {
        self.refresh();
        let (profile, task) = find_task(profiles, request)?;
        let key = task_key(&profile.id, &task.name);
        let runtime = self
            .tasks
            .get_mut(&key)
            .ok_or_else(|| format!("{} is not owned by Cutting Board.", task.name))?;
        if !is_active(&runtime.state) {
            return Ok(snapshot_from(&profile.id, &task.name, runtime));
        }
        runtime.state = "stopping".into();
        runtime.message = Some(format!("Stopping {}…", task.name));
        terminate_runtime(runtime)?;
        runtime.state = "stopped".into();
        runtime.message = Some(format!("Stopped {}.", task.name));
        Ok(snapshot_from(&profile.id, &task.name, runtime))
    }

    pub fn stop_profile(
        &mut self,
        profiles: &[LaunchProfile],
        profile_id: &str,
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
            if self.tasks.get(&key).is_some_and(|runtime| is_active(&runtime.state)) {
                snapshots.push(self.stop_task(profiles, &request)?);
            }
        }
        Ok(snapshots)
    }

    pub fn profile_is_active(&mut self, profile_id: &str) -> bool {
        self.refresh();
        let prefix = format!("{profile_id}\0");
        self.tasks
            .iter()
            .any(|(key, runtime)| key.starts_with(&prefix) && is_active(&runtime.state))
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
            let Some(child) = runtime.child.as_mut() else { continue };
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
                    runtime.message = Some(format!("Could not inspect the managed process: {error}"));
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
        return Err(format!("The task directory does not exist: {}", resolved.display()));
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
        child.kill().map_err(|error| format!("Could not stop PID {pid}: {error}"))?;
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

fn snapshot_from(
    profile_id: &str,
    task_name: &str,
    runtime: &RuntimeTask,
) -> ManagedTaskSnapshot {
    ManagedTaskSnapshot {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
        state: runtime.state.clone(),
        main_pid: runtime.pid,
        started_at: runtime.started_at,
        message: runtime.message.clone(),
        log_tail: read_log_tail(&runtime.log_path).unwrap_or_default(),
    }
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

fn listener_is_external(
    profile: &LaunchProfile,
    task: &LaunchTask,
    workspace: Option<&WorkspaceSnapshot>,
) -> bool {
    let Some(port) = task.expected_port else { return false };
    let task_root = task_cwd(profile, task)
        .canonicalize()
        .unwrap_or_else(|_| task_cwd(profile, task));
    workspace.is_some_and(|snapshot| {
        snapshot.services.iter().any(|service| {
            service.endpoints.iter().any(|endpoint| endpoint.port == port)
                && (service
                    .process
                    .as_ref()
                    .and_then(|process| process.working_directory.as_deref())
                    .is_some_and(|path| path_matches_task(path, &task_root))
                    || service.project.as_ref().is_some_and(|project| {
                        [project.root_path.as_str(), project.workspace_root_path.as_str()]
                            .into_iter()
                            .any(|path| path_matches_task(path, &task_root))
                    }))
        })
    })
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
        .map(|character| if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') { character } else { '_' })
        .collect::<String>();
    if safe.is_empty() { "task".into() } else { safe }
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
            }],
            scanned_at: 0,
            scan_duration_ms: 0,
            endpoint_count: 1,
            errors: vec![],
        }
    }

    #[test]
    fn relative_task_directory_is_anchored_to_project() {
        let temporary = tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("frontend")).unwrap();
        let profile = LaunchProfile { id: "p".into(), name: "P".into(), project_root: temporary.path().to_string_lossy().into_owned(), tasks: vec![] };
        let task = LaunchTask { name: "web".into(), cwd: "frontend".into(), command: "echo ok".into(), expected_port: None };
        assert_eq!(resolve_cwd(&profile, &task).unwrap(), temporary.path().join("frontend"));
    }

    #[test]
    fn nested_task_is_external_when_service_project_matches_task_cwd() {
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

        let snapshots = LaunchManager::default().snapshots(&[profile], Some(&workspace));

        assert_eq!(snapshots[0].state, "external");
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

        let snapshots = LaunchManager::default().snapshots(&[profile], Some(&workspace));

        assert_eq!(snapshots[0].state, "external");
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

        let snapshots = manager.snapshots(&[profile], Some(&workspace));

        assert_eq!(snapshots[0].state, "running");
        assert_eq!(snapshots[0].main_pid, Some(42));
    }

    #[test]
    fn task_key_separates_profile_and_name() {
        assert_ne!(task_key("ab", "c"), task_key("a", "bc"));
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
