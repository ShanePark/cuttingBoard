use crate::models::{LaunchProfile, LaunchTask, PrepareState, ServiceSnapshot};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

use super::external::{path_matches_task, task_matches_service};

/// Where a task's output comes from when Cutting Board did not start the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogSourceKind {
    /// A file the process itself writes to.
    Process,
    /// A Cutting Board task log left by an earlier session that started the process.
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogSource {
    pub(super) kind: LogSourceKind,
    pub(super) path: PathBuf,
}

/// Remembers, per process, where its output was found. Finding it means listing the process's
/// open files and scanning task logs, which is far too slow to repeat for every poll.
#[derive(Debug, Default)]
pub(super) struct LogSourceCache {
    entries: HashMap<(u32, u64), CachedLogSource>,
}

#[derive(Debug)]
struct CachedLogSource {
    source: Option<LogSource>,
    probed_at: Instant,
    used_at: Instant,
}

/// A process usually opens its log file right after it starts, so a fruitless search is repeated
/// after a short wait rather than on every poll.
const LOG_SOURCE_RETRY_AFTER: Duration = Duration::from_secs(15);
/// An entry nobody has asked about for this long belongs to a process that has gone away.
const LOG_SOURCE_EXPIRE_AFTER: Duration = Duration::from_secs(10 * 60);

impl LogSourceCache {
    /// The remembered source for the process, or the result of `probe` when there is none, the
    /// remembered file has disappeared, or the last fruitless search is old enough to repeat.
    pub(super) fn resolve(
        &mut self,
        pid: u32,
        started_at: u64,
        probe: impl FnOnce() -> Option<LogSource>,
    ) -> Option<LogSource> {
        let now = Instant::now();
        self.entries.retain(|_, entry| {
            now.saturating_duration_since(entry.used_at) < LOG_SOURCE_EXPIRE_AFTER
        });
        let key = (pid, started_at);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used_at = now;
            match &entry.source {
                Some(source)
                    if fs::metadata(&source.path).is_ok_and(|metadata| metadata.is_file()) =>
                {
                    return Some(source.clone());
                }
                None if now.saturating_duration_since(entry.probed_at) < LOG_SOURCE_RETRY_AFTER => {
                    return None;
                }
                _ => {}
            }
        }
        let source = probe();
        self.entries.insert(
            key,
            CachedLogSource {
                source: source.clone(),
                probed_at: now,
                used_at: now,
            },
        );
        source
    }

    #[cfg(test)]
    fn age_probe(&mut self, pid: u32, started_at: u64, by: Duration) {
        if let Some(entry) = self.entries.get_mut(&(pid, started_at)) {
            entry.probed_at = entry.probed_at.checked_sub(by).unwrap_or(entry.probed_at);
        }
    }
}

/// The Cutting Board task log of an earlier session that started the process behind this task.
pub(super) fn recovered_external_task_log_path(
    logs_dir: &Path,
    profiles: &[LaunchProfile],
    profile: &LaunchProfile,
    task: &LaunchTask,
    workspace: Option<&crate::models::WorkspaceSnapshot>,
    process_start: u64,
) -> Option<PathBuf> {
    let service = workspace.and_then(|snapshot| {
        snapshot
            .services
            .iter()
            .find(|service| task_matches_service(profile, task, service))
    })?;
    recovered_managed_log_path(logs_dir, profiles, service, process_start)
}

const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// The last bytes of the file, and whether anything before them was left out.
fn read_tail_bytes(path: &Path) -> io::Result<(Vec<u8>, bool)> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let start = file.metadata()?.len().saturating_sub(LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok((bytes, start > 0))
}

