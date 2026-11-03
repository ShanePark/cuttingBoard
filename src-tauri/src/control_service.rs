use crate::{
    control::{ControlError, ControlRequest, ControlResponse},
    launch::containers::ContainerError,
    models::{
        now_epoch, ContainerDetail, LaunchProfile, LaunchTask, ManagedTaskSnapshot,
        PrepareProgress, PrepareState, TaskRequest,
    },
    AppState,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_RUNNING_OPERATIONS: usize = 8;
const MAX_RETAINED_OPERATIONS: usize = 64;
const DEFAULT_LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 1_000;
const DEFAULT_LOG_BYTES: usize = 65_536;
const MAX_LOG_BYTES: usize = 65_536;
const MAX_DOCKER_DETAIL_CHARS: usize = 200;
/// Run starts and process start times are read separately, in whole seconds.
const LISTENER_START_SKEW_SECS: u64 = 2;
const READINESS_SCAN_MAX_AGE_SECS: u64 = 5;
const LOG_MARKER_PREFIX: &str = "=== Cutting Board ";
const LOG_START_MARKER_PREFIX: &str = "=== Cutting Board start ";

/// A failed task action. Container failures stay typed so they are never classified by matching
/// words in a message, where a container named `maven-db` would read as a Maven failure.
#[derive(Debug, Clone)]
pub(crate) enum TaskActionError {
    Message(String),
    Container(ContainerError),
    PortInUse(u16),
}

impl fmt::Display for TaskActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => formatter.write_str(message),
            Self::Container(error) => error.fmt(formatter),
            Self::PortInUse(port) => write!(
                formatter,
                "The expected port {port} is already in use by a process that is not safely matched to this task."
            ),
        }
    }
}

impl From<String> for TaskActionError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for TaskActionError {
    fn from(message: &str) -> Self {
        Self::Message(message.into())
    }
}

