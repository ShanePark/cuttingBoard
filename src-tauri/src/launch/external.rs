use crate::models::{
    LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceSnapshot, WorkspaceSnapshot,
};
use std::{
    cmp::Reverse,
    collections::HashSet,
    fs::{self, OpenOptions},
    io,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

#[cfg(unix)]
use std::time::Instant;

#[derive(Debug)]
pub(super) struct ExternalTaskInfo {
    pub(super) pid: Option<u32>,
    pub(super) started_at: Option<u64>,
    pub(super) uid: Option<u32>,
    pub(super) expected_port: Option<u16>,
    pub(super) endpoint_addresses: Vec<IpAddr>,
    pub(super) working_directory: Option<String>,
    pub(super) log_path: Option<PathBuf>,
    pub(super) log_tail: String,
}

pub(super) fn external_task_info(
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
    // Finding the process's output is left to the caller: it means listing the process's open
    // files, which is far too slow to repeat for every poll and for every caller that only asks
    // whether the task is running.
    Some(ExternalTaskInfo {
        pid: Some(process.pid),
        started_at: Some(process.create_time),
        uid: process.uid,
        expected_port: task.expected_port,
        endpoint_addresses: task
            .expected_port
            .into_iter()
            .flat_map(|port| {
                service
                    .endpoints
                    .iter()
                    .filter(move |endpoint| endpoint.port == port)
            })
            .filter_map(|endpoint| endpoint.address.parse().ok())
            .collect(),
        working_directory: process.working_directory.clone(),
        log_path: None,
        log_tail: String::new(),
    })
}

pub(super) fn external_snapshot(
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
        exit_code: None,
        message: None,
        log_tail: external.log_tail,
        external_pid: external.pid,
        external_working_directory: external.working_directory,
        external_log_path: external
            .log_path
            .map(|path| path.to_string_lossy().into_owned()),
    }
}

pub(super) fn stop_external_task(
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
    let tree = external_process_tree(pid, started_at, external.uid)?;
    signal_external_tree(&tree, external_term_signal())?;
    if !wait_for_external_tree_exit(&tree, Duration::from_secs(2))? {
        signal_external_tree(&tree, external_kill_signal())?;
        if !wait_for_external_tree_exit(&tree, Duration::from_secs(1))? {
            return Err(format!(
                "{task_name} and its known child processes did not stop."
            ));
        }
    }

    if let Some(port) = external.expected_port {
        wait_for_port_release(task_name, port, &external.endpoint_addresses)?;
    }
    Ok(external_stopped_snapshot(
        profile_id,
        task_name,
        started_at,
        external.log_tail,
    ))
}

#[derive(Debug, Clone)]
struct ExternalProcessIdentity {
    pid: u32,
    started_at: u64,
    uid: Option<u32>,
    depth: usize,
}

#[derive(Debug, Clone)]
struct ProcessTreeCandidate {
    identity: ExternalProcessIdentity,
    parent_pid: Option<u32>,
    status: ProcessStatus,
}