pub(super) fn read_log_tail(path: &Path) -> io::Result<String> {
    let (bytes, _) = read_tail_bytes(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// A task's log tail for the control socket, with what it needs to find the current run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskLog {
    pub text: String,
    pub truncated: bool,
    /// Whether this is a Cutting Board task log, whose start markers separate runs.
    pub has_run_markers: bool,
    /// When a process Cutting Board does not manage is behind the log, its start time.
    pub process_started_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunBoundary {
    StartMarker,
    /// The marker has scrolled out of the tail, so the whole tail belongs to the current run.
    LogTail,
    /// The log shows no run at all.
    None,
}

impl RunBoundary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StartMarker => "start_marker",
            Self::LogTail => "log_tail",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentRun<'a> {
    pub text: &'a str,
    pub started_at: Option<u64>,
    pub boundary: RunBoundary,
    pub truncated: bool,
}

impl TaskLog {
    /// The part of the log written since the last start. `None` means the log cannot tell runs
    /// apart, so nothing in it can be attributed to the process that is running now.
    pub fn current_run(&self) -> Option<CurrentRun<'_>> {
        if !self.has_run_markers {
            return None;
        }
        let mut offset = 0;
        let mut last_start = None;
        for line in self.text.split_inclusive('\n') {
            if let Some(marker) = parse_log_start_marker(line) {
                last_start = Some((offset, marker.started_at));
            }
            offset += line.len();
        }
        match last_start {
            Some((_, started_at))
                if self.process_started_at.is_some_and(|process_start| {
                    process_start.abs_diff(started_at) > LOG_RECOVERY_START_SKEW_SECS
                }) =>
            {
                None
            }
            Some((offset, started_at)) => Some(CurrentRun {
                text: &self.text[offset..],
                started_at: Some(started_at),
                boundary: RunBoundary::StartMarker,
                truncated: false,
            }),
            None if self.truncated => Some(CurrentRun {
                text: &self.text,
                started_at: None,
                boundary: RunBoundary::LogTail,
                truncated: true,
            }),
            None => Some(CurrentRun {
                text: "",
                started_at: None,
                boundary: RunBoundary::None,
                truncated: false,
            }),
        }
    }
}

/// Reads the tail like `read_log_tail`, without the line the cut falls in. Output filters match
/// whole lines, so the end of a marker cut in half would otherwise pass them with the rest of its
/// command line. Also reports whether anything was cut.
pub(super) fn read_complete_log_tail(path: &Path) -> io::Result<(String, bool)> {
    let (mut bytes, truncated) = read_tail_bytes(path)?;
    if truncated {
        let line_end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1);
        bytes.drain(..line_end);
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

/// The latest preparation in a task log. A preparation with no outcome yet reads as `Running`,
/// and only the caller knows whether one is still in flight.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareRun {
    pub state: PrepareState,
    pub started_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub failure: Option<String>,
    pub text: String,
    pub truncated: bool,
}

const PREPARE_MARKER_PREFIX: &[u8] = b"=== Cutting Board prepare ";

/// Build output need not be UTF-8, so lines are only decoded once they look like a marker. A
/// failure can scroll out of the tail behind a chatty old process, so this looks further back.
pub(super) fn read_prepare_run(path: &Path) -> io::Result<PrepareRun> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(PrepareRun::default()),
        Err(error) => return Err(error),
    };
    let scan_start = file.metadata()?.len().saturating_sub(LOG_MARKER_SCAN_BYTES);
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(scan_start))?;
    let mut line = Vec::new();
    let mut offset = scan_start;
    if scan_start > 0 {
        offset += reader.read_until(b'\n', &mut line)? as u64;
    }
    let scanned_from = offset;

    let mut run = PrepareRun::default();
    let mut range = None;
    loop {
        line.clear();
        let bytes = reader.read_until(b'\n', &mut line)? as u64;
        if bytes == 0 {
            break;
        }
        if let Some(marker) = parse_prepare_marker(&line) {
            match marker.state {
                PrepareState::Running => {
                    run = PrepareRun {
                        state: PrepareState::Running,
                        started_at: marker.at,
                        ..Default::default()
                    };
                    range = Some((offset, None));
                }
                state => {
                    // Its start has scrolled out of what was scanned.
                    let start = range.map_or(scanned_from, |(start, _)| start);
                    run.truncated |= range.is_none();
                    run.state = state;
                    run.completed_at = marker.at;
                    run.failure = marker.failure;
                    range = Some((start, Some(offset)));
                }
            }
        }
        offset += bytes;
    }

    if let Some((start, end)) = range {
        let end = end.unwrap_or(offset);
        let from = start.max(end.saturating_sub(LOG_TAIL_BYTES));
        reader.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        reader.take(end - from).read_to_end(&mut bytes)?;
        if from > start {
            let line_end = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            bytes.drain(..line_end);
            run.truncated = true;
        }
        run.text = String::from_utf8_lossy(&bytes).into_owned();
    }
    Ok(run)
}

#[derive(Debug, PartialEq, Eq)]
struct PrepareMarker {
    /// `Running` for the marker that starts a preparation.
    state: PrepareState,
    at: Option<u64>,
    failure: Option<String>,
}