impl From<ContainerError> for TaskActionError {
    fn from(error: ContainerError) -> Self {
        Self::Container(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskAction {
    Start,
    Stop,
    Restart,
}

impl TaskAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    fn pending_state(self) -> &'static str {
        match self {
            Self::Start => "starting",
            Self::Stop => "stopping",
            Self::Restart => "restarting",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct TaskStatus {
    profile_id: String,
    task_name: String,
    state: String,
    main_pid: Option<u32>,
    started_at: Option<u64>,
    exit_code: Option<i32>,
    message: Option<String>,
    external_pid: Option<u32>,
    in_flight: bool,
    last_operation: Option<OperationView>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskEntry {
    name: String,
    container: Option<String>,
    expected_port: Option<u16>,
    #[serde(flatten)]
    status: TaskStatus,
}

#[derive(Debug, Clone, Serialize)]
struct ProfileEntry {
    id: String,
    name: String,
    tasks: Vec<TaskEntry>,
}

#[derive(Debug, Clone, Serialize)]
struct OperationView {
    operation_id: String,
    action: String,
    state: String,
    profile_id: String,
    task_name: String,
    accepted_at: u64,
    completed_at: Option<u64>,
    error_code: Option<String>,
    error_message: Option<String>,
    prepare: Option<PrepareProgress>,
}

impl OperationView {
    /// The operation is over, so a preparation it never heard the end of is not still running.
    fn settle_prepare(&mut self) {
        if let Some(prepare) = &mut self.prepare {
            if prepare.state == PrepareState::Running {
                prepare.state = PrepareState::Interrupted;
                prepare.completed_at = Some(now_epoch());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct OperationResult {
    operation: OperationView,
    task: Option<TaskStatus>,
}

#[derive(Debug, Clone)]
struct OperationRecord {
    view: OperationView,
    task: Option<TaskStatus>,
    error: Option<ControlError>,
}

#[derive(Debug, Default)]
struct ControlInner {
    next_sequence: u64,
    in_flight: HashMap<String, String>,
    profile_in_flight: HashSet<String>,
    profile_config_in_flight: bool,
    operations: HashMap<String, OperationRecord>,
    completed_order: VecDeque<String>,
    last_operation_by_task: HashMap<String, String>,
    snapshots: HashMap<String, TaskStatus>,
}

/// Process-local operation registry shared by Tauri commands and the local control socket.
#[derive(Debug, Clone, Default)]
pub(crate) struct ControlState(Arc<Mutex<ControlInner>>);

pub(crate) struct TaskOperationLease {
    state: ControlState,
    key: String,
    operation_id: String,
    action: TaskAction,
    previous_snapshot: Option<TaskStatus>,
    finished: bool,
}

pub(crate) struct ProfileOperationLease {
    state: ControlState,
    profile_id: Option<String>,
}

impl Drop for ProfileOperationLease {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.state.0.lock() {
            if let Some(profile_id) = &self.profile_id {
                inner.profile_in_flight.remove(profile_id);
            } else {
                inner.profile_config_in_flight = false;
            }
        }
    }
}

impl TaskOperationLease {
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Records a synchronous manager result and releases the per-task guard.
    pub(crate) fn finish(mut self, result: &Result<ManagedTaskSnapshot, TaskActionError>) {
        self.finish_inner(result);
    }

    fn finish_inner(&mut self, result: &Result<ManagedTaskSnapshot, TaskActionError>) {
        if self.finished {
            return;
        }
        let mut inner = match self.state.0.lock() {
            Ok(inner) => inner,
            Err(_) => {
                self.finished = true;
                return;
            }
        };
        let evaluated = evaluate_action(self.action, result);
        let (snapshot, error) = match evaluated {
            Ok(snapshot) => (Some(snapshot), None),
            Err((error, snapshot)) => (snapshot, Some(error)),
        };
        if let Some(record) = inner.operations.get_mut(&self.operation_id) {
            record.view.state = if error.is_some() {
                "failed"
            } else {
                "succeeded"
            }
            .into();
            record.view.completed_at = Some(epoch_seconds());
            record.view.error_code = error.as_ref().map(|error| error.code.clone());
            record.view.error_message = error.as_ref().map(|error| error.message.clone());
            record.view.settle_prepare();
            record.error = error;
        }
        let operation_view = inner
            .operations
            .get(&self.operation_id)
            .map(|record| record.view.clone());
        let mut task = snapshot
            .as_ref()
            .map(|snapshot| task_status(snapshot, false, operation_view.clone()))
            .or_else(|| self.previous_snapshot.clone())
            .or_else(|| {
                Some(TaskStatus {
                    profile_id: inner
                        .operations
                        .get(&self.operation_id)
                        .map(|record| record.view.profile_id.clone())
                        .unwrap_or_default(),
                    task_name: inner
                        .operations
                        .get(&self.operation_id)
                        .map(|record| record.view.task_name.clone())
                        .unwrap_or_default(),
                    state: "unknown".into(),
                    main_pid: None,
                    started_at: None,
                    exit_code: None,
                    message: None,
                    external_pid: None,
                    in_flight: false,
                    last_operation: operation_view.clone(),
                })
            });
        if let Some(task) = task.as_mut() {
            task.in_flight = false;
            task.last_operation = operation_view;
        }
        if let Some(record) = inner.operations.get_mut(&self.operation_id) {
            record.task = task.clone();
        }
        if let Some(status) = task {
            inner.snapshots.insert(self.key.clone(), status);
        }
        inner.in_flight.remove(&self.key);
        inner.completed_order.push_back(self.operation_id.clone());
        prune_operations(&mut inner);
        self.finished = true;
    }
}

impl Drop for TaskOperationLease {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Ok(mut inner) = self.state.0.lock() {
            inner.in_flight.remove(&self.key);
            if let Some(record) = inner.operations.get_mut(&self.operation_id) {
                record.view.state = "failed".into();
                record.view.completed_at = Some(epoch_seconds());
                record.view.error_code = Some("operation_interrupted".into());
                record.view.error_message =
                    Some("The task operation ended before its result could be recorded.".into());
                record.view.settle_prepare();
                record.error = Some(ControlError {
                    code: "operation_interrupted".into(),
                    message: "The task operation ended before its result could be recorded.".into(),
                });
                if let Some(mut previous) = self.previous_snapshot.clone() {
                    previous.in_flight = false;
                    inner.snapshots.insert(self.key.clone(), previous);
                } else {
                    inner.snapshots.remove(&self.key);
                }
                inner.completed_order.push_back(self.operation_id.clone());
                prune_operations(&mut inner);
            }
        }
        self.finished = true;
    }
}

impl ControlState {
    pub(crate) fn begin_task_action(
        &self,
        request: &TaskRequest,
        action: TaskAction,
    ) -> Result<TaskOperationLease, ControlError> {
        let key = task_key(&request.profile_id, &request.task_name);
        let mut inner = self.0.lock().map_err(|_| ControlError {
            code: "internal_error".into(),
            message: "The control operation registry is unavailable.".into(),
        })?;
        prune_operations(&mut inner);
        if let Some(operation_id) = inner.in_flight.get(&key) {
            return Err(ControlError {
                code: "busy".into(),
                message: format!(
                    "Task {} is already handling operation {operation_id}.",
                    request.task_name,
                ),
            });
        }
        if inner.profile_config_in_flight || inner.profile_in_flight.contains(&request.profile_id) {
            return Err(ControlError {
                code: "busy".into(),
                message: "A profile operation is already in progress.".into(),
            });
        }
        let running = inner
            .operations
            .values()
            .filter(|operation| operation.view.completed_at.is_none())
            .count();
        if running >= MAX_RUNNING_OPERATIONS {
            return Err(ControlError {
                code: "busy".into(),
                message: "Cutting Board already has the maximum number of active task operations."
                    .into(),
            });
        }

        inner.next_sequence = inner.next_sequence.wrapping_add(1);
        let sequence = inner.next_sequence;
        let operation_id = format!("op-{}-{}-{sequence}", std::process::id(), epoch_millis());
        let accepted_at = epoch_seconds();
        let previous_snapshot = inner.snapshots.get(&key).cloned();
        let mut pending = previous_snapshot.clone().unwrap_or_else(|| TaskStatus {
            profile_id: request.profile_id.clone(),
            task_name: request.task_name.clone(),
            state: "unknown".into(),
            main_pid: None,
            started_at: None,
            exit_code: None,
            message: None,
            external_pid: None,
            in_flight: false,
            last_operation: None,
        });
        pending.state = action.pending_state().into();
        pending.in_flight = true;
        pending.last_operation = None;
        inner.snapshots.insert(key.clone(), pending);
        inner.in_flight.insert(key.clone(), operation_id.clone());
        inner
            .last_operation_by_task
            .insert(key.clone(), operation_id.clone());
        inner.operations.insert(
            operation_id.clone(),
            OperationRecord {
                view: OperationView {
                    operation_id: operation_id.clone(),
                    action: action.as_str().into(),
                    state: "running".into(),
                    profile_id: request.profile_id.clone(),
                    task_name: request.task_name.clone(),
                    accepted_at,
                    completed_at: None,
                    error_code: None,
                    error_message: None,
                    prepare: None,
                },
                task: None,
                error: None,
            },
        );

        Ok(TaskOperationLease {
            state: self.clone(),
            key,
            operation_id,
            action,
            previous_snapshot,
            finished: false,
        })
    }

    pub(crate) fn begin_profile_action(
        &self,
        profile_id: &str,
    ) -> Result<ProfileOperationLease, ControlError> {
        let mut inner = self.0.lock().map_err(|_| ControlError {
            code: "internal_error".into(),
            message: "The control operation registry is unavailable.".into(),
        })?;
        if inner.profile_in_flight.contains(profile_id)
            || inner.profile_config_in_flight
            || inner
                .in_flight
                .keys()
                .any(|key| key.starts_with(&format!("{profile_id}\0")))
        {
            return Err(ControlError {
                code: "busy".into(),
                message: "A task operation is already in progress for this profile.".into(),
            });
        }
        inner.profile_in_flight.insert(profile_id.to_owned());
        Ok(ProfileOperationLease {
            state: self.clone(),
            profile_id: Some(profile_id.to_owned()),
        })
    }

    pub(crate) fn begin_profile_edit(&self) -> Result<ProfileOperationLease, ControlError> {
        let mut inner = self.0.lock().map_err(|_| ControlError {
            code: "internal_error".into(),
            message: "The control operation registry is unavailable.".into(),
        })?;
        if inner.profile_config_in_flight
            || !inner.profile_in_flight.is_empty()
            || !inner.in_flight.is_empty()
        {
            return Err(ControlError {
                code: "busy".into(),
                message: "A profile or task operation is already in progress.".into(),
            });
        }
        inner.profile_config_in_flight = true;
        Ok(ProfileOperationLease {
            state: self.clone(),
            profile_id: None,
        })
    }

    /// Notes how the preparation of a task's operation is going. A task with no operation in flight
    /// has nobody to tell.
    pub(crate) fn record_prepare(
        &self,
        profile_id: &str,
        task_name: &str,
        progress: PrepareProgress,
    ) {
        if let Ok(mut inner) = self.0.lock() {
            let operation_id = inner
                .in_flight
                .get(&task_key(profile_id, task_name))
                .cloned();
            if let Some(record) = operation_id.and_then(|id| inner.operations.get_mut(&id)) {
                record.view.prepare = Some(progress);
            }
        }
    }

    fn record_snapshot(&self, snapshot: &ManagedTaskSnapshot) {
        let key = task_key(&snapshot.profile_id, &snapshot.task_name);
        if let Ok(mut inner) = self.0.lock() {
            let operation_id = inner.last_operation_by_task.get(&key).cloned();
            let last_operation = operation_id
                .and_then(|id| inner.operations.get(&id))
                .map(|record| record.view.clone());
            let mut status = task_status(snapshot, false, last_operation);
            status.in_flight = inner.in_flight.contains_key(&key);
            inner.snapshots.insert(key, status);
        }
    }

    fn cached_snapshot(&self, profile_id: &str, task_name: &str) -> Option<TaskStatus> {
        self.0
            .lock()
            .ok()?
            .snapshots
            .get(&task_key(profile_id, task_name))
            .cloned()
    }

    fn last_operation(&self, profile_id: &str, task_name: &str) -> Option<OperationView> {
        let inner = self.0.lock().ok()?;
        let id = inner
            .last_operation_by_task
            .get(&task_key(profile_id, task_name))?;
        inner.operations.get(id).map(|record| record.view.clone())
    }

    fn is_in_flight(&self, profile_id: &str, task_name: &str) -> bool {
        self.0.lock().is_ok_and(|inner| {
            inner
                .in_flight
                .contains_key(&task_key(profile_id, task_name))
        })
    }

    fn operation(&self, operation_id: &str) -> Option<OperationRecord> {
        self.0.lock().ok()?.operations.get(operation_id).cloned()
    }

    fn apply_snapshots(&self, snapshots: &[ManagedTaskSnapshot]) {
        for snapshot in snapshots {
            self.record_snapshot(snapshot);
        }
    }
}

pub(crate) fn dispatch(state: &AppState, request: ControlRequest) -> ControlResponse {
    match request {
        ControlRequest::List {} => list(state),
        ControlRequest::Status {
            profile_id,
            task_name,
        } => status(state, &profile_id, &task_name),
        ControlRequest::Logs {
            profile_id,
            task_name,
            lines,
            bytes,
            current_run,
            prepare,
        } => {
            if prepare {
                prepare_logs(state, &profile_id, &task_name, lines, bytes)
            } else {
                logs(state, &profile_id, &task_name, lines, bytes, current_run)
            }
        }
        ControlRequest::Readiness {
            profile_id,
            task_name,
            log_pattern,
        } => readiness(state, &profile_id, &task_name, log_pattern.as_deref()),
        ControlRequest::Show {
            profile_id,
            task_name,
        } => show(state, &profile_id, &task_name),
        ControlRequest::Update {
            profile_id,
            task_name,
            command,
            cwd,
            expected_port,
            clear_expected_port,
            clear_prepare,
        } => update(
            state,
            &profile_id,
            &task_name,
            &TaskUpdate {
                command,
                cwd,
                expected_port,
                clear_expected_port,
                clear_prepare,
            },
        ),
        ControlRequest::Start {
            profile_id,
            task_name,
        } => start_action(state, profile_id, task_name, TaskAction::Start),
        ControlRequest::Stop {
            profile_id,
            task_name,
        } => start_action(state, profile_id, task_name, TaskAction::Stop),
        ControlRequest::Restart {
            profile_id,
            task_name,
        } => start_action(state, profile_id, task_name, TaskAction::Restart),
        ControlRequest::Operation { operation_id } => operation(state, &operation_id),
    }
}

fn list(state: &AppState) -> ControlResponse {
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let (scanned_at, mut refresh_error) = match crate::refresh_control_scan(state) {
        Ok(workspace) if workspace.errors.is_empty() => (Some(workspace.scanned_at), None),
        Ok(workspace) => (
            Some(workspace.scanned_at),
            Some("Service detection returned partial results; external task matches may be stale."),
        ),
        Err(_) => (
            crate::control_cached_scan(state)
                .ok()
                .flatten()
                .map(|workspace| workspace.scanned_at),
            Some("Service detection could not be refreshed; external task matches may be stale."),
        ),
    };
    let (snapshots, mut snapshot_stale) = match crate::control_task_snapshots(state, &profiles) {
        Ok(Some(snapshots)) => {
            state.control_service_state().apply_snapshots(&snapshots);
            (snapshots, refresh_error.is_some())
        }
        Ok(None) => {
            refresh_error
                .get_or_insert("Task state could not be refreshed because the manager is busy.");
            (Vec::new(), true)
        }
        Err(error) => return failure("status_error", safe_read_error(&error)),
    };
    snapshot_stale |= refresh_error.is_some();
    let entries = profile_entries(state, &profiles, &snapshots, snapshot_stale);
    success(json!({
        "profiles": entries,
        "scanned_at": scanned_at,
        "snapshot_stale": snapshot_stale,
        "refresh_error": refresh_error
    }))
}

fn status(state: &AppState, profile_id: &str, task_name: &str) -> ControlResponse {
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let (profile, task) = match find_profile_task(&profiles, profile_id, task_name) {
        Ok(found) => found,
        Err(error) => return failure(error.code, error.message),
    };
    let (scanned_at, mut refresh_error) = match crate::refresh_control_scan(state) {
        Ok(workspace) if workspace.errors.is_empty() => (Some(workspace.scanned_at), None),
        Ok(workspace) => (
            Some(workspace.scanned_at),
            Some("Service detection returned partial results; external task matches may be stale."),
        ),
        Err(_) => (
            crate::control_cached_scan(state)
                .ok()
                .flatten()
                .map(|workspace| workspace.scanned_at),
            Some("Service detection could not be refreshed; external task matches may be stale."),
        ),
    };
    let (snapshots, mut snapshot_stale) = match crate::control_task_snapshots(state, &profiles) {
        Ok(Some(snapshots)) => {
            state.control_service_state().apply_snapshots(&snapshots);
            (snapshots, refresh_error.is_some())
        }
        Ok(None) => {
            refresh_error
                .get_or_insert("Task state could not be refreshed because the manager is busy.");
            (Vec::new(), true)
        }
        Err(error) => return failure("status_error", safe_read_error(&error)),
    };
    snapshot_stale |= refresh_error.is_some();
    let status = find_status(state, &snapshots, profile_id, task_name, snapshot_stale);
    success(json!({
        "profile": { "id": profile.id, "name": profile.name },
        "task": task_entry(task, status),
        "scanned_at": scanned_at,
        "snapshot_stale": snapshot_stale,
        "refresh_error": refresh_error
    }))
}

fn logs(
    state: &AppState,
    profile_id: &str,
    task_name: &str,
    lines: Option<u32>,
    bytes: Option<u32>,
    current_run: bool,
) -> ControlResponse {
    let (lines, bytes) = match log_limits(lines, bytes) {
        Ok(limits) => limits,
        Err(error) => return failure(error.code, error.message),
    };
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let (_, task) = match find_profile_task(&profiles, profile_id, task_name) {
        Ok(found) => found,
        Err(error) => return failure(error.code, error.message),
    };
    let request = TaskRequest {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
    };
    let is_container = task.container_name().is_some();
    if current_run && is_container {
        return failure("run_boundary_unknown", CONTAINER_LOG_HAS_NO_BOUNDARY);
    }
    // Which log belongs to the task depends on what is running. Plain reads accept the last scan.
    let workspace = if current_run {
        crate::refresh_control_scan(state)
            .ok()
            .or_else(|| crate::control_cached_scan(state).ok().flatten())
    } else {
        crate::control_cached_scan(state)
            .ok()
            .flatten()
            .or_else(|| crate::refresh_control_scan(state).ok())
    };
    let log = match crate::control_task_log(state, &profiles, &request, workspace.as_ref()) {
        Ok(log) => log,
        Err(error) => {
            let error = task_log_error(&error);
            return failure(error.code, error.message);
        }
    };
    let run = match current_run.then(|| log.current_run()) {
        Some(None) => return failure("run_boundary_unknown", EXTERNAL_LOG_HAS_NO_BOUNDARY),
        Some(run) => run,
        None => None,
    };
    let (text, source_may_be_truncated) = run.map_or((log.text.as_str(), log.truncated), |run| {
        (run.text, run.truncated)
    });
    let filtered = if is_container {
        text.to_owned()
    } else {
        sanitize_task_log(text)
    };
    let limited = limit_log_tail(&filtered, lines, bytes, source_may_be_truncated);
    let mut result = json!({
        "profile_id": profile_id,
        "task_name": task_name,
        "logs": limited.content,
        "lines": if limited.content.is_empty() { 0 } else { limited.content.lines().count() },
        "bytes": limited.content.len(),
        "truncated": limited.truncated
    });
    if let Some(run) = run {
        result["current_run"] = json!({
            "started_at": run.started_at,
            "boundary": run.boundary.as_str()
        });
    }
    success(result)
}

fn log_limits(lines: Option<u32>, bytes: Option<u32>) -> Result<(usize, usize), ControlError> {
    let lines = lines
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LOG_LINES);
    let bytes = bytes
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LOG_BYTES);
    if !(1..=MAX_LOG_LINES).contains(&lines) || !(1..=MAX_LOG_BYTES).contains(&bytes) {
        return Err(ControlError {
            code: "invalid_log_limit".into(),
            message: "Log limits must be between 1 and 1000 lines and between 1 and 65536 bytes."
                .into(),
        });
    }
    Ok((lines, bytes))
}

fn prepare_logs(
    state: &AppState,
    profile_id: &str,
    task_name: &str,
    lines: Option<u32>,
    bytes: Option<u32>,
) -> ControlResponse {
    let (lines, bytes) = match log_limits(lines, bytes) {
        Ok(limits) => limits,
        Err(error) => return failure(error.code, error.message),
    };
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let (profile, task) = match find_profile_task(&profiles, profile_id, task_name) {
        Ok(found) => found,
        Err(error) => return failure(error.code, error.message),
    };
    if task.container_name().is_some() {
        return failure("invalid_request", "A container task has no prepare step.");
    }
    let run = match crate::control_task_prepare_run(state, profile, task) {
        Ok(run) => run,
        Err(_) => return failure("log_read_failed", "The task log could not be read."),
    };
    let prepare_state = if run.state == PrepareState::Running
        && !state
            .control_service_state()
            .is_in_flight(profile_id, task_name)
    {
        PrepareState::Interrupted
    } else {
        run.state
    };
    let limited = limit_log_tail(&sanitize_task_log(&run.text), lines, bytes, run.truncated);
    success(json!({
        "profile_id": profile_id,
        "task_name": task_name,
        "logs": limited.content,
        "lines": if limited.content.is_empty() { 0 } else { limited.content.lines().count() },
        "bytes": limited.content.len(),
        "truncated": limited.truncated,
        "prepare_run": {
            "state": prepare_state,
            "started_at": run.started_at,
            "completed_at": run.completed_at,
            "failure": run.failure
        }
    }))
}

fn task_log_error(error: &TaskActionError) -> ControlError {
    match error {
        TaskActionError::Container(error) => container_error(error),
        TaskActionError::Message(error) => ControlError {
            code: "log_read_failed".into(),
            message: safe_read_error(error),
        },
        TaskActionError::PortInUse(port) => port_in_use_error(*port),
    }
}

const CONTAINER_LOG_HAS_NO_BOUNDARY: &str =
    "Container logs have no run boundary, so a current run cannot be told apart from earlier ones.";
const EXTERNAL_LOG_HAS_NO_BOUNDARY: &str =
    "This task's process was not started by Cutting Board, so its log has no run boundary.";

#[derive(Debug, PartialEq, Eq)]
enum ContainerHealth {
    Healthy,
    Starting,
    Unhealthy,
}

fn container_health(status: &str) -> Option<ContainerHealth> {
    if status.contains("(healthy)") {
        Some(ContainerHealth::Healthy)
    } else if status.contains("(health: starting)") {
        Some(ContainerHealth::Starting)
    } else if status.contains("(unhealthy)") {
        Some(ContainerHealth::Unhealthy)
    } else {
        None
    }
}

#[derive(Debug, Default)]
struct ContainerReadiness {
    health: Option<ContainerHealth>,
    /// What to report when the container is not running.
    stopped_error: Option<ControlError>,
}

#[derive(Debug)]
struct FailedOperation {
    code: String,
    message: String,
    completed_at: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum LogPatternMatch {
    Matched,
    NotMatched,
    NoRunBoundary,
}

/// Everything `wait` bases its judgement on, gathered once per poll.
#[derive(Debug, Default)]
struct ReadinessFacts {
    in_flight: bool,
    /// The operation in flight is building the project.
    preparing: bool,
    state: String,
    /// The task state could not be refreshed, so `state` may be old.
    state_stale: bool,
    exit_code: Option<i32>,
    run_started_at: Option<u64>,
    failed_operation: Option<FailedOperation>,
    container: Option<ContainerReadiness>,
    log_pattern: Option<LogPatternMatch>,
    port: Option<u16>,
    port_accepting: bool,
    /// When the process listening on `port` started, if the scan can see it. It cannot see
    /// docker-proxy, for example, so its absence says nothing against an accepted connection.
    listener_started_at: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum ReadinessVerdict {
    Ready(&'static str),
    Waiting(&'static str),
    Failed { code: String, message: String },
}

impl ReadinessVerdict {
    fn failed(code: &str, message: impl Into<String>) -> Self {
        Self::Failed {
            code: code.into(),
            message: message.into(),
        }
    }
}

fn judge_readiness(facts: &ReadinessFacts) -> ReadinessVerdict {
    if facts.container.is_some() && facts.log_pattern.is_some() {
        return ReadinessVerdict::failed("run_boundary_unknown", CONTAINER_LOG_HAS_NO_BOUNDARY);
    }
    if facts.in_flight {
        return ReadinessVerdict::Waiting(if facts.preparing {
            "prepare"
        } else {
            "operation"
        });
    }
    // A failed restart leaves the old process running and holding its port, so nothing below may
    // be consulted before this.
    if let Some(operation) = &facts.failed_operation {
        if facts
            .run_started_at
            .map_or(true, |started_at| operation.completed_at >= started_at)
            && !docker_contradicts(facts, operation)
        {
            return ReadinessVerdict::failed(&operation.code, &operation.message);
        }
    }
    if facts.state_stale {
        return ReadinessVerdict::Waiting("task_state");
    }
    match facts.state.as_str() {
        "running" => {}
        "failed" => {
            return ReadinessVerdict::failed(
                "process_exited",
                match facts.exit_code {
                    Some(code) => format!("The task exited with code {code}."),
                    None => "The task exited.".into(),
                },
            );
        }
        "stopped" => {
            let reason = facts
                .container
                .as_ref()
                .and_then(|container| container.stopped_error.as_ref());
            return match reason {
                Some(error) => ReadinessVerdict::failed(&error.code, &error.message),
                None => ReadinessVerdict::failed("not_running", "The task is not running."),
            };
        }
        _ => return ReadinessVerdict::Waiting("task_state"),
    }
    if let Some(container) = &facts.container {
        return match container.health {
            Some(ContainerHealth::Healthy) => ReadinessVerdict::Ready("container_health"),
            Some(ContainerHealth::Starting) => ReadinessVerdict::Waiting("container_health"),
            Some(ContainerHealth::Unhealthy) => ReadinessVerdict::failed(
                "container_unhealthy",
                "Docker reports the container as unhealthy.",
            ),
            None if facts.port.is_none() => {
                ReadinessVerdict::failed("readiness_unavailable", NO_READINESS_SIGNAL)
            }
            None if facts.port_accepting => ReadinessVerdict::Ready("container_port"),
            None => ReadinessVerdict::Waiting("port"),
        };
    }
    match facts.log_pattern {
        Some(LogPatternMatch::Matched) => return ReadinessVerdict::Ready("log_pattern"),
        Some(LogPatternMatch::NotMatched) => return ReadinessVerdict::Waiting("log_pattern"),
        Some(LogPatternMatch::NoRunBoundary) => {
            return ReadinessVerdict::failed("run_boundary_unknown", EXTERNAL_LOG_HAS_NO_BOUNDARY);
        }
        None => {}
    }
    if facts.port.is_none() {
        return ReadinessVerdict::failed("readiness_unavailable", NO_READINESS_SIGNAL);
    }
    if !facts.port_accepting {
        return ReadinessVerdict::Waiting("port");
    }
    if let (Some(listener), Some(run)) = (facts.listener_started_at, facts.run_started_at) {
        if listener + LISTENER_START_SKEW_SECS < run {
            return ReadinessVerdict::Waiting("listener_predates_run");
        }
    }
    ReadinessVerdict::Ready("port")
}

/// A container has no run start to date a failure against, but one that said it was gone or down is
/// over once Docker shows it running, say after `docker compose up` by hand. A failed restart stays:
/// it can leave the old container up, so that one being up proves nothing.
fn docker_contradicts(facts: &ReadinessFacts, operation: &FailedOperation) -> bool {
    facts.container.is_some()
        && !facts.state_stale
        && facts.state == "running"
        && matches!(
            operation.code.as_str(),
            "container_missing" | "docker_unavailable" | "process_exited"
        )
}

const NO_READINESS_SIGNAL: &str =
    "This task has no expected port, so readiness needs --log-pattern. Save a port or pass a pattern.";

fn readiness(
    state: &AppState,
    profile_id: &str,
    task_name: &str,
    log_pattern: Option<&str>,
) -> ControlResponse {
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let (_, task) = match find_profile_task(&profiles, profile_id, task_name) {
        Ok(found) => found,
        Err(error) => return failure(error.code, error.message),
    };
    let is_container = task.container_name().is_some();
    let mut facts = ReadinessFacts {
        in_flight: state
            .control_service_state()
            .is_in_flight(profile_id, task_name),
        container: is_container.then(ContainerReadiness::default),
        log_pattern: log_pattern.map(|_| LogPatternMatch::NotMatched),
        port: task.expected_port,
        ..Default::default()
    };
    // These verdicts need no scan, and a build can hold the manager for minutes.
    let answered_early = facts.in_flight || (is_container && log_pattern.is_some());
    let mut snapshots = Vec::new();
    if !answered_early {
        let request = TaskRequest {
            profile_id: profile_id.into(),
            task_name: task_name.into(),
        };
        if let Err(error) = gather_readiness_facts(
            state,
            &profiles,
            &request,
            log_pattern,
            &mut facts,
            &mut snapshots,
        ) {
            return failure(error.code, error.message);
        }
    }
    let status = find_status(state, &snapshots, profile_id, task_name, facts.state_stale);
    if facts.state.is_empty() {
        facts.state = status.state.clone();
    }
    // An operation that began during the scan makes these facts describe the process it replaces.
    // An early answer gathered no facts, so one that has just ended must not clear the flag.
    facts.in_flight = facts.in_flight
        || state
            .control_service_state()
            .is_in_flight(profile_id, task_name);
    facts.preparing = state
        .control_service_state()
        .last_operation(profile_id, task_name)
        .and_then(|operation| operation.prepare)
        .is_some_and(|prepare| prepare.state == PrepareState::Running);
    let verdict = judge_readiness(&facts);
    let (ready, key, value) = match verdict {
        ReadinessVerdict::Ready(signal) => (true, "signal", signal),
        ReadinessVerdict::Waiting(reason) => (false, "waiting_for", reason),
        ReadinessVerdict::Failed { code, message } => return failure(code, message),
    };
    let mut result = json!({
        "profile_id": profile_id,
        "task_name": task_name,
        "ready": ready,
        "state": status.state,
        "started_at": status.started_at,
        "port": facts.port
    });
    result[key] = json!(value);
    success(result)
}

fn gather_readiness_facts(
    state: &AppState,
    profiles: &[LaunchProfile],
    request: &TaskRequest,
    log_pattern: Option<&str>,
    facts: &mut ReadinessFacts,
    snapshots: &mut Vec<ManagedTaskSnapshot>,
) -> Result<(), ControlError> {
    let (profile_id, task_name) = (request.profile_id.as_str(), request.task_name.as_str());
    let snapshot_of_target = || {
        crate::control_task_snapshot(state, profiles, request).map_err(|error| ControlError {
            code: "status_error".into(),
            message: safe_read_error(&error),
        })
    };
    // Scanning is expensive and this runs once a second. Docker answers for a container and a live
    // managed process for itself; anything else may have been started outside Cutting Board, which
    // only a new scan can show.
    let mut current = snapshot_of_target()?;
    let mut workspace = None;
    if facts.container.is_none() && current.as_ref().is_some_and(|task| !is_live_managed(task)) {
        workspace = crate::refresh_control_scan(state).ok();
        facts.state_stale = workspace.is_none();
        current = snapshot_of_target()?;
    }
    match current {
        Some(current) => {
            state
                .control_service_state()
                .apply_snapshots(std::slice::from_ref(&current));
            *snapshots = vec![current];
        }
        None => facts.state_stale = true,
    }
    if let Some(operation) = state
        .control_service_state()
        .last_operation(profile_id, task_name)
    {
        if let (Some(code), Some(message), Some(completed_at)) = (
            operation.error_code,
            operation.error_message,
            operation.completed_at,
        ) {
            facts.failed_operation = Some(FailedOperation {
                code,
                message,
                completed_at,
            });
        }
    }
    let Some(snapshot) = snapshots.first() else {
        return Ok(());
    };
    facts.state = snapshot.state.clone();
    facts.exit_code = snapshot.exit_code;
    facts.run_started_at = snapshot.started_at;
    if let (Some(container), Some(detail)) = (&mut facts.container, &snapshot.container_detail) {
        match detail {
            ContainerDetail::Listed { status, ports } => {
                container.health = container_health(status);
                facts.port = facts.port.or(ports.first().copied());
                container.stopped_error = Some(ControlError {
                    code: "not_running".into(),
                    message: match safe_docker_text(status) {
                        Some(status) => {
                            format!("The container is not running. Docker status: {status}")
                        }
                        None => "The container is not running.".into(),
                    },
                });
            }
            ContainerDetail::Missing { container: name } => {
                container.stopped_error = Some(ControlError {
                    code: "container_missing".into(),
                    message: container_missing_message(name),
                });
            }
            ContainerDetail::DockerUnavailable { detail } => {
                container.stopped_error = Some(ControlError {
                    code: "docker_unavailable".into(),
                    message: docker_unavailable_message(detail),
                });
            }
        }
    }
    if snapshot.state != "running" {
        return Ok(());
    }
    if log_pattern.is_none() || facts.container.is_some() {
        if let Some(port) = facts.port {
            let recent = match &workspace {
                Some(_) => None,
                None => recent_scan(state),
            };
            let probe = probe_port(port, workspace.as_ref().or(recent.as_ref()));
            (facts.port_accepting, facts.listener_started_at) = probe;
            // A refused connection needs no scan, but dating a listener needs one from before the
            // connection: a listener that appears between the two is new, whereas probing first
            // could credit a connection to a process the scan then no longer shows.
            if facts.port_accepting && workspace.is_none() && facts.container.is_none() {
                workspace = crate::refresh_control_scan(state).ok();
                facts.state_stale = workspace.is_none();
                (facts.port_accepting, facts.listener_started_at) =
                    probe_port(port, workspace.as_ref());
            }
        }
    }
    if let (Some(pattern), None) = (log_pattern, &facts.container) {
        let log = crate::control_task_log(state, profiles, request, workspace.as_ref())
            .map_err(|error| task_log_error(&error))?;
        facts.log_pattern = Some(match log.current_run() {
            Some(run) if run.boundary != crate::launch::RunBoundary::None => {
                if sanitize_task_log(run.text).contains(pattern) {
                    LogPatternMatch::Matched
                } else {
                    LogPatternMatch::NotMatched
                }
            }
            _ => LogPatternMatch::NoRunBoundary,
        });
    }
    Ok(())
}

fn is_live_managed(snapshot: &ManagedTaskSnapshot) -> bool {
    snapshot.external_pid.is_none()
        && matches!(snapshot.state.as_str(), "starting" | "running" | "stopping")
}

/// The last scan while it is recent, which also bounds how long a listener only a scan can point
/// the probe at, on a non-loopback address, goes unnoticed.
fn recent_scan(state: &AppState) -> Option<crate::models::WorkspaceSnapshot> {
    crate::control_cached_scan(state)
        .ok()
        .flatten()
        .filter(|scan| {
            epoch_seconds().saturating_sub(scan.scanned_at) < READINESS_SCAN_MAX_AGE_SECS
        })
        .or_else(|| crate::refresh_control_scan(state).ok())
}

/// Whether the port accepts a connection, and when the oldest process the scan shows on it started.
fn probe_port(
    port: u16,
    workspace: Option<&crate::models::WorkspaceSnapshot>,
) -> (bool, Option<u64>) {
    let listeners = workspace
        .iter()
        .flat_map(|workspace| &workspace.services)
        .filter(|service| {
            service
                .endpoints
                .iter()
                .any(|endpoint| endpoint.port == port)
        });
    let mut listener_started_at = None;
    let mut endpoints = Vec::new();
    for service in listeners {
        if let Some(process) = &service.process {
            listener_started_at = Some(
                listener_started_at.map_or(process.create_time, |known: u64| {
                    known.min(process.create_time)
                }),
            );
        }
        endpoints.extend(
            service
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.port == port)
                .filter_map(|endpoint| endpoint.address.parse::<std::net::IpAddr>().ok()),
        );
    }
    (
        crate::launch::port_accepts_connection(port, &endpoints),
        listener_started_at,
    )
}

fn show(state: &AppState, profile_id: &str, task_name: &str) -> ControlResponse {
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    match find_profile_task(&profiles, profile_id, task_name) {
        Ok((profile, task)) => success(json!({
            "profile": { "id": profile.id, "name": profile.name },
            "task": task_details(profile, task)
        })),
        Err(error) => failure(error.code, error.message),
    }
}

fn find_profile_task<'a>(
    profiles: &'a [LaunchProfile],
    profile_id: &str,
    task_name: &str,
) -> Result<(&'a LaunchProfile, &'a LaunchTask), ControlError> {
    let profile = profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| ControlError {
            code: "unknown_profile".into(),
            message: "The launch profile does not exist.".into(),
        })?;
    let task = profile
        .tasks
        .iter()
        .find(|task| task.name == task_name)
        .ok_or_else(|| ControlError {
            code: "unknown_task".into(),
            message: "The task does not exist in that launch profile.".into(),
        })?;
    Ok((profile, task))
}

/// Saved values, unmasked. Only `show` and `update` return this, on explicit request.
fn task_details(profile: &LaunchProfile, task: &LaunchTask) -> Value {
    let resolved = crate::launch::task_resolved(profile, task);
    json!({
        "name": task.name,
        "cwd": task.cwd,
        "resolved_cwd": crate::launch::task_cwd(profile, task).to_string_lossy(),
        "command": task.command,
        "expected_port": task.expected_port,
        "container": task.container_name(),
        "prepare": task.prepare,
        "launch_source": resolved.as_ref().map_or("container", |resolved| resolved.launch_source),
        "resolved": resolved
    })
}

#[derive(Debug, Default)]
struct TaskUpdate {
    command: Option<String>,
    cwd: Option<String>,
    expected_port: Option<u16>,
    clear_expected_port: bool,
    clear_prepare: bool,
}

fn apply_task_update(
    task: &mut LaunchTask,
    update: &TaskUpdate,
) -> Result<serde_json::Map<String, Value>, ControlError> {
    if task.container_name().is_some() && (update.command.is_some() || update.clear_prepare) {
        return Err(ControlError {
            code: "invalid_request".into(),
            message: "A container task has no command or prepare settings to change.".into(),
        });
    }
    let mut changed = serde_json::Map::new();
    let mut record = |field: &str, old: Value, new: Value| {
        if old != new {
            changed.insert(field.into(), json!({ "old": old, "new": new }));
        }
    };
    if let Some(command) = &update.command {
        let command = command.trim();
        record("command", json!(task.command), json!(command));
        task.command = command.into();
    }
    if let Some(cwd) = &update.cwd {
        let cwd = cwd.trim();
        record("cwd", json!(task.cwd), json!(cwd));
        task.cwd = cwd.into();
    }
    if update.expected_port.is_some() || update.clear_expected_port {
        record(
            "expected_port",
            json!(task.expected_port),
            json!(update.expected_port),
        );
        task.expected_port = update.expected_port;
    }
    if update.clear_prepare {
        record("prepare", json!(task.prepare), Value::Null);
        task.prepare = None;
    }
    Ok(changed)
}

fn update(
    state: &AppState,
    profile_id: &str,
    task_name: &str,
    update: &TaskUpdate,
) -> ControlResponse {
    if let Err(message) = crate::reject_demo(state) {
        return failure("demo_mode", message);
    }
    // Without a fresh scan, a process started outside Cutting Board could go unnoticed.
    let Ok(workspace) = crate::refresh_control_scan(state) else {
        return failure(
            "status_error",
            "Service detection failed, so the task could not be confirmed as stopped.",
        );
    };
    let mut changed = serde_json::Map::new();
    let saved = crate::save_profile_guarded(state, |profiles| {
        let (profile, task) = find_profile_task(profiles, profile_id, task_name)?;
        match crate::control_task_is_active(state, profile, task, &workspace) {
            Ok(false) => {}
            Ok(true) => {
                return Err(ControlError {
                    code: "task_active".into(),
                    message: "Stop this task before updating it.".into(),
                })
            }
            Err(TaskActionError::Container(error)) => return Err(container_error(&error)),
            Err(error) => {
                return Err(ControlError {
                    code: "status_error".into(),
                    message: safe_read_error(&error.to_string()),
                })
            }
        }
        let mut updated = profile.clone();
        if let Some(target) = updated.tasks.iter_mut().find(|task| task.name == task_name) {
            changed = apply_task_update(target, update)?;
        }
        Ok(updated)
    });
    let profiles = match saved {
        Ok(profiles) => profiles,
        Err(error) if error.code == crate::SAVE_PROFILE_STORAGE_ERROR => {
            return failure(error.code, "The launch profile could not be saved.")
        }
        Err(error) => return failure(error.code, error.message),
    };
    let (profile, task) = match find_profile_task(&profiles, profile_id, task_name) {
        Ok(found) => found,
        Err(error) => return failure(error.code, error.message),
    };
    let mut warnings = Vec::new();
    if crate::launch::task_launch_source(profile, task) == "prepare" {
        warnings.push(if update.clear_prepare {
            "Clearing the prepare settings did not make this task run its saved command verbatim: project auto-detection still selects the prepare path."
        } else {
            "This task takes the prepare path, so its saved command is not run verbatim."
        });
    }
    success(json!({
        "profile": { "id": profile.id, "name": profile.name },
        "task": task_details(profile, task),
        "changed": changed,
        "warnings": warnings
    }))
}

fn start_action(
    state: &AppState,
    profile_id: String,
    task_name: String,
    action: TaskAction,
) -> ControlResponse {
    if crate::control_is_demo(state) {
        return failure("demo_mode", "Actions are disabled in demonstration mode.");
    }
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    if let Err(error) = find_profile_task(&profiles, &profile_id, &task_name) {
        return failure(error.code, error.message);
    }
    let request = TaskRequest {
        profile_id,
        task_name,
    };
    let lease = match state
        .control_service_state()
        .begin_task_action(&request, action)
    {
        Ok(lease) => lease,
        Err(error) => return failure(&error.code, &error.message),
    };
    let operation_id = lease.operation_id().to_owned();
    let action_state = state.clone();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::execute_control_task_action(&action_state, &request, action)
        }))
        .unwrap_or_else(|_| Err("The task operation ended unexpectedly.".into()));
        lease.finish(&result);
    });
    accepted(&operation_id)
}

fn operation(state: &AppState, operation_id: &str) -> ControlResponse {
    let Some(record) = state.control_service_state().operation(operation_id) else {
        return failure(
            "unknown_operation",
            "The operation ID is not available in this app session.",
        );
    };
    let complete = record.view.completed_at.is_some();
    let result = serde_json::to_value(OperationResult {
        operation: record.view.clone(),
        task: record.task.clone(),
    })
    .unwrap_or(Value::Null);
    ControlResponse {
        ok: record.error.is_none(),
        accepted: true,
        completed: complete,
        operation_id: Some(operation_id.into()),
        result: Some(result),
        error: record.error,
    }
}

fn profile_entries(
    state: &AppState,
    profiles: &[LaunchProfile],
    snapshots: &[ManagedTaskSnapshot],
    snapshot_stale: bool,
) -> Vec<ProfileEntry> {
    profiles
        .iter()
        .map(|profile| ProfileEntry {
            id: profile.id.clone(),
            name: profile.name.clone(),
            tasks: profile
                .tasks
                .iter()
                .map(|task| {
                    let status =
                        find_status(state, snapshots, &profile.id, &task.name, snapshot_stale);
                    task_entry(task, status)
                })
                .collect(),
        })
        .collect()
}

fn task_entry(task: &LaunchTask, status: TaskStatus) -> TaskEntry {
    TaskEntry {
        name: task.name.clone(),
        container: task.container_name().map(str::to_owned),
        expected_port: task.expected_port,
        status,
    }
}

fn find_status(
    state: &AppState,
    snapshots: &[ManagedTaskSnapshot],
    profile_id: &str,
    task_name: &str,
    snapshot_stale: bool,
) -> TaskStatus {
    let key = task_key(profile_id, task_name);
    let snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.profile_id == profile_id && snapshot.task_name == task_name);
    let mut status = snapshot
        .map(|snapshot| {
            let last_operation = state
                .control_service_state()
                .last_operation(profile_id, task_name);
            task_status(snapshot, false, last_operation)
        })
        .or_else(|| {
            state
                .control_service_state()
                .cached_snapshot(profile_id, task_name)
        })
        .unwrap_or_else(|| TaskStatus {
            profile_id: profile_id.into(),
            task_name: task_name.into(),
            state: if snapshot_stale { "unknown" } else { "stopped" }.into(),
            main_pid: None,
            started_at: None,
            exit_code: None,
            message: None,
            external_pid: None,
            in_flight: false,
            last_operation: None,
        });
    status.in_flight = state
        .control_service_state()
        .0
        .lock()
        .is_ok_and(|inner| inner.in_flight.contains_key(&key));
    if status.in_flight {
        if let Some(operation) = state
            .control_service_state()
            .last_operation(profile_id, task_name)
        {
            status.state = match operation.action.as_str() {
                "start" => "starting",
                "stop" => "stopping",
                "restart" => "restarting",
                _ => &status.state,
            }
            .into();
            status.last_operation = Some(operation);
        }
    }
    status
}

