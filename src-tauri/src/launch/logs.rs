use crate::models::{LaunchProfile, LaunchTask, ServiceSnapshot};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};

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

pub(super) fn read_log_tail(path: &Path) -> io::Result<String> {
    let mut file = OpenOptions::new().read(true).open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(64 * 1024);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
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
    let system = System::new_with_specifics(RefreshKind::nothing().with_processes(refresh_kind));
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
}
