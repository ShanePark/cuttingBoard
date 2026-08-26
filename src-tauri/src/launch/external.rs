use crate::models::{
    LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceSnapshot, WorkspaceSnapshot,
};
use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

#[cfg(unix)]
use std::time::Instant;

#[derive(Debug)]
pub(super) struct ExternalTaskInfo {
    pub(super) pid: Option<u32>,
    pub(super) started_at: Option<u64>,
    pub(super) uid: Option<u32>,
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
    let pid = Some(process.pid);
    let started_at = Some(process.create_time);
    let uid = process.uid;
    let working_directory = process.working_directory.clone();
    let (log_path, log_tail) = external_log_path(process.pid)
        .and_then(|path| {
            super::logs::read_log_tail(&path)
                .ok()
                .map(|tail| (Some(path), tail))
        })
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

pub(super) fn task_matches_service(
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

#[cfg(unix)]
pub(super) fn external_log_path(pid: u32) -> Option<PathBuf> {
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
pub(super) fn external_log_path(_pid: u32) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
pub(super) fn parse_lsof_output_paths(text: &str) -> Vec<(u8, PathBuf)> {
    let mut fd = None;
    let mut paths = Vec::new();
    for line in text.lines() {
        let Some(field) = line.get(..1) else {
            continue;
        };
        let value = &line[1..];
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
pub(super) fn parse_lsof_fd(value: &str) -> Option<u8> {
    let digits = value
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    matches!(digits.as_str(), "1" | "2").then(|| digits.parse().expect("known fd"))
}

pub(super) fn first_readable_external_log_path(mut paths: Vec<(u8, PathBuf)>) -> Option<PathBuf> {
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
}