fn task_status(
    snapshot: &ManagedTaskSnapshot,
    in_flight: bool,
    last_operation: Option<OperationView>,
) -> TaskStatus {
    TaskStatus {
        profile_id: snapshot.profile_id.clone(),
        task_name: snapshot.task_name.clone(),
        state: snapshot.state.clone(),
        main_pid: snapshot.main_pid,
        started_at: snapshot.started_at,
        exit_code: snapshot.exit_code,
        message: safe_snapshot_message(snapshot),
        external_pid: snapshot.external_pid,
        in_flight,
        last_operation,
    }
}

fn evaluate_action(
    action: TaskAction,
    result: &Result<ManagedTaskSnapshot, TaskActionError>,
) -> Result<ManagedTaskSnapshot, (ControlError, Option<ManagedTaskSnapshot>)> {
    match result {
        Err(TaskActionError::Container(error)) => Err((container_error(error), None)),
        Err(TaskActionError::Message(message)) => Err((operation_error(action, message), None)),
        Err(TaskActionError::PortInUse(port)) => Err((port_in_use_error(*port), None)),
        Ok(snapshot) => match action {
            TaskAction::Start | TaskAction::Restart
                if matches!(snapshot.state.as_str(), "failed" | "stopped") =>
            {
                Err((
                    ControlError {
                        code: "process_exited".into(),
                        message: match (snapshot.exit_code, safe_docker_status(snapshot)) {
                            (Some(code), _) => format!(
                                "The task exited before start completed (state={}, exit_code={code}).",
                                snapshot.state
                            ),
                            (None, Some(status)) => format!(
                                "The task exited before start completed (state={}, docker_status={status}).",
                                snapshot.state
                            ),
                            (None, None) => format!(
                                "The task exited before start completed (state={}).",
                                snapshot.state
                            ),
                        },
                    },
                    Some(snapshot.clone()),
                ))
            }
            TaskAction::Stop if snapshot.state == "failed" => Err((
                ControlError {
                    code: "not_running".into(),
                    message: "The task had already exited before it could be stopped.".into(),
                },
                Some(snapshot.clone()),
            )),
            TaskAction::Stop if matches!(snapshot.state.as_str(), "running" | "starting") => Err((
                ControlError {
                    code: "stop_incomplete".into(),
                    message: "The stop request completed but the task still appears active.".into(),
                },
                Some(snapshot.clone()),
            )),
            _ => Ok(snapshot.clone()),
        },
    }
}

