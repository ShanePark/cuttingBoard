use crate::models::{
    LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceSnapshot, WorkspaceSnapshot,
};
use std::{
    cmp::Reverse,
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
    // Finding the process's output is left to the caller: it means listing the process's open
    // files, which is far too slow to repeat for every poll and for every caller that only asks
    // whether the task is running.
    Some(ExternalTaskInfo {
        pid: Some(process.pid),
        started_at: Some(process.create_time),
        uid: process.uid,
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
}