fn external_process_tree(
    root_pid: u32,
    root_started_at: u64,
    root_uid: Option<u32>,
) -> Result<Vec<ExternalProcessIdentity>, String> {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All, true);
    let root = system
        .process(Pid::from_u32(root_pid))
        .ok_or_else(|| "The process already exited. Refresh and try again.".to_string())?;
    if root.start_time() != root_started_at {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }

    let root_process_uid = root.user_id().map(|uid| **uid as u32);
    let current_uid = effective_uid();
    if (root_uid.is_some() && current_uid.is_some() && root_uid != current_uid)
        || (root_process_uid.is_some() && current_uid.is_some() && root_process_uid != current_uid)
    {
        return Err("Cutting Board only stops processes owned by the current user.".into());
    }
    let expected_uid = current_uid
        .or(root_uid)
        .or(root_process_uid)
        .ok_or_else(|| "Cutting Board could not verify the service process owner.".to_string())?;

    let candidates = system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessTreeCandidate {
            identity: ExternalProcessIdentity {
                pid: pid.as_u32(),
                started_at: process.start_time(),
                uid: process.user_id().map(|uid| **uid as u32),
                depth: 0,
            },
            parent_pid: process.parent().map(|parent| parent.as_u32()),
            status: process.status(),
        })
        .collect::<Vec<_>>();
    let mut tree = vec![ExternalProcessIdentity {
        pid: root_pid,
        started_at: root_started_at,
        uid: root_process_uid.or(Some(expected_uid)),
        depth: 0,
    }];
    let mut visited = HashSet::from([root_pid]);
    let mut cursor = 0;
    while cursor < tree.len() {
        let parent_pid = tree[cursor].pid;
        let child_depth = tree[cursor].depth + 1;
        cursor += 1;
        let children = candidates
            .iter()
            .filter(|candidate| {
                candidate.parent_pid == Some(parent_pid)
                    && !matches!(
                        candidate.status,
                        ProcessStatus::Zombie | ProcessStatus::Dead
                    )
                    && !visited.contains(&candidate.identity.pid)
            })
            .cloned()
            .collect::<Vec<_>>();
        for mut child in children {
            if child.identity.uid != Some(expected_uid) {
                return Err(format!(
                    "Cutting Board could not safely stop child PID {} because its owner could not be verified.",
                    child.identity.pid
                ));
            }
            visited.insert(child.identity.pid);
            child.identity.depth = child_depth;
            tree.push(child.identity);
        }
    }
    // Stop the root first so it cannot create new descendants while captured children stop.
    tree.sort_by_key(|process| (process.depth != 0, Reverse(process.depth)));
    Ok(tree)
}

fn signal_external_tree(
    tree: &[ExternalProcessIdentity],
    signal: ExternalSignal,
) -> Result<(), String> {
    for process in tree {
        if !external_process_identity_is_current(process)? {
            continue;
        }
        send_external_signal(process.pid, signal)?;
    }
    Ok(())
}

#[cfg(unix)]
type ExternalSignal = i32;

#[cfg(not(unix))]
type ExternalSignal = ();

fn wait_for_external_tree_exit(
    tree: &[ExternalProcessIdentity],
    timeout: Duration,
) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !tree.iter().try_fold(false, |alive, process| {
            Ok::<_, String>(alive || external_process_identity_is_live(process))
        })? {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(80));
    }
}

fn external_process_identity_is_current(
    identity: &ExternalProcessIdentity,
) -> Result<bool, String> {
    let state = current_process_state(identity.pid);
    if !process_identity_is_live(identity, state.as_ref()) {
        return Ok(false);
    }
    let uid = state.and_then(|(_, _, uid)| uid);
    if identity.uid.is_some() && effective_uid().is_some() && uid != identity.uid {
        return Err(format!(
            "Cutting Board refused to signal PID {} after its owner changed.",
            identity.pid
        ));
    }
    Ok(true)
}

fn external_process_identity_is_live(identity: &ExternalProcessIdentity) -> bool {
    process_identity_is_live(identity, current_process_state(identity.pid).as_ref())
}

fn process_identity_is_live(
    identity: &ExternalProcessIdentity,
    state: Option<&(u64, ProcessStatus, Option<u32>)>,
) -> bool {
    let Some((started_at, status, _)) = state else {
        return false;
    };
    *started_at == identity.started_at
        && !matches!(status, ProcessStatus::Zombie | ProcessStatus::Dead)
}