fn container_error(error: &ContainerError) -> ControlError {
    let (code, message) = match error {
        ContainerError::Missing { container } => {
            ("container_missing", container_missing_message(container))
        }
        ContainerError::DockerUnavailable { detail } => {
            ("docker_unavailable", docker_unavailable_message(detail))
        }
        ContainerError::ActionFailed {
            container,
            action,
            message,
        } => {
            // Docker's own text follows the first separator; what precedes it names the raw ID.
            let detail = message
                .split_once(": ")
                .map_or(message.as_str(), |(_, rest)| rest);
            let container = safe_docker_text(container).unwrap_or_default();
            (
                "container_action_failed",
                match safe_docker_text(detail) {
                    Some(detail) => format!("Could not {action} container {container}: {detail}"),
                    None => format!("Could not {action} container {container}."),
                },
            )
        }
    };
    ControlError {
        code: code.into(),
        message,
    }
}

fn container_missing_message(container: &str) -> String {
    format!(
        "Container {} does not exist. Recreate it, for example with docker compose up, and try again.",
        safe_docker_text(container).unwrap_or_default()
    )
}

fn docker_unavailable_message(detail: &Option<String>) -> String {
    match detail.as_deref().and_then(safe_docker_text) {
        Some(detail) => format!("Docker is unavailable: {detail}"),
        None => "Docker is unavailable.".into(),
    }
}