fn parse_prepare_marker(line: &[u8]) -> Option<PrepareMarker> {
    if !line.starts_with(PREPARE_MARKER_PREFIX) {
        return None;
    }
    let line = String::from_utf8_lossy(line);
    let body = line
        .trim_end_matches(['\r', '\n'])
        .strip_prefix("=== Cutting Board prepare ")?
        .strip_suffix(" ===")?;
    // Logs written before markers carried a time have none.
    let (body, at) = match body.rsplit_once(" · at=") {
        Some((body, at)) => (body, at.parse().ok()),
        None => (body, None),
    };
    let mut failure = None;
    let state = if body.starts_with("detection started") {
        PrepareState::Running
    } else if body.starts_with("not required") {
        PrepareState::NotRequired
    } else if body.starts_with("skipped") {
        PrepareState::Skipped
    } else if body.starts_with("completed") {
        PrepareState::Succeeded
    } else if let Some(reason) = body.strip_prefix("failed · ") {
        failure = Some(reason.to_owned());
        PrepareState::Failed
    } else {
        return None;
    };
    Some(PrepareMarker { state, at, failure })
}

pub(super) const LOG_RECOVERY_START_SKEW_SECS: u64 = 15 * 60;
pub(super) const LOG_MARKER_SCAN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
struct LogStartMarker {
    started_at: u64,
    command: String,
    profile_id: Option<String>,
    task_name: Option<String>,
    cwd: Option<String>,
}

pub(super) fn recovered_managed_log_path(
    logs_dir: &Path,
    profiles: &[LaunchProfile],
    service: &ServiceSnapshot,
    process_start: u64,
) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for profile in profiles {
        for task in &profile.tasks {
            if task_matches_service(profile, task, service) {
                candidates.push((
                    profile,
                    task,
                    format!("{}-{}.log", safe_name(&profile.id), safe_name(&task.name)),
                    format!("-{}.log", safe_name(&task.name)),
                ));
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }

    let mut matches = Vec::new();
    let process_commands = process_command_provenance(service);
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return None;
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
        if !candidates.iter().any(|(_, _, expected_name, suffix)| {
            file_name == expected_name.as_str() || file_name.ends_with(suffix)
        }) {
            continue;
        }
        let path = entry.path();
        let Ok(markers) = read_log_start_markers(&path) else {
            continue;
        };
        for (profile, task, expected_name, suffix) in &candidates {
            let expected_path = file_name == expected_name.as_str();
            if !expected_path && !file_name.ends_with(suffix) {
                continue;
            }
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
                        expected_path,
                    )
                })
                .min()
            else {
                continue;
            };
            matches.push((
                rank,
                distance,
                path.clone(),
                profile.id.clone(),
                task.name.clone(),
            ));
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
    let task_root = super::task_cwd(profile, task)
        .canonicalize()
        .unwrap_or_else(|_| super::task_cwd(profile, task));
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
    let refresh_kind = ProcessRefreshKind::nothing()
        .without_tasks()
        .with_cmd(UpdateKind::OnlyIfNotSet);
    // Log recovery only needs command lines along this process's ancestor chain.
    let mut system = System::new_with_specifics(RefreshKind::nothing());
    let mut pid = Some(Pid::from_u32(process.pid));
    let mut visited = Vec::new();
    let mut commands = Vec::new();
    while let Some(current_pid) = pid {
        if visited.contains(&current_pid) {
            break;
        }
        visited.push(current_pid);
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[current_pid]),
            true,
            refresh_kind,
        );
        let Some((command, parent)) = system.process(current_pid).map(|current| {
            let command = current
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            (command, current.parent())
        }) else {
            break;
        };
        if !command.is_empty() {
            commands.push(command);
        }
        pid = parent;
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