fn wait_for_port_release(task_name: &str, port: u16, endpoints: &[IpAddr]) -> Result<(), String> {
    const PORT_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);
    const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
    let addresses = if endpoints.is_empty() {
        vec![
            IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ]
    } else {
        endpoints.to_vec()
    };
    let deadline = std::time::Instant::now() + PORT_RELEASE_TIMEOUT;
    loop {
        let mut occupied = false;
        for mut address in addresses.iter().copied() {
            if address.is_unspecified() {
                address = match address {
                    IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                    IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                };
            }
            match TcpStream::connect_timeout(&SocketAddr::new(address, port), CONNECT_TIMEOUT) {
                Ok(stream) => {
                    drop(stream);
                    occupied = true;
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    match TcpListener::bind(SocketAddr::new(address, port)) {
                        Ok(listener) => drop(listener),
                        Err(bind_error) if bind_error.kind() == io::ErrorKind::AddrInUse => {
                            occupied = true;
                        }
                        Err(bind_error) => {
                            return Err(format!(
                                "Could not verify that {task_name} released port {port}: {bind_error}"
                            ));
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::TimedOut => occupied = true,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
                    ) && !address_family_available(address) => {}
                Err(error) => {
                    return Err(format!(
                        "Could not verify that {task_name} released port {port}: {error}"
                    ));
                }
            }
        }
        if !occupied {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "{task_name} stopped, but port {port} is still in use."
            ));
        }
        thread::sleep(Duration::from_millis(80));
    }
}

fn address_family_available(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(_) => true,
        IpAddr::V6(_) => TcpListener::bind(SocketAddr::new(
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            0,
        ))
        .is_ok(),
    }
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
        exit_code: None,
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
    let (actual_start_time, status, actual_uid) = current_process_state(pid)
        .ok_or_else(|| "The process already exited. Refresh and try again.".to_string())?;
    if actual_start_time != started_at {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }
    if matches!(status, ProcessStatus::Zombie | ProcessStatus::Dead) {
        return Err("The process already exited. Refresh and try again.".into());
    }
    if uid.is_some() && effective_uid().is_some() && actual_uid != uid {
        return Err("Cutting Board only stops processes owned by the current user.".into());
    }
    Ok(())
}