/// The allowlisted view of Docker's text: the last line, length-limited, with any word that could
/// be a path or a `KEY=value` argument replaced.
fn safe_docker_text(text: &str) -> Option<String> {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let masked = line
        .split_whitespace()
        .map(|word| {
            if word.contains(['/', '\\']) {
                "<path>"
            } else if word.contains('=') {
                "<value>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    Some(
        masked
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_DOCKER_DETAIL_CHARS)
            .collect(),
    )
}

fn safe_docker_status(snapshot: &ManagedTaskSnapshot) -> Option<String> {
    match snapshot.container_detail.as_ref()? {
        ContainerDetail::Listed { status, .. } => safe_docker_text(status),
        _ => None,
    }
}

fn port_in_use_error(port: u16) -> ControlError {
    ControlError {
        code: "port_in_use".into(),
        message: format!(
            "The task's expected port {port} is occupied by a process that is not safely matched to this task."
        ),
    }
}

fn operation_error(action: TaskAction, message: &str) -> ControlError {
    let lower = message.to_lowercase();
    let (code, safe_message) = if lower.contains("profile no longer exists") {
        ("unknown_profile", "The launch profile does not exist.")
    } else if lower.contains("task no longer exists") {
        (
            "unknown_task",
            "The task does not exist in that launch profile.",
        )
    } else if lower.contains("is not running") || lower.contains("not running") {
        ("not_running", "The task is not running.")
    } else if lower.contains("matches more than one registered task") || lower.contains("ambiguous")
    {
        (
            "ambiguous_external_process",
            "More than one registered task could match this process; refresh the service list and resolve the profile configuration.",
        )
    } else if lower.contains("port") && (lower.contains("in use") || lower.contains("occupied")) {
        (
            "port_in_use",
            "The task's expected port is occupied by a process that is not safely matched to this task.",
        )
    } else if lower.contains("directory does not exist") {
        (
            "missing_directory",
            "The task working directory does not exist. Update the saved profile path and try again.",
        )
    } else if lower.contains("permission denied")
        || lower.contains("only stops processes owned")
        || lower.contains("could not verify the service process owner")
        || lower.contains("owner could not be verified")
        || lower.contains("refused to stop that process")
    {
        (
            "permission_denied",
            "The operation was denied by operating-system permissions or process ownership.",
        )
    } else if lower.contains("pid was reused")
        || lower.contains("process changed since the last scan")
        || lower.contains("process already exited")
        || lower.contains("after its owner changed")
    {
        (
            "identity_changed",
            "The matched process changed since the last scan. Refresh status and try again.",
        )
    } else if lower.contains("still has live processes") || lower.contains("did not stop") {
        (
            "termination_timeout",
            "The process tree did not stop within the allowed time. Inspect the task status before retrying.",
        )
    } else if lower.contains("could not start") {
        (
            "spawn_failed",
            "The task process could not be started. Check the saved working directory, executable, and bounded task log.",
        )
    } else if lower.contains("prepare") || lower.contains("maven") || lower.contains("gradle") {
        (
            "prepare_failed",
            "Task preparation or build failed. Read its output with `logs --prepare`.",
        )
    } else if action == TaskAction::Stop {
        (
            "stop_failed",
            "The task could not be stopped safely. Check its status and logs.",
        )
    } else {
        (
            "operation_failed",
            "The task operation failed. Check its status and bounded log output for details.",
        )
    };
    ControlError {
        code: code.into(),
        message: safe_message.into(),
    }
}

fn success(result: Value) -> ControlResponse {
    ControlResponse {
        ok: true,
        accepted: true,
        completed: true,
        operation_id: None,
        result: Some(result),
        error: None,
    }
}

fn failure(code: impl AsRef<str>, message: impl AsRef<str>) -> ControlResponse {
    ControlResponse {
        ok: false,
        accepted: false,
        completed: true,
        operation_id: None,
        result: None,
        error: Some(ControlError {
            code: code.as_ref().into(),
            message: message.as_ref().into(),
        }),
    }
}

fn accepted(operation_id: &str) -> ControlResponse {
    ControlResponse {
        ok: true,
        accepted: true,
        completed: false,
        operation_id: Some(operation_id.into()),
        result: Some(json!({ "operation_id": operation_id, "state": "running" })),
        error: None,
    }
}

fn task_key(profile_id: &str, task_name: &str) -> String {
    format!("{profile_id}\0{task_name}")
}

fn prune_operations(inner: &mut ControlInner) {
    while inner.operations.len() > MAX_RETAINED_OPERATIONS {
        let Some(id) = inner.completed_order.pop_front() else {
            break;
        };
        if inner
            .operations
            .get(&id)
            .is_some_and(|record| record.view.completed_at.is_some())
        {
            inner.operations.remove(&id);
            inner
                .last_operation_by_task
                .retain(|_, last_id| last_id != &id);
        }
    }
}

fn safe_read_error(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("profile") && lower.contains("exist") {
        "The requested launch profile is unavailable.".into()
    } else if lower.contains("log") {
        "The task log could not be read.".into()
    } else {
        "The local control request could not be completed.".into()
    }
}

fn sanitize_task_log(raw: &str) -> String {
    let mut lines = Vec::new();
    for line in raw.lines() {
        if line.starts_with(LOG_MARKER_PREFIX) {
            // Every start marker is written with a blank line of its own ahead of it.
            if line.starts_with(LOG_START_MARKER_PREFIX) && lines.last() == Some(&"") {
                lines.pop();
            }
            continue;
        }
        // Logs written before markers were kept on their own line can have one after output that
        // ended without a newline. What follows the prefix can hold a command line.
        lines.push(
            line.find(LOG_MARKER_PREFIX)
                .map_or(line, |index| &line[..index]),
        );
    }
    lines.join("\n")
}

fn safe_snapshot_message(snapshot: &ManagedTaskSnapshot) -> Option<String> {
    match &snapshot.container_detail {
        Some(ContainerDetail::Missing { container }) => {
            return Some(container_missing_message(container));
        }
        Some(ContainerDetail::DockerUnavailable { detail }) => {
            return Some(docker_unavailable_message(detail));
        }
        Some(ContainerDetail::Listed { .. }) => {
            if let Some(status) = safe_docker_status(snapshot) {
                return Some(format!("Docker status: {status}"));
            }
        }
        None => {}
    }
    Some(match snapshot.state.as_str() {
        "running" if snapshot.external_pid.is_some() => {
            "A matching registered task is running outside Cutting Board.".into()
        }
        "running" => "The task is running.".into(),
        "starting" => "The task is starting.".into(),
        "stopping" => "The task is stopping.".into(),
        "stopped" => "The task is stopped.".into(),
        "failed" => match snapshot.exit_code {
            Some(code) => format!("The task exited with code {code}."),
            None => "The task failed. Check its bounded logs for details.".into(),
        },
        _ => snapshot
            .message
            .as_ref()
            .map(|_| "Task status is available.".into())?,
    })
}

struct LimitedLog {
    content: String,
    truncated: bool,
}

fn limit_log_tail(
    raw: &str,
    max_lines: usize,
    max_bytes: usize,
    source_may_be_truncated: bool,
) -> LimitedLog {
    let tail = truncate_utf8_tail(raw, max_bytes);
    let mut lines = tail.split('\n').collect::<Vec<_>>();
    let line_truncated = lines.len() > max_lines;
    if line_truncated {
        lines.drain(..lines.len() - max_lines);
    }
    let content = lines.join("\n");
    let content = truncate_utf8_tail(&content, max_bytes);
    LimitedLog {
        truncated: source_may_be_truncated
            || tail.len() < raw.len()
            || line_truncated
            || content.len() < lines.join("\n").len(),
        content,
    }
}

fn truncate_utf8_tail(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut start = value.len() - max_bytes;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    value[start..].to_owned()
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(profile_id: &str, task_name: &str) -> TaskRequest {
        TaskRequest {
            profile_id: profile_id.into(),
            task_name: task_name.into(),
        }
    }

    fn snapshot(profile_id: &str, task_name: &str, state: &str) -> ManagedTaskSnapshot {
        ManagedTaskSnapshot {
            profile_id: profile_id.into(),
            task_name: task_name.into(),
            state: state.into(),
            main_pid: Some(42),
            started_at: Some(123),
            exit_code: None,
            message: Some("safe internal detail".into()),
            log_tail: "not part of the control status".into(),
            external_pid: None,
            external_working_directory: Some("/private/path".into()),
            external_log_path: Some("/private/log".into()),
            container_detail: None,
        }
    }

    #[test]
    fn shared_task_guard_rejects_repeated_actions_and_records_ui_snapshot() {
        let state = ControlState::default();
        let request = request("profile", "api");
        let lease = state
            .begin_task_action(&request, TaskAction::Start)
            .unwrap();
        let operation_id = lease.operation_id().to_owned();

        let repeated = state.begin_task_action(&request, TaskAction::Restart);
        assert_eq!(repeated.err().unwrap().code, "busy");

        lease.finish(&Ok(snapshot("profile", "api", "running")));
        let status = state.cached_snapshot("profile", "api").unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(
            status.last_operation.as_ref().unwrap().operation_id,
            operation_id
        );
        assert_eq!(
            state.operation(&operation_id).unwrap().view.state,
            "succeeded"
        );
        assert_eq!(status.message.as_deref(), Some("The task is running."));
        let json = serde_json::to_value(status).unwrap();
        assert!(json.get("command").is_none());
        assert!(json.get("log_tail").is_none());
        assert!(json.get("external_working_directory").is_none());
        assert!(json.get("external_log_path").is_none());
    }

    #[test]
    fn container_task_lifecycle_actions_share_the_same_busy_guard() {
        let state = ControlState::default();
        let request = request("profile", "postgres-container");
        let start = state
            .begin_task_action(&request, TaskAction::Start)
            .unwrap();

        for action in [TaskAction::Stop, TaskAction::Restart] {
            assert_eq!(
                state
                    .begin_task_action(&request, action)
                    .err()
                    .unwrap()
                    .code,
                "busy"
            );
        }
        drop(start);
    }

    #[test]
    fn profile_guard_serializes_task_actions_in_both_directions() {
        let state = ControlState::default();
        let request = request("profile", "api");
        let profile_lease = state.begin_profile_action("profile").unwrap();
        assert_eq!(
            state
                .begin_task_action(&request, TaskAction::Start)
                .err()
                .unwrap()
                .code,
            "busy"
        );
        drop(profile_lease);

        let task_lease = state
            .begin_task_action(&request, TaskAction::Start)
            .unwrap();
        assert_eq!(
            state.begin_profile_action("profile").err().unwrap().code,
            "busy"
        );
        drop(task_lease);
        assert!(state.begin_profile_action("profile").is_ok());
    }

    #[test]
    fn profile_edit_guard_blocks_task_actions_across_profile_id_changes() {
        let state = ControlState::default();
        let edit_lease = state.begin_profile_edit().unwrap();
        assert_eq!(
            state
                .begin_task_action(&request("old-id", "api"), TaskAction::Start)
                .err()
                .unwrap()
                .code,
            "busy"
        );
        drop(edit_lease);

        let task_lease = state
            .begin_task_action(&request("old-id", "api"), TaskAction::Start)
            .unwrap();
        assert_eq!(state.begin_profile_edit().err().unwrap().code, "busy");
        drop(task_lease);
    }

    #[test]
    fn failed_operation_is_completed_with_safe_reason() {
        let state = ControlState::default();
        let request = request("profile", "api");
        let lease = state
            .begin_task_action(&request, TaskAction::Start)
            .unwrap();
        let operation_id = lease.operation_id().to_owned();
        lease.finish(&Err(
            "The task directory does not exist: /private/secret/path".into(),
        ));

        let operation = state.operation(&operation_id).unwrap();
        assert_eq!(operation.view.state, "failed");
        assert_eq!(
            operation.view.error_code.as_deref(),
            Some("missing_directory")
        );
        let message = operation.view.error_message.unwrap();
        assert!(message.contains("working directory"));
        assert!(!message.contains("/private/secret/path"));
    }

    fn failed_with(action: TaskAction, error: impl Into<TaskActionError>) -> ControlError {
        evaluate_action(action, &Err(error.into())).err().unwrap().0
    }

    #[test]
    fn container_failures_get_their_own_codes_whatever_the_container_is_called() {
        let missing = failed_with(
            TaskAction::Start,
            ContainerError::Missing {
                container: "maven-db".into(),
            },
        );
        assert_eq!(missing.code, "container_missing");
        assert!(missing.message.contains("maven-db"));

        let stop_missing = failed_with(
            TaskAction::Stop,
            ContainerError::Missing {
                container: "app-db".into(),
            },
        );
        assert_eq!(stop_missing.code, "container_missing");

        for (action, name) in [
            (TaskAction::Start, "start"),
            (TaskAction::Restart, "restart"),
            (TaskAction::Stop, "stop"),
        ] {
            let error = failed_with(
                action,
                ContainerError::ActionFailed {
                    container: "gradle-cache".into(),
                    action: name,
                    message: format!(
                        "Could not {name} container {}: first line\nError: port is already allocated",
                        "a".repeat(64)
                    ),
                },
            );
            assert_eq!(error.code, "container_action_failed");
            assert_eq!(
                error.message,
                format!(
                    "Could not {name} container gradle-cache: Error: port is already allocated"
                )
            );
        }

        let unavailable = failed_with(
            TaskAction::Start,
            ContainerError::DockerUnavailable {
                detail: Some(
                    "Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is it running?"
                        .into(),
                ),
            },
        );
        assert_eq!(unavailable.code, "docker_unavailable");
        assert_eq!(
            unavailable.message,
            "Docker is unavailable: Cannot connect to the Docker daemon at <path> Is it running?"
        );
    }

    fn begin_restart(control: &ControlState) -> TaskOperationLease {
        control
            .begin_task_action(
                &TaskRequest {
                    profile_id: "p".into(),
                    task_name: "api".into(),
                },
                TaskAction::Restart,
            )
            .unwrap()
    }

    fn progress(state: PrepareState) -> PrepareProgress {
        PrepareProgress {
            state,
            tool: Some("maven"),
            started_at: 100,
            completed_at: None,
        }
    }

    #[test]
    fn a_preparation_is_recorded_on_the_operation_in_flight() {
        let control = ControlState::default();
        control.record_prepare("p", "api", progress(PrepareState::Running));
        assert!(control.last_operation("p", "api").is_none());

        let lease = begin_restart(&control);
        let id = lease.operation_id().to_owned();
        assert_eq!(control.operation(&id).unwrap().view.prepare, None);
        control.record_prepare("p", "api", progress(PrepareState::Running));
        control.record_prepare("p", "other", progress(PrepareState::Failed));
        let view = control.last_operation("p", "api").unwrap();
        assert_eq!(view.prepare.unwrap().state, PrepareState::Running);

        control.record_prepare("p", "api", progress(PrepareState::Succeeded));
        lease.finish(&Err("Could not prepare api.".into()));
        let view = control.operation(&id).unwrap().view;
        assert_eq!(view.prepare.unwrap().state, PrepareState::Succeeded);
    }

    #[test]
    fn a_finished_operation_leaves_no_preparation_running() {
        let control = ControlState::default();
        let lease = begin_restart(&control);
        let finished = lease.operation_id().to_owned();
        control.record_prepare("p", "api", progress(PrepareState::Running));
        lease.finish(&Err("The task operation ended unexpectedly.".into()));
        let prepare = control.operation(&finished).unwrap().view.prepare.unwrap();
        assert_eq!(prepare.state, PrepareState::Interrupted);
        assert!(prepare.completed_at.is_some());

        let lease = begin_restart(&control);
        let dropped = lease.operation_id().to_owned();
        control.record_prepare("p", "api", progress(PrepareState::Running));
        drop(lease);
        let prepare = control.operation(&dropped).unwrap().view.prepare.unwrap();
        assert_eq!(prepare.state, PrepareState::Interrupted);
    }

    #[test]
    fn operation_summaries_report_a_preparation_without_commands_or_paths() {
        let control = ControlState::default();
        let lease = begin_restart(&control);
        control.record_prepare("p", "api", progress(PrepareState::Running));
        let view = serde_json::to_value(control.last_operation("p", "api").unwrap()).unwrap();
        lease.finish(&Err("x".into()));
        assert_eq!(
            view["prepare"],
            json!({ "state": "running", "tool": "maven", "started_at": 100, "completed_at": null })
        );
    }

    #[test]
    fn wait_names_a_build_apart_from_any_other_operation() {
        let operation = ReadinessFacts {
            in_flight: true,
            ..Default::default()
        };
        assert_eq!(
            judge_readiness(&operation),
            ReadinessVerdict::Waiting("operation")
        );
        let building = ReadinessFacts {
            in_flight: true,
            preparing: true,
            ..Default::default()
        };
        assert_eq!(
            judge_readiness(&building),
            ReadinessVerdict::Waiting("prepare")
        );
    }

    #[test]
    fn docker_text_is_the_masked_last_line_within_a_limit() {
        assert_eq!(safe_docker_text(" \n\n"), None);
        assert_eq!(
            safe_docker_text("secret first line\nexec /srv/app/run --token=abc failed\n").unwrap(),
            "exec <path> <value> failed"
        );
        assert_eq!(
            safe_docker_text(&"x".repeat(500)).unwrap().chars().count(),
            MAX_DOCKER_DETAIL_CHARS
        );
        assert_eq!(
            safe_docker_text("bad\u{1b}[31mtext").unwrap(),
            "bad[31mtext"
        );
    }

    #[test]
    fn non_container_failures_keep_their_masked_classification() {
        assert_eq!(
            failed_with(
                TaskAction::Restart,
                "Could not prepare adm: maven failed in /private"
            )
            .code,
            "prepare_failed"
        );
        assert_eq!(
            failed_with(TaskAction::Start, "Could not start api: /private/run").code,
            "spawn_failed"
        );
        let port = failed_with(TaskAction::Start, TaskActionError::PortInUse(8081));
        assert_eq!(port.code, "port_in_use");
        assert!(port.message.contains("8081"));
        assert_eq!(
            TaskActionError::PortInUse(8081).to_string(),
            "The expected port 8081 is already in use by a process that is not safely matched to this task."
        );
        // A number in the text no longer counts: it could be anything, a build's "port 1" included.
        let unrelated = failed_with(TaskAction::Start, "The port is occupied, see port 42 docs");
        assert_eq!(unrelated.code, "port_in_use");
        assert!(!unrelated.message.contains("42"));
        let unknown_port = failed_with(TaskAction::Start, "The port is occupied by /private/app");
        assert_eq!(unknown_port.code, "port_in_use");
        assert!(!unknown_port.message.contains("/private"));
    }

    #[test]
    fn container_status_and_exit_reason_reach_the_caller() {
        let detail = |detail| ManagedTaskSnapshot {
            container_detail: Some(detail),
            ..snapshot("profile", "db", "stopped")
        };
        let listed = detail(ContainerDetail::Listed {
            status: "Exited (1) 2 seconds ago".into(),
            ports: vec![],
        });
        assert_eq!(
            safe_snapshot_message(&listed).as_deref(),
            Some("Docker status: Exited (1) 2 seconds ago")
        );
        let error = evaluate_action(TaskAction::Start, &Ok(listed))
            .err()
            .unwrap()
            .0;
        assert_eq!(error.code, "process_exited");
        assert!(error
            .message
            .contains("docker_status=Exited (1) 2 seconds ago"));

        assert!(safe_snapshot_message(&detail(ContainerDetail::Missing {
            container: "app-db".into()
        }))
        .unwrap()
        .contains("Container app-db does not exist"));
        assert_eq!(
            safe_snapshot_message(&detail(ContainerDetail::DockerUnavailable { detail: None }))
                .as_deref(),
            Some("Docker is unavailable.")
        );
    }

    #[test]
    fn bounded_log_output_drops_manager_command_headers() {
        let raw = format!(
            "=== Cutting Board prepare command · --password=secret ===\n{}",
            (0..30).map(|_| "line\n").collect::<String>()
        );
        let filtered = sanitize_task_log(&raw);
        assert!(!filtered.contains("secret"));
        let bounded = limit_log_tail(&filtered, 5, 32, false);
        assert!(bounded.content.len() <= 32);
        assert!(bounded.content.lines().count() <= 5);
        assert!(bounded.truncated);
    }

    fn ready_on_port() -> ReadinessFacts {
        ReadinessFacts {
            state: "running".into(),
            run_started_at: Some(1_000),
            port: Some(8080),
            port_accepting: true,
            listener_started_at: Some(1_001),
            ..Default::default()
        }
    }

    fn failed_code(verdict: ReadinessVerdict) -> String {
        match verdict {
            ReadinessVerdict::Failed { code, .. } => code,
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    fn failed_operation(code: &str, completed_at: u64) -> Option<FailedOperation> {
        Some(FailedOperation {
            code: code.into(),
            message: "message".into(),
            completed_at,
        })
    }

    #[test]
    fn readiness_accepts_a_listener_of_this_run() {
        assert_eq!(
            judge_readiness(&ready_on_port()),
            ReadinessVerdict::Ready("port")
        );
    }

    #[test]
    fn readiness_is_not_fooled_by_the_old_process_after_a_failed_prepare() {
        let facts = ReadinessFacts {
            failed_operation: failed_operation("prepare_failed", 1_100),
            ..ready_on_port()
        };
        assert_eq!(failed_code(judge_readiness(&facts)), "prepare_failed");
    }

    #[test]
    fn readiness_ignores_a_failed_operation_from_before_this_run() {
        let facts = ReadinessFacts {
            failed_operation: failed_operation("prepare_failed", 999),
            ..ready_on_port()
        };
        assert_eq!(judge_readiness(&facts), ReadinessVerdict::Ready("port"));
    }

    #[test]
    fn readiness_reports_a_failed_operation_when_no_run_is_known() {
        let facts = ReadinessFacts {
            run_started_at: None,
            failed_operation: failed_operation("container_action_failed", 5),
            ..ready_on_port()
        };
        assert_eq!(
            failed_code(judge_readiness(&facts)),
            "container_action_failed"
        );
    }

    #[test]
    fn readiness_rejects_a_listener_that_predates_this_run() {
        let predates = |listener_started_at| ReadinessFacts {
            listener_started_at: Some(listener_started_at),
            ..ready_on_port()
        };
        assert_eq!(
            judge_readiness(&predates(900)),
            ReadinessVerdict::Waiting("listener_predates_run")
        );
        assert_eq!(
            judge_readiness(&predates(997)),
            ReadinessVerdict::Waiting("listener_predates_run")
        );
        // Start times are whole seconds read separately, so a small difference means nothing.
        assert_eq!(
            judge_readiness(&predates(998)),
            ReadinessVerdict::Ready("port")
        );
    }

    #[test]
    fn readiness_accepts_a_connection_to_a_listener_the_scan_cannot_see() {
        let facts = ReadinessFacts {
            listener_started_at: None,
            ..ready_on_port()
        };
        assert_eq!(judge_readiness(&facts), ReadinessVerdict::Ready("port"));
    }

    #[test]
    fn readiness_waits_while_an_operation_runs_or_the_state_is_unknown() {
        let facts = ReadinessFacts {
            in_flight: true,
            failed_operation: failed_operation("prepare_failed", 1_100),
            ..ready_on_port()
        };
        assert_eq!(
            judge_readiness(&facts),
            ReadinessVerdict::Waiting("operation")
        );

        let stale = ReadinessFacts {
            state_stale: true,
            ..ready_on_port()
        };
        assert_eq!(
            judge_readiness(&stale),
            ReadinessVerdict::Waiting("task_state")
        );

        for state in ["starting", "restarting", "stopping", "unknown"] {
            let facts = ReadinessFacts {
                state: state.into(),
                ..ready_on_port()
            };
            assert_eq!(
                judge_readiness(&facts),
                ReadinessVerdict::Waiting("task_state")
            );
        }
    }

    #[test]
    fn readiness_fails_when_the_task_is_not_running() {
        let exited = ReadinessFacts {
            state: "failed".into(),
            exit_code: Some(3),
            ..ready_on_port()
        };
        let ReadinessVerdict::Failed { code, message } = judge_readiness(&exited) else {
            panic!("an exited task cannot be ready");
        };
        assert_eq!(code, "process_exited");
        assert!(message.contains('3'));

        let stopped = ReadinessFacts {
            state: "stopped".into(),
            ..ready_on_port()
        };
        assert_eq!(failed_code(judge_readiness(&stopped)), "not_running");
    }

    #[test]
    fn readiness_needs_a_port_or_a_pattern() {
        let no_signal = ReadinessFacts {
            port: None,
            port_accepting: false,
            ..ready_on_port()
        };
        assert_eq!(
            failed_code(judge_readiness(&no_signal)),
            "readiness_unavailable"
        );

        let closed = ReadinessFacts {
            port_accepting: false,
            ..ready_on_port()
        };
        assert_eq!(judge_readiness(&closed), ReadinessVerdict::Waiting("port"));
    }

    #[test]
    fn a_log_pattern_replaces_the_port_check() {
        let with_pattern = |log_pattern, port_accepting| ReadinessFacts {
            log_pattern: Some(log_pattern),
            port_accepting,
            ..ready_on_port()
        };
        assert_eq!(
            judge_readiness(&with_pattern(LogPatternMatch::Matched, false)),
            ReadinessVerdict::Ready("log_pattern")
        );
        assert_eq!(
            judge_readiness(&with_pattern(LogPatternMatch::NotMatched, true)),
            ReadinessVerdict::Waiting("log_pattern")
        );
        assert_eq!(
            failed_code(judge_readiness(&with_pattern(
                LogPatternMatch::NoRunBoundary,
                true
            ))),
            "run_boundary_unknown"
        );
        let no_port = ReadinessFacts {
            port: None,
            ..with_pattern(LogPatternMatch::Matched, false)
        };
        assert_eq!(
            judge_readiness(&no_port),
            ReadinessVerdict::Ready("log_pattern")
        );
    }

    #[test]
    fn container_readiness_uses_health_then_the_port() {
        let container = |health, port, port_accepting| ReadinessFacts {
            state: "running".into(),
            container: Some(ContainerReadiness {
                health,
                stopped_error: None,
            }),
            port,
            port_accepting,
            ..Default::default()
        };
        assert_eq!(
            judge_readiness(&container(Some(ContainerHealth::Healthy), None, false)),
            ReadinessVerdict::Ready("container_health")
        );
        assert_eq!(
            judge_readiness(&container(
                Some(ContainerHealth::Starting),
                Some(5432),
                true
            )),
            ReadinessVerdict::Waiting("container_health")
        );
        assert_eq!(
            failed_code(judge_readiness(&container(
                Some(ContainerHealth::Unhealthy),
                Some(5432),
                true
            ))),
            "container_unhealthy"
        );
        assert_eq!(
            judge_readiness(&container(None, Some(5432), true)),
            ReadinessVerdict::Ready("container_port")
        );
        assert_eq!(
            judge_readiness(&container(None, Some(5432), false)),
            ReadinessVerdict::Waiting("port")
        );
        assert_eq!(
            failed_code(judge_readiness(&container(None, None, false))),
            "readiness_unavailable"
        );
    }

    #[test]
    fn container_readiness_rejects_patterns_and_explains_a_stopped_container() {
        let with_pattern = ReadinessFacts {
            state: "running".into(),
            container: Some(ContainerReadiness::default()),
            log_pattern: Some(LogPatternMatch::NotMatched),
            ..Default::default()
        };
        assert_eq!(
            failed_code(judge_readiness(&with_pattern)),
            "run_boundary_unknown"
        );

        let missing = ReadinessFacts {
            state: "stopped".into(),
            container: Some(ContainerReadiness {
                health: None,
                stopped_error: Some(ControlError {
                    code: "container_missing".into(),
                    message: container_missing_message("app-db"),
                }),
            }),
            ..Default::default()
        };
        assert_eq!(failed_code(judge_readiness(&missing)), "container_missing");
    }

    fn running_container_after(code: &str) -> ReadinessFacts {
        ReadinessFacts {
            state: "running".into(),
            container: Some(ContainerReadiness {
                health: Some(ContainerHealth::Healthy),
                stopped_error: None,
            }),
            failed_operation: failed_operation(code, 5),
            ..Default::default()
        }
    }

    #[test]
    fn a_running_container_overrides_a_failure_that_said_it_was_gone_or_down() {
        for code in ["container_missing", "docker_unavailable", "process_exited"] {
            assert_eq!(
                judge_readiness(&running_container_after(code)),
                ReadinessVerdict::Ready("container_health"),
                "{code}"
            );
        }
    }

    #[test]
    fn a_failed_container_action_keeps_failing_while_the_container_runs() {
        assert_eq!(
            failed_code(judge_readiness(&running_container_after(
                "container_action_failed"
            ))),
            "container_action_failed"
        );
    }

    #[test]
    fn only_a_fresh_running_container_overrides_a_failed_operation() {
        let stale = ReadinessFacts {
            state_stale: true,
            ..running_container_after("container_missing")
        };
        assert_eq!(failed_code(judge_readiness(&stale)), "container_missing");

        for state in ["starting", "stopped"] {
            let facts = ReadinessFacts {
                state: state.into(),
                ..running_container_after("docker_unavailable")
            };
            assert_eq!(failed_code(judge_readiness(&facts)), "docker_unavailable");
        }

        let plain_task = ReadinessFacts {
            run_started_at: None,
            failed_operation: failed_operation("process_exited", 5),
            ..ready_on_port()
        };
        assert_eq!(failed_code(judge_readiness(&plain_task)), "process_exited");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_container_target_makes_readiness_ask_docker() {
        let temporary = tempfile::tempdir().unwrap();
        let state = crate::tests::test_app_state(temporary.path());
        let task = |name: &str, container: Option<&str>| LaunchTask {
            name: name.into(),
            cwd: ".".into(),
            command: "true".into(),
            expected_port: None,
            container: container.map(str::to_owned),
            prepare: None,
        };
        crate::persist_profile(
            &state.0.profiles_path,
            LaunchProfile {
                id: "p".into(),
                name: "Readiness".into(),
                project_root: temporary.path().to_string_lossy().into_owned(),
                tasks: vec![task("api", None), task("db", Some("app-db"))],
            },
        )
        .unwrap();
        let wait_for = |task_name: &str| {
            dispatch(
                &state,
                ControlRequest::Readiness {
                    profile_id: "p".into(),
                    task_name: task_name.into(),
                    log_pattern: None,
                },
            )
        };
        // Every snapshot gets cached, whatever Docker answers, so a listing would leave one for db.
        let db_listed = || {
            state
                .control_service_state()
                .cached_snapshot("p", "db")
                .is_some()
        };

        assert_eq!(wait_for("api").error.unwrap().code, "not_running");
        assert!(state
            .control_service_state()
            .cached_snapshot("p", "api")
            .is_some());
        assert!(!db_listed());

        wait_for("db");
        assert!(db_listed());
    }

    #[cfg(unix)]
    #[test]
    fn readiness_of_a_live_managed_task_looks_only_at_that_task_and_scans_only_when_needed() {
        use crate::models::WorkspaceSnapshot;
        use std::net::{Ipv4Addr, TcpListener};

        let temporary = tempfile::tempdir().unwrap();
        let state = crate::tests::test_app_state(temporary.path());
        let unused_port = || {
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .unwrap()
                .local_addr()
                .unwrap()
                .port()
        };
        let task = |name: &str, port| LaunchTask {
            name: name.into(),
            cwd: ".".into(),
            command: "sleep 30".into(),
            expected_port: Some(port),
            container: None,
            prepare: None,
        };
        let refused_port = unused_port();
        let open = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let open_port = open.local_addr().unwrap().port();
        let profiles = crate::persist_profile(
            &state.0.profiles_path,
            LaunchProfile {
                id: "p".into(),
                name: "Readiness".into(),
                project_root: temporary.path().to_string_lossy().into_owned(),
                tasks: vec![
                    task("api", refused_port),
                    task("open", open_port),
                    task("sibling", unused_port()),
                ],
            },
        )
        .unwrap();
        let start = |task_name: &str| {
            let request = TaskRequest {
                profile_id: "p".into(),
                task_name: task_name.into(),
            };
            state
                .0
                .launch
                .lock()
                .unwrap()
                .start_task(&profiles, &request, &state.0.logs_dir, None)
                .unwrap();
        };
        start("api");
        start("open");
        let poll = |task_name: &str| {
            dispatch(
                &state,
                ControlRequest::Readiness {
                    profile_id: "p".into(),
                    task_name: task_name.into(),
                    log_pattern: None,
                },
            )
        };
        // A scan replaces whatever is cached, so a marked one still being there means no scan ran.
        let seed_scan = |scanned_at: u64| {
            state.0.scan.lock().unwrap().workspace = Some(WorkspaceSnapshot {
                services: vec![],
                scanned_at,
                scan_duration_ms: 0,
                endpoint_count: 0,
                errors: vec!["seeded".into()],
            });
        };
        let scanned = || {
            state
                .0
                .scan
                .lock()
                .unwrap()
                .workspace
                .as_ref()
                .is_some_and(|workspace| workspace.errors != ["seeded"])
        };

        seed_scan(epoch_seconds());
        let waiting = poll("api");
        assert_eq!(waiting.result.unwrap()["waiting_for"], "port");
        assert!(!scanned(), "a refused connection needs no scan");
        assert!(state
            .control_service_state()
            .cached_snapshot("p", "sibling")
            .is_none());

        seed_scan(0);
        poll("api");
        assert!(scanned(), "an old scan is not used forever");

        seed_scan(epoch_seconds());
        poll("open");
        assert!(scanned(), "an accepted connection is dated by a new scan");

        state.0.launch.lock().unwrap().stop_all();
        seed_scan(epoch_seconds());
        assert!(!poll("api").ok);
        assert!(
            scanned(),
            "only a new scan can show a stopped task elsewhere"
        );
    }

    #[test]
    fn docker_status_gives_the_container_health() {
        assert_eq!(
            container_health("Up 3 minutes (healthy)"),
            Some(ContainerHealth::Healthy)
        );
        assert_eq!(
            container_health("Up 3 seconds (health: starting)"),
            Some(ContainerHealth::Starting)
        );
        assert_eq!(
            container_health("Up 3 minutes (unhealthy)"),
            Some(ContainerHealth::Unhealthy)
        );
        assert_eq!(container_health("Up 3 minutes"), None);
    }

    #[test]
    fn sanitizer_removes_the_blank_line_written_for_a_start_marker_only() {
        let raw = "first\n\n=== Cutting Board start 1 · cmd ===\n=== Cutting Board task metadata {} ===\nsecond\n\nreal blank above\n\n=== Cutting Board prepare command · cmd ===\nthird\n";
        assert_eq!(
            sanitize_task_log(raw),
            "first\nsecond\n\nreal blank above\n\nthird"
        );
    }

    #[test]
    fn sanitizer_cuts_a_marker_that_follows_output_on_the_same_line() {
        let raw = "partial=== Cutting Board prepare command · --password=secret ===\nnext\n";
        let sanitized = sanitize_task_log(raw);
        assert_eq!(sanitized, "partial\nnext");
        assert!(!sanitized.contains("secret"));
    }

    #[test]
    fn task_update_trims_values_and_records_only_real_changes() {
        let mut task = LaunchTask {
            name: "api".into(),
            cwd: ".".into(),
            command: "old".into(),
            expected_port: Some(1),
            container: None,
            prepare: None,
        };
        let changed = apply_task_update(
            &mut task,
            &TaskUpdate {
                command: Some("  new  ".into()),
                cwd: Some(" . ".into()),
                clear_expected_port: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(task.command, "new");
        assert_eq!(task.expected_port, None);
        assert_eq!(
            Value::Object(changed),
            json!({
                "command": { "old": "old", "new": "new" },
                "expected_port": { "old": 1, "new": null }
            })
        );
    }

    #[test]
    fn task_update_refuses_command_and_prepare_on_a_container() {
        let mut task = LaunchTask {
            name: "db".into(),
            cwd: ".".into(),
            command: String::new(),
            expected_port: None,
            container: Some("app-db".into()),
            prepare: None,
        };
        for update in [
            TaskUpdate {
                command: Some("run".into()),
                ..Default::default()
            },
            TaskUpdate {
                clear_prepare: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                apply_task_update(&mut task, &update).unwrap_err().code,
                "invalid_request"
            );
        }
        let changed = apply_task_update(
            &mut task,
            &TaskUpdate {
                expected_port: Some(5432),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(changed.contains_key("expected_port"));
    }
}
