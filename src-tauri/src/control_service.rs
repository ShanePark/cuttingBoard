use crate::{
    control::{ControlError, ControlRequest, ControlResponse},
    models::{LaunchProfile, LaunchTask, ManagedTaskSnapshot, TaskRequest},
    AppState,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_RUNNING_OPERATIONS: usize = 8;
const MAX_RETAINED_OPERATIONS: usize = 64;
const DEFAULT_LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 1_000;
const DEFAULT_LOG_BYTES: usize = 65_536;
const MAX_LOG_BYTES: usize = 65_536;

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
    pub(crate) fn finish(mut self, result: &Result<ManagedTaskSnapshot, String>) {
        self.finish_inner(result);
    }

    fn finish_inner(&mut self, result: &Result<ManagedTaskSnapshot, String>) {
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
        } => logs(state, &profile_id, &task_name, lines, bytes),
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
    let Some(profile) = profiles.iter().find(|profile| profile.id == profile_id) else {
        return failure("unknown_profile", "The launch profile does not exist.");
    };
    let Some(task) = profile.tasks.iter().find(|task| task.name == task_name) else {
        return failure(
            "unknown_task",
            "The task does not exist in that launch profile.",
        );
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
) -> ControlResponse {
    let lines = lines
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LOG_LINES);
    let bytes = bytes
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LOG_BYTES);
    if !(1..=MAX_LOG_LINES).contains(&lines) || !(1..=MAX_LOG_BYTES).contains(&bytes) {
        return failure(
            "invalid_log_limit",
            "Log limits must be between 1 and 1000 lines and between 1 and 65536 bytes.",
        );
    }
    let profiles = match crate::control_profiles(state) {
        Ok(profiles) => profiles,
        Err(error) => return failure("configuration_error", safe_read_error(&error)),
    };
    let Some(task) = find_task(&profiles, profile_id, task_name) else {
        return failure(
            "unknown_task",
            "The profile/task identifier does not exist.",
        );
    };
    let request = TaskRequest {
        profile_id: profile_id.into(),
        task_name: task_name.into(),
    };
    let raw = match crate::control_task_log_tail(state, &profiles, &request) {
        Ok(logs) => logs,
        Err(error) => return failure("log_read_failed", safe_read_error(&error)),
    };
    let is_container = task.container_name().is_some();
    let source_may_be_truncated = if is_container {
        raw.lines().count() >= 200
    } else {
        raw.len() >= DEFAULT_LOG_BYTES
    };
    let filtered = if is_container {
        raw
    } else {
        sanitize_task_log(&raw)
    };
    let limited = limit_log_tail(&filtered, lines, bytes, source_may_be_truncated);
    success(json!({
        "profile_id": profile_id,
        "task_name": task_name,
        "logs": limited.content,
        "lines": if limited.content.is_empty() { 0 } else { limited.content.lines().count() },
        "bytes": limited.content.len(),
        "truncated": limited.truncated
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
    if find_task(&profiles, &profile_id, &task_name).is_none() {
        let code = if profiles.iter().any(|profile| profile.id == profile_id) {
            "unknown_task"
        } else {
            "unknown_profile"
        };
        return failure(code, "The profile/task identifier does not exist.");
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
    result: &Result<ManagedTaskSnapshot, String>,
) -> Result<ManagedTaskSnapshot, (ControlError, Option<ManagedTaskSnapshot>)> {
    match result {
        Err(message) => Err((operation_error(action, message), None)),
        Ok(snapshot) => match action {
            TaskAction::Start | TaskAction::Restart
                if matches!(snapshot.state.as_str(), "failed" | "stopped") =>
            {
                Err((
                    ControlError {
                        code: "process_exited".into(),
                        message: match snapshot.exit_code {
                            Some(code) => format!(
                                "The task exited before start completed (state={}, exit_code={code}).",
                                snapshot.state
                            ),
                            None => format!(
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
            "Task preparation or build failed. Check the bounded task log for details.",
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

fn find_task<'a>(
    profiles: &'a [LaunchProfile],
    profile_id: &str,
    task_name: &str,
) -> Option<&'a LaunchTask> {
    profiles
        .iter()
        .find(|profile| profile.id == profile_id)?
        .tasks
        .iter()
        .find(|task| task.name == task_name)
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
    raw.lines()
        .filter(|line| !line.starts_with("=== Cutting Board "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn safe_snapshot_message(snapshot: &ManagedTaskSnapshot) -> Option<String> {
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
}