pub(super) fn safe_name(value: &str) -> String {
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
    use crate::models::{Endpoint, ProjectInfo, WorkspaceSnapshot};

    fn workspace_with_project(port: u16, project_root: &Path) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            services: vec![ServiceSnapshot {
                id: "service".into(),
                display_name: "frontend".into(),
                tech: "vite".into(),
                category: "web".into(),
                relevance: "dev".into(),
                endpoints: vec![Endpoint {
                    family: "IPv4".into(),
                    address: "127.0.0.1".into(),
                    port,
                    scope: "loopback".into(),
                    protocol: "TCP".into(),
                }],
                process: None,
                project: Some(ProjectInfo {
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
                container: None,
                prepare: None,
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
                container: None,
                prepare: None,
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
                container: None,
                prepare: None,
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
                container: None,
                prepare: None,
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
    fn recovered_log_preserves_ambiguous_duplicate_behavior() {
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
                container: None,
                prepare: None,
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        let marker = "=== Cutting Board start 1000 · npm run dev ===\n";
        fs::write(logs_dir.join("current-profile-frontend.log"), marker).unwrap();
        fs::write(logs_dir.join("legacy-profile-frontend.log"), marker).unwrap();
        fs::create_dir(logs_dir.join("archived-frontend.log")).unwrap();

        let expected = legacy_recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );
        let actual = recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );

        assert_eq!(expected, None);
        assert_eq!(actual, expected);
    }

    #[test]
    fn recovered_log_preserves_missing_directory_behavior() {
        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        fs::create_dir(&frontend).unwrap();
        let profile = LaunchProfile {
            id: "current-profile".into(),
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
        let logs_dir = temporary.path().join("missing-logs");

        let expected = legacy_recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );
        let actual = recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );

        assert_eq!(expected, None);
        assert_eq!(actual, expected);
    }

    #[test]
    fn recovered_log_skips_expensive_work_without_matching_profiles() {
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
                expected_port: Some(5174),
                container: None,
                prepare: None,
            }],
        };
        let workspace = workspace_with_project(5173, &frontend);
        fs::write(
            logs_dir.join("current-profile-frontend.log"),
            "=== Cutting Board start 1000 · npm run dev ===\n",
        )
        .unwrap();

        let expected = legacy_recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );
        let actual = recovered_managed_log_path(
            &logs_dir,
            std::slice::from_ref(&profile),
            &workspace.services[0],
            1000,
        );

        assert_eq!(expected, None);
        assert_eq!(actual, expected);
    }

    fn legacy_recovered_managed_log_path(
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
                let expected_name =
                    format!("{}-{}.log", safe_name(&profile.id), safe_name(&task.name));
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

    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_recovered_managed_log_path_reuses_directory_and_markers() {
        use std::fmt::Write as _;

        fn measure_pair(
            iterations: usize,
            mut baseline_call: impl FnMut() -> Option<PathBuf>,
            mut targeted_call: impl FnMut() -> Option<PathBuf>,
        ) -> (Vec<Duration>, Vec<Duration>) {
            let mut baseline = Vec::with_capacity(iterations);
            let mut targeted = Vec::with_capacity(iterations);
            for iteration in 0..iterations {
                if iteration % 2 == 0 {
                    let started_at = Instant::now();
                    let expected = baseline_call();
                    baseline.push(started_at.elapsed());

                    let started_at = Instant::now();
                    let actual = targeted_call();
                    targeted.push(started_at.elapsed());
                    assert_eq!(actual, expected);
                } else {
                    let started_at = Instant::now();
                    let expected = targeted_call();
                    targeted.push(started_at.elapsed());

                    let started_at = Instant::now();
                    let actual = baseline_call();
                    baseline.push(started_at.elapsed());
                    assert_eq!(actual, expected);
                }
            }
            baseline.sort();
            targeted.sort();
            (baseline, targeted)
        }

        let temporary = tempfile::tempdir().unwrap();
        let frontend = temporary.path().join("frontend");
        let logs_dir = temporary.path().join("logs");
        fs::create_dir(&frontend).unwrap();
        fs::create_dir(&logs_dir).unwrap();
        let profiles = (0..8)
            .map(|index| LaunchProfile {
                id: format!("profile-{index}"),
                name: format!("Profile {index}"),
                project_root: temporary.path().to_string_lossy().into_owned(),
                tasks: vec![LaunchTask {
                    name: "frontend".into(),
                    cwd: "frontend".into(),
                    command: "npm run dev".into(),
                    expected_port: Some(5173),
                    container: None,
                    prepare: None,
                }],
            })
            .collect::<Vec<_>>();
        let service = workspace_with_project(5173, &frontend).services[0].clone();
        let process_start = 4_000_000;
        let filler = "2026-09-12 info: build output line\n".repeat(32_768);
        for index in 0..8 {
            let mut contents = filler.clone();
            let started_at = process_start - index * 60;
            writeln!(
                contents,
                "=== Cutting Board start {started_at} · npm run dev ==="
            )
            .unwrap();
            writeln!(
                contents,
                "=== Cutting Board task metadata {} ===",
                serde_json::json!({
                    "profile_id": format!("profile-{index}"),
                    "task_name": "frontend",
                    "cwd": frontend.to_string_lossy(),
                })
            )
            .unwrap();
            fs::write(
                logs_dir.join(format!("profile-{index}-frontend.log")),
                contents,
            )
            .unwrap();
        }
        fs::create_dir(logs_dir.join("archived-frontend.log")).unwrap();

        let expected =
            legacy_recovered_managed_log_path(&logs_dir, &profiles, &service, process_start);
        let actual = recovered_managed_log_path(&logs_dir, &profiles, &service, process_start);
        assert_eq!(actual, expected);

        let (baseline, targeted) = measure_pair(
            21,
            || legacy_recovered_managed_log_path(&logs_dir, &profiles, &service, process_start),
            || recovered_managed_log_path(&logs_dir, &profiles, &service, process_start),
        );
        eprintln!(
            "recovered_managed_log_path benchmark n=21 files=8 log_bytes={} : full median={:?}, p95={:?}; targeted median={:?}, p95={:?}",
            filler.len(),
            baseline[10],
            baseline[19],
            targeted[10],
            targeted[19]
        );

        let single_logs_dir = temporary.path().join("single-logs");
        fs::create_dir(&single_logs_dir).unwrap();
        fs::copy(
            logs_dir.join("profile-0-frontend.log"),
            single_logs_dir.join("profile-0-frontend.log"),
        )
        .unwrap();
        let single_profiles = profiles[..1].to_vec();
        let (single_baseline, single_targeted) = measure_pair(
            21,
            || {
                legacy_recovered_managed_log_path(
                    &single_logs_dir,
                    &single_profiles,
                    &service,
                    process_start,
                )
            },
            || {
                recovered_managed_log_path(
                    &single_logs_dir,
                    &single_profiles,
                    &service,
                    process_start,
                )
            },
        );
        eprintln!(
            "recovered_managed_log_path common-case n=21 files=1 log_bytes={} : full median={:?}, p95={:?}; targeted median={:?}, p95={:?}",
            filler.len(),
            single_baseline[10],
            single_baseline[19],
            single_targeted[10],
            single_targeted[19]
        );
    }

    #[test]
    fn log_source_cache_reuses_a_found_file_and_forgets_a_deleted_one() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("app.log");
        fs::write(&path, "output").unwrap();
        let source = LogSource {
            kind: LogSourceKind::Process,
            path: path.clone(),
        };
        let mut cache = LogSourceCache::default();
        let mut probes = 0;

        for _ in 0..3 {
            let found = cache.resolve(7, 100, || {
                probes += 1;
                Some(source.clone())
            });
            assert_eq!(found, Some(source.clone()));
        }
        assert_eq!(probes, 1);

        fs::remove_file(&path).unwrap();
        let found = cache.resolve(7, 100, || {
            probes += 1;
            None
        });
        assert_eq!(found, None);
        assert_eq!(probes, 2);
    }

    #[test]
    fn log_source_cache_waits_before_searching_again_after_a_miss() {
        let mut cache = LogSourceCache::default();
        let probes = std::cell::Cell::new(0);
        let resolve = |cache: &mut LogSourceCache, pid, started_at| {
            cache.resolve(pid, started_at, || {
                probes.set(probes.get() + 1);
                None
            })
        };

        for _ in 0..3 {
            assert_eq!(resolve(&mut cache, 7, 100), None);
        }
        assert_eq!(probes.get(), 1);
        // A different start time is a different process, even with a recycled pid.
        assert_eq!(resolve(&mut cache, 7, 200), None);
        assert_eq!(probes.get(), 2);

        cache.age_probe(7, 100, LOG_SOURCE_RETRY_AFTER);
        assert_eq!(resolve(&mut cache, 7, 100), None);
        assert_eq!(probes.get(), 3);
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "manual performance measurement"]
    fn benchmark_process_command_provenance_single_process() {
        fn full_process_command_provenance(service: &ServiceSnapshot) -> Vec<String> {
            let Some(process) = service.process.as_ref() else {
                return Vec::new();
            };
            let refresh_kind = ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cmd(UpdateKind::OnlyIfNotSet);
            let system =
                System::new_with_specifics(RefreshKind::nothing().with_processes(refresh_kind));
            let mut pid = Some(Pid::from_u32(process.pid));
            let mut visited = Vec::new();
            let mut commands = Vec::new();
            while let Some(current_pid) = pid {
                if visited.contains(&current_pid) {
                    break;
                }
                visited.push(current_pid);
                let Some((command, parent)) = system.process(current_pid).map(|current| {
                    let command = current
                        .cmd()
                        .iter()
                        .map(|part| part.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                    (command, current.parent())
                }) else {
                    break;
                };
                if !command.is_empty() {
                    commands.push(command);
                }
                pid = parent;
            }
            commands
        }

        let service = process_service(std::process::id());
        let mut baseline = Vec::with_capacity(101);
        let mut targeted = Vec::with_capacity(101);
        for iteration in 0..101 {
            if iteration % 2 == 0 {
                let started_at = Instant::now();
                let expected = full_process_command_provenance(&service);
                baseline.push(started_at.elapsed());

                let started_at = Instant::now();
                let actual = process_command_provenance(&service);
                targeted.push(started_at.elapsed());
                assert_eq!(actual, expected);
            } else {
                let started_at = Instant::now();
                let expected = process_command_provenance(&service);
                targeted.push(started_at.elapsed());

                let started_at = Instant::now();
                let actual = full_process_command_provenance(&service);
                baseline.push(started_at.elapsed());
                assert_eq!(actual, expected);
            }
        }
        baseline.sort();
        targeted.sort();
        eprintln!(
            "process_command_provenance benchmark n=101: full median={:?}, p95={:?}; targeted median={:?}, p95={:?}",
            baseline[50],
            baseline[95],
            targeted[50],
            targeted[95]
        );
    }

    #[cfg(unix)]
    #[test]
    fn process_command_provenance_keeps_child_command_and_ancestors() {
        use std::{
            io::Read,
            process::{Command, Stdio},
        };

        let mut child = Command::new("sh")
            .args(["-c", "printf ready; sleep 5"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut ready = [0; 5];
        child
            .stdout
            .as_mut()
            .unwrap()
            .read_exact(&mut ready)
            .unwrap();
        assert_eq!(&ready, b"ready");

        let commands = process_command_provenance(&process_service(child.id()));
        let current_process_commands =
            process_command_provenance(&process_service(std::process::id()));
        let _ = child.kill();
        let _ = child.wait();

        assert!(commands.iter().any(|command| command.contains("sleep 5")));
        assert!(current_process_commands
            .first()
            .is_some_and(|parent_command| commands
                .iter()
                .skip(1)
                .any(|command| command == parent_command)));
    }

    #[test]
    fn process_command_provenance_returns_no_commands_without_process() {
        let mut workspace = workspace_with_project(5173, Path::new("/tmp/frontend"));
        let service = workspace.services.pop().unwrap();

        assert!(process_command_provenance(&service).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn process_command_provenance_returns_no_commands_for_missing_process() {
        assert!(process_command_provenance(&process_service(u32::MAX)).is_empty());
    }

    fn process_service(pid: u32) -> ServiceSnapshot {
        use crate::models::ProcessInfo;

        ServiceSnapshot {
            id: "benchmark".into(),
            display_name: "benchmark".into(),
            tech: "unknown".into(),
            category: "runtime".into(),
            relevance: "dev".into(),
            endpoints: Vec::new(),
            process: Some(ProcessInfo {
                pid,
                parent_pid: None,
                name: "benchmark".into(),
                executable: None,
                working_directory: None,
                command: String::new(),
                launch_command: None,
                create_time: 0,
                uptime_seconds: 0,
                cpu_percent: None,
                memory_bytes: None,
                uid: None,
            }),
            project: None,
            status: "healthy".into(),
            warnings: Vec::new(),
            origin_kind: "unknown".into(),
            origin_label: None,
            can_terminate: false,
            browser_url: None,
            active_profiles: Vec::new(),
        }
    }

    fn managed_log(text: &str, truncated: bool) -> TaskLog {
        TaskLog {
            text: text.into(),
            truncated,
            has_run_markers: true,
            process_started_at: None,
        }
    }

    fn read_run(contents: &[u8]) -> PrepareRun {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("task.log");
        fs::write(&path, contents).unwrap();
        read_prepare_run(&path).unwrap()
    }

    const STARTED: &str =
        "=== Cutting Board prepare detection started · task=api · cwd=. · at=100 ===\n";

    #[test]
    fn prepare_run_is_the_latest_attempt_up_to_its_outcome() {
        let log = format!(
            "{STARTED}first build\n=== Cutting Board prepare completed · tool=maven · module=(root) · at=110 ===\n\
             \n=== Cutting Board start 111 · cmd ===\napp output\n\
             === Cutting Board prepare detection started · task=api · cwd=. · at=200 ===\n\
             second build\n\
             === Cutting Board prepare failed · Could not prepare api: mvnw exited with exit status: 1. · at=245 ===\n\
             old process output\n"
        );
        let run = read_run(log.as_bytes());
        assert_eq!(run.state, PrepareState::Failed);
        assert_eq!((run.started_at, run.completed_at), (Some(200), Some(245)));
        assert_eq!(
            run.failure.as_deref(),
            Some("Could not prepare api: mvnw exited with exit status: 1.")
        );
        assert!(run.text.contains("second build"));
        assert!(!run.text.contains("first build"));
        assert!(!run.text.contains("old process output"));
        assert!(!run.truncated);
    }

    #[test]
    fn prepare_run_without_an_outcome_reads_as_running() {
        let run = read_run(format!("{STARTED}compiling\n").as_bytes());
        assert_eq!(run.state, PrepareState::Running);
        assert_eq!((run.started_at, run.completed_at), (Some(100), None));
        assert!(run.text.contains("compiling"));
    }

    #[test]
    fn prepare_run_reports_each_outcome_a_log_can_hold() {
        for (marker, state) in [
            (
                "=== Cutting Board prepare not required · task=api · using saved command · at=101 ===",
                PrepareState::NotRequired,
            ),
            (
                "=== Cutting Board prepare skipped (unchanged inputs) · cache hit · fingerprint=abc · at=101 ===",
                PrepareState::Skipped,
            ),
            (
                "=== Cutting Board prepare completed · tool=gradle · module=(root) · at=101 ===",
                PrepareState::Succeeded,
            ),
        ] {
            let run = read_run(format!("{STARTED}{marker}\n").as_bytes());
            assert_eq!(run.state, state);
            assert_eq!(run.completed_at, Some(101));
            assert_eq!(run.failure, None);
        }
    }

    #[test]
    fn prepare_run_is_none_without_a_log_or_any_marker() {
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(
            read_prepare_run(&temporary.path().join("missing.log")).unwrap(),
            PrepareRun::default()
        );
        assert_eq!(read_run(b"only app output\n"), PrepareRun::default());
    }

    #[test]
    fn prepare_run_reads_markers_written_before_they_carried_a_time() {
        let run = read_run(
            b"=== Cutting Board prepare detection started \xC2\xB7 task=api \xC2\xB7 cwd=. ===\n\
              === Cutting Board prepare failed \xC2\xB7 boom ===\n",
        );
        assert_eq!(run.state, PrepareState::Failed);
        assert_eq!((run.started_at, run.completed_at), (None, None));
        assert_eq!(run.failure.as_deref(), Some("boom"));
    }

    #[test]
    fn prepare_run_survives_output_that_is_not_utf8() {
        let mut log = STARTED.as_bytes().to_vec();
        log.extend_from_slice(b"\xC7\xD1\xB1\xDB build output\n");
        log.extend_from_slice(
            "=== Cutting Board prepare completed · tool=maven · module=(root) · at=120 ===\n"
                .as_bytes(),
        );
        let run = read_run(&log);
        assert_eq!(run.state, PrepareState::Succeeded);
        assert!(run.text.contains("build output"));
    }

    #[test]
    fn prepare_run_keeps_the_end_of_a_long_build_on_whole_lines() {
        let mut log = STARTED.to_owned();
        log.push_str(&"x".repeat(LOG_TAIL_BYTES as usize));
        log.push_str("\n[ERROR] the reason\n");
        log.push_str("=== Cutting Board prepare failed · boom · at=150 ===\n");
        let run = read_run(log.as_bytes());
        assert!(run.truncated);
        assert_eq!(run.text, "[ERROR] the reason\n");
        assert_eq!(run.started_at, Some(100));
    }

    #[test]
    fn prepare_run_finds_a_failure_behind_more_than_a_tail_of_later_output() {
        let mut log = STARTED.to_owned();
        log.push_str("[ERROR] the reason\n");
        log.push_str("=== Cutting Board prepare failed · boom · at=150 ===\n");
        log.push_str(&"old process\n".repeat(LOG_TAIL_BYTES as usize / 4));
        let run = read_run(log.as_bytes());
        assert_eq!(run.state, PrepareState::Failed);
        assert_eq!(run.text.lines().nth(1), Some("[ERROR] the reason"));
        assert!(!run.truncated);
    }

    #[test]
    fn prepare_run_marks_an_outcome_whose_start_scrolled_out_of_the_scan() {
        let mut log = "x\n".repeat(LOG_MARKER_SCAN_BYTES as usize / 2 + 1);
        log.push_str("=== Cutting Board prepare failed · boom · at=150 ===\n");
        let run = read_run(log.as_bytes());
        assert_eq!(run.state, PrepareState::Failed);
        assert_eq!(run.started_at, None);
        assert!(run.truncated);
    }

    #[test]
    fn current_run_starts_at_the_last_start_marker() {
        let log = managed_log(
            "\n=== Cutting Board start 100 · old ===\nStarted old\n\n=== Cutting Board start 200 · new ===\nbooting\n",
            true,
        );
        let run = log.current_run().unwrap();
        assert_eq!(run.started_at, Some(200));
        assert_eq!(run.boundary, RunBoundary::StartMarker);
        assert!(!run.truncated);
        assert_eq!(run.text, "=== Cutting Board start 200 · new ===\nbooting\n");
    }

    #[test]
    fn current_run_only_counts_a_marker_at_the_start_of_a_line() {
        let log = managed_log(
            "=== Cutting Board start 100 · cmd ===\nsaid === Cutting Board start 200 · cmd ===\n",
            false,
        );
        assert_eq!(log.current_run().unwrap().started_at, Some(100));
    }

    #[test]
    fn current_run_is_the_whole_tail_once_the_marker_has_scrolled_out() {
        let log = managed_log("still booting\n", true);
        let run = log.current_run().unwrap();
        assert_eq!(run.boundary, RunBoundary::LogTail);
        assert_eq!(run.started_at, None);
        assert!(run.truncated);
        assert_eq!(run.text, "still booting\n");
    }

    #[test]
    fn current_run_is_empty_for_a_complete_log_without_a_marker() {
        let run = managed_log("", false);
        let run = run.current_run().unwrap();
        assert_eq!(run.boundary, RunBoundary::None);
        assert_eq!(run.text, "");
    }

    #[test]
    fn current_run_is_unknown_without_markers_or_for_another_process() {
        let foreign = TaskLog {
            has_run_markers: false,
            ..managed_log("=== Cutting Board start 100 · cmd ===\n", false)
        };
        assert_eq!(foreign.current_run(), None);

        let marker = "=== Cutting Board start 10000 · cmd ===\n";
        let other_process = TaskLog {
            process_started_at: Some(10_000 + LOG_RECOVERY_START_SKEW_SECS + 1),
            ..managed_log(marker, false)
        };
        assert_eq!(other_process.current_run(), None);
        let same_process = TaskLog {
            process_started_at: Some(10_000 + LOG_RECOVERY_START_SKEW_SECS),
            ..managed_log(marker, false)
        };
        assert_eq!(same_process.current_run().unwrap().started_at, Some(10_000));
    }

    #[test]
    fn complete_tail_drops_the_line_the_cut_falls_in() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("task.log");

        fs::write(&path, "short\nlog\n").unwrap();
        assert_eq!(
            read_complete_log_tail(&path).unwrap(),
            ("short\nlog\n".to_string(), false)
        );

        // The whole marker no longer fits, so the cut leaves just its secret end behind.
        let marker_end = "--password=secret ===\n";
        let filler = "x".repeat(LOG_TAIL_BYTES as usize - marker_end.len() - "next\n".len());
        let filler_line = format!("{}\nnext\n", &filler[1..]);
        fs::write(
            &path,
            format!("=== Cutting Board prepare command · {marker_end}{filler_line}"),
        )
        .unwrap();
        let (tail, truncated) = read_complete_log_tail(&path).unwrap();
        assert!(truncated);
        assert!(!tail.contains("secret"));
        assert!(tail.ends_with("next\n"));

        fs::write(&path, "y".repeat(LOG_TAIL_BYTES as usize + 10)).unwrap();
        assert_eq!(
            read_complete_log_tail(&path).unwrap(),
            (String::new(), true)
        );
    }
}