fn current_process_state(pid: u32) -> Option<(u64, ProcessStatus, Option<u32>)> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing()
            .with_user(UpdateKind::Always)
            .with_tasks(),
    );
    system.process(pid).map(|process| {
        (
            process.start_time(),
            process.status(),
            process.user_id().map(|uid| **uid as u32),
        )
    })
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
fn send_external_signal(pid: u32, signal: ExternalSignal) -> Result<(), String> {
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
fn send_external_signal(pid: u32, _signal: ExternalSignal) -> Result<(), String> {
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

pub(super) fn task_matches_service(
    profile: &LaunchProfile,
    task: &LaunchTask,
    service: &ServiceSnapshot,
) -> bool {
    // A container task is answered by Docker, never by the process that publishes its port: the
    // listening process belongs to the daemon, so signalling it would be the wrong target.
    if task.container_name().is_some() {
        return false;
    }
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
    let task_root = super::task_cwd(profile, task)
        .canonicalize()
        .unwrap_or_else(|_| super::task_cwd(profile, task));
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

pub(super) fn service_belongs_to_runtime(
    service: &ServiceSnapshot,
    runtime_pid: Option<u32>,
) -> bool {
    let Some(process) = service.process.as_ref() else {
        return false;
    };
    let Some(runtime_pid) = runtime_pid else {
        return true;
    };
    if process.pid == runtime_pid {
        return true;
    }

    let mut system = System::new();
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
        let pid = Pid::from_u32(pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        parent_pid = system
            .process(pid)
            .and_then(|process| process.parent())
            .map(|pid| pid.as_u32());
    }
    false
}

/// A file the process writes its output to: stdout or stderr redirected to a regular file, or
/// failing that the log file it wrote to most recently. That second case is how a Spring Boot
/// app with `logging.file.name`, or any app with a file appender, exposes output that otherwise
/// only reaches the IDE or terminal that started it.
#[cfg(unix)]
pub(super) fn external_log_path(pid: u32) -> Option<PathBuf> {
    const TIMEOUT: Duration = Duration::from_secs(2);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    // `-b` skips the blocking stat of every mount point, which costs about a second on a machine
    // with Docker volumes, and `-w` silences the warnings that skip would print.
    let mut child = Command::new("lsof")
        .args(["-b", "-w", "-nP", "-a", "-p", &pid.to_string(), "-Ffatn"])
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
    preferred_log_file(parse_lsof_open_files(&text))
}

#[cfg(not(unix))]
pub(super) fn external_log_path(_pid: u32) -> Option<PathBuf> {
    None
}

/// One numbered descriptor from `lsof -Ffatn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenFile {
    pub(super) fd: u32,
    pub(super) writable: bool,
    /// False once lsof reported a type other than a regular file; the path is checked again on
    /// disk before it is used.
    pub(super) regular: bool,
    pub(super) path: PathBuf,
}

#[cfg(unix)]
pub(super) fn parse_lsof_open_files(text: &str) -> Vec<OpenFile> {
    let mut files = Vec::new();
    let mut current: Option<OpenFile> = None;
    for line in text.lines() {
        let Some(field) = line.get(..1) else {
            continue;
        };
        let value = &line[1..];
        match field {
            "f" => {
                files.extend(current.take().filter(OpenFile::has_path));
                // Named descriptors such as cwd, txt and mem are not output targets.
                current = parse_lsof_fd(value).map(|fd| OpenFile {
                    fd,
                    writable: false,
                    regular: true,
                    path: PathBuf::new(),
                });
            }
            "a" => {
                if let Some(file) = current.as_mut() {
                    file.writable = matches!(value.trim(), "w" | "u");
                }
            }
            "t" => {
                if let Some(file) = current.as_mut() {
                    file.regular = value.trim() == "REG";
                }
            }
            "n" => {
                if let Some(file) = current.as_mut() {
                    file.path = PathBuf::from(value);
                }
            }
            _ => {}
        }
    }
    files.extend(current.filter(OpenFile::has_path));
    files
}

impl OpenFile {
    fn has_path(&self) -> bool {
        !self.path.as_os_str().is_empty()
    }
}

#[cfg(unix)]
pub(super) fn parse_lsof_fd(value: &str) -> Option<u32> {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn is_standard_output(fd: u32) -> bool {
    matches!(fd, 1 | 2)
}

/// Whether an open file is plausibly a log rather than data the process keeps: `app.log`,
/// `app.log.1`, `nohup.out`, or a text file inside a `log` or `logs` directory such as a Tomcat
/// access log.
pub(super) fn looks_like_log_file(path: &Path) -> bool {
    let Some(name) = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
    else {
        return false;
    };
    if name.contains(".log") || name.ends_with(".out") {
        return true;
    }
    let in_log_directory = path
        .parent()
        .and_then(Path::file_name)
        .map(|directory| directory.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|directory| directory == "log" || directory == "logs");
    in_log_directory && (name.ends_with(".txt") || name.contains("log"))
}

/// The output file to read: stdout before stderr, then the log file written most recently.
pub(super) fn preferred_log_file(files: Vec<OpenFile>) -> Option<PathBuf> {
    let mut candidates = files
        .into_iter()
        .filter(|file| {
            file.regular
                && (is_standard_output(file.fd)
                    || (file.writable && looks_like_log_file(&file.path)))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_cached_key(|file| {
        let modified = if is_standard_output(file.fd) {
            None
        } else {
            fs::metadata(&file.path)
                .and_then(|metadata| metadata.modified())
                .ok()
        };
        (!is_standard_output(file.fd), Reverse(modified), file.fd)
    });
    let mut seen: Vec<PathBuf> = Vec::new();
    for OpenFile { path, .. } in candidates {
        let comparison_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.contains(&comparison_path) {
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

pub(super) fn path_matches_task(candidate: &str, task_root: &Path) -> bool {
    if candidate.trim().is_empty() {
        return false;
    }
    let candidate = Path::new(candidate)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(candidate));
    candidate == task_root || candidate.starts_with(task_root) || task_root.starts_with(&candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn open_file(fd: u32, writable: bool, path: &Path) -> OpenFile {
        OpenFile {
            fd,
            writable,
            regular: true,
            path: path.to_path_buf(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn lsof_parser_reads_numbered_descriptors_with_access_and_type() {
        let output = "p42\nfcwd\na \ntDIR\nn/srv/app\nf1\naw\ntREG\nn/srv/app/stdout.log\nf7\nau\ntREG\nn/srv/app/logs/app.log\nf8\nar\ntCHR\nn/dev/null\n";

        assert_eq!(
            parse_lsof_open_files(output),
            vec![
                open_file(1, true, Path::new("/srv/app/stdout.log")),
                open_file(7, true, Path::new("/srv/app/logs/app.log")),
                OpenFile {
                    fd: 8,
                    writable: false,
                    regular: false,
                    path: "/dev/null".into(),
                },
            ]
        );
        assert_eq!(parse_lsof_fd("10w"), Some(10));
        assert_eq!(parse_lsof_fd("cwd"), None);
    }

    #[cfg(unix)]
    #[test]
    fn preferred_log_file_prefers_stdout_and_skips_non_regular_files() {
        let temporary = tempfile::tempdir().unwrap();
        let stdout = temporary.path().join("stdout.log");
        let stderr = temporary.path().join("stderr.log");
        fs::write(&stdout, "stdout").unwrap();
        fs::write(&stderr, "stderr").unwrap();
        let output = format!(
            "f2\naw\ntDIR\nn{}\nf1\naw\ntREG\nn{}\nf2\naw\ntREG\nn{}\n",
            temporary.path().display(),
            stdout.display(),
            stderr.display()
        );

        assert_eq!(
            preferred_log_file(parse_lsof_open_files(&output)),
            Some(stdout)
        );
        assert_eq!(
            preferred_log_file(vec![
                open_file(1, true, temporary.path()),
                open_file(2, true, &stderr),
            ]),
            Some(stderr)
        );
    }

    #[test]
    fn preferred_log_file_falls_back_to_the_newest_writable_log_file() {
        let temporary = tempfile::tempdir().unwrap();
        let older = temporary.path().join("app.log");
        let newer = temporary.path().join("spring.log");
        let read_only = temporary.path().join("other.log");
        let data = temporary.path().join("cache.db");
        for path in [&older, &newer, &read_only, &data] {
            fs::write(path, "contents").unwrap();
        }
        let hour_ago = SystemTime::now() - Duration::from_secs(60 * 60);
        OpenOptions::new()
            .write(true)
            .open(&older)
            .unwrap()
            .set_modified(hour_ago)
            .unwrap();
        let files = vec![
            open_file(3, true, &data),
            open_file(4, true, &older),
            open_file(5, false, &read_only),
            open_file(12, true, &newer),
        ];

        assert_eq!(preferred_log_file(files), Some(newer));
        assert_eq!(preferred_log_file(vec![open_file(3, true, &data)]), None);
    }

    #[test]
    fn log_file_names_are_recognised() {
        assert!(looks_like_log_file(Path::new("/srv/app/logs/app.log")));
        assert!(looks_like_log_file(Path::new(
            "/srv/app/app.log.2026-09-02"
        )));
        assert!(looks_like_log_file(Path::new("/srv/app/nohup.out")));
        assert!(looks_like_log_file(Path::new(
            "/srv/tomcat/logs/localhost_access_log.2026-09-02.txt"
        )));
        assert!(!looks_like_log_file(Path::new("/srv/app/catalog.db")));
        assert!(!looks_like_log_file(Path::new("/srv/app/data/output.txt")));
    }

    #[test]
    fn port_release_check_rejects_a_residual_listener() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();

        let error = wait_for_port_release(
            "dummy task",
            port,
            &[IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)],
        )
        .unwrap_err();

        assert!(error.contains("is still in use"));
        drop(listener);
    }

    #[test]
    fn process_liveness_wait_does_not_require_owner_metadata() {
        let identity = ExternalProcessIdentity {
            pid: 42,
            started_at: 123,
            uid: Some(1000),
            depth: 0,
        };

        assert!(process_identity_is_live(
            &identity,
            Some(&(123, ProcessStatus::Sleep, None))
        ));
        assert!(!process_identity_is_live(
            &identity,
            Some(&(123, ProcessStatus::Zombie, None))
        ));
        assert!(!process_identity_is_live(
            &identity,
            Some(&(124, ProcessStatus::Sleep, None))
        ));
    }
}
