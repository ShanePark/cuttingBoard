use crate::cli::ControlCommand;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CONTROL_DIRECTORY: &str = "control";
const SOCKET_NAME: &str = "control.sock";
const LOCK_NAME: &str = "control.lock";
const MAX_REQUEST_BYTES: usize = 64 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const OPERATION_POLL_INTERVAL: Duration = Duration::from_millis(150);
const READINESS_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a `wait` tolerates a socket nothing listens on after reaching the app, so that a quick
/// app restart does not end it.
const APP_GONE_GRACE: Duration = Duration::from_secs(5);
const MAX_LOG_PATTERN_BYTES: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ControlRequest {
    List {},
    Status {
        profile_id: String,
        task_name: String,
    },
    Logs {
        profile_id: String,
        task_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bytes: Option<u32>,
        // Left out when unset so that plain `logs` keeps working against an older app.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        current_run: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        prepare: bool,
    },
    Readiness {
        profile_id: String,
        task_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        log_pattern: Option<String>,
    },
    Show {
        profile_id: String,
        task_name: String,
    },
    Update {
        profile_id: String,
        task_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_port: Option<u16>,
        #[serde(default)]
        clear_expected_port: bool,
        #[serde(default)]
        clear_prepare: bool,
    },
    Start {
        profile_id: String,
        task_name: String,
    },
    Stop {
        profile_id: String,
        task_name: String,
    },
    Restart {
        profile_id: String,
        task_name: String,
    },
    Operation {
        operation_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlResponse {
    pub(crate) ok: bool,
    pub(crate) accepted: bool,
    pub(crate) completed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<ControlError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlError {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl ControlResponse {
    pub(crate) fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            accepted: false,
            completed: true,
            operation_id: None,
            result: None,
            error: Some(ControlError {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

impl ControlRequest {
    fn validate(&self) -> Result<(), String> {
        let validate_task = |profile_id: &str, task_name: &str| {
            if profile_id.trim().is_empty() || profile_id.len() > 128 {
                return Err("profile_id must contain 1 to 128 bytes.".to_string());
            }
            if task_name.trim().is_empty() || task_name.len() > 80 {
                return Err("task_name must contain 1 to 80 bytes.".to_string());
            }
            Ok(())
        };
        match self {
            Self::List {} => Ok(()),
            Self::Status {
                profile_id,
                task_name,
            }
            | Self::Show {
                profile_id,
                task_name,
            }
            | Self::Start {
                profile_id,
                task_name,
            }
            | Self::Stop {
                profile_id,
                task_name,
            }
            | Self::Restart {
                profile_id,
                task_name,
            } => validate_task(profile_id, task_name),
            Self::Logs {
                profile_id,
                task_name,
                lines,
                bytes,
                current_run,
                prepare,
            } => {
                validate_task(profile_id, task_name)?;
                if *current_run && *prepare {
                    return Err("--current-run and --prepare cannot be combined.".into());
                }
                if lines.is_some_and(|value| !(1..=1_000).contains(&value)) {
                    return Err("lines must be from 1 to 1000.".into());
                }
                if bytes.is_some_and(|value| !(1..=65_536).contains(&value)) {
                    return Err("bytes must be from 1 to 65536.".into());
                }
                Ok(())
            }
            Self::Readiness {
                profile_id,
                task_name,
                log_pattern,
            } => {
                validate_task(profile_id, task_name)?;
                if log_pattern.as_deref().is_some_and(|pattern| {
                    pattern.trim().is_empty()
                        || pattern.len() > MAX_LOG_PATTERN_BYTES
                        || pattern.contains(['\n', '\r'])
                }) {
                    return Err(format!(
                        "log_pattern must be a single line of 1 to {MAX_LOG_PATTERN_BYTES} bytes."
                    ));
                }
                Ok(())
            }
            Self::Update {
                profile_id,
                task_name,
                command,
                cwd,
                expected_port,
                clear_expected_port,
                clear_prepare,
            } => {
                validate_task(profile_id, task_name)?;
                if command.is_none()
                    && cwd.is_none()
                    && expected_port.is_none()
                    && !clear_expected_port
                    && !clear_prepare
                {
                    return Err("update requires at least one change.".into());
                }
                // A line break would let the rest of a saved value escape the log marker that
                // carries it, and with it the log filter.
                for (name, value) in [("command", command), ("cwd", cwd)] {
                    if value.as_deref().is_some_and(|value| {
                        value.trim().is_empty() || value.contains(['\n', '\r'])
                    }) {
                        return Err(format!("{name} must be a single non-empty line."));
                    }
                }
                if *expected_port == Some(0) {
                    return Err("expected_port must be from 1 to 65535.".into());
                }
                if expected_port.is_some() && *clear_expected_port {
                    return Err("expected_port and clear_expected_port cannot be combined.".into());
                }
                Ok(())
            }
            Self::Operation { operation_id } => {
                if operation_id.trim().is_empty() || operation_id.len() > 128 {
                    return Err("operation_id must contain 1 to 128 bytes.".into());
                }
                Ok(())
            }
        }
    }
}

#[cfg(unix)]
struct ServerIdentity {
    device: u64,
    inode: u64,
    uid: u32,
}

#[cfg(unix)]
pub(crate) struct ControlServer {
    stop: Arc<AtomicBool>,
    listener_thread: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    identity: ServerIdentity,
    _instance_lock: std::fs::File,
}

#[cfg(unix)]
impl Drop for ControlServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.listener_thread.take() {
            let _ = thread.join();
        }
        remove_socket_if_unchanged(&self.socket_path, &self.identity);
    }
}

#[cfg(not(unix))]
pub(crate) struct ControlServer;

#[cfg(unix)]
pub(crate) fn start_server<F>(handler: F) -> Result<ControlServer, String>
where
    F: Fn(ControlRequest) -> ControlResponse + Send + Sync + 'static,
{
    start_server_at(control_socket_path()?, handler)
}

#[cfg(not(unix))]
pub(crate) fn start_server<F>(_handler: F) -> Result<ControlServer, String>
where
    F: Fn(ControlRequest) -> ControlResponse + Send + Sync + 'static,
{
    Err("External control is unsupported on this platform.".into())
}

#[cfg(unix)]
pub(crate) fn start_server_at<F>(socket_path: PathBuf, handler: F) -> Result<ControlServer, String>
where
    F: Fn(ControlRequest) -> ControlResponse + Send + Sync + 'static,
{
    use std::os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixListener,
    };

    let control_dir = socket_path
        .parent()
        .ok_or_else(|| "The control socket path has no parent directory.".to_string())?;
    ensure_private_control_directory(control_dir)?;
    ensure_socket_path_fits(&socket_path)?;
    let instance_lock = acquire_instance_lock(control_dir)?;
    remove_stale_socket(&socket_path)?;

    let listener = UnixListener::bind(&socket_path)
        .map_err(|error| format!("Could not create the control socket: {error}"))?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Could not secure the control socket: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure the control socket: {error}"))?;

    let metadata = std::fs::symlink_metadata(&socket_path)
        .map_err(|error| format!("Could not inspect the control socket: {error}"))?;
    if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
        return Err("The control socket has an unsafe owner or file type.".into());
    }
    let identity = ServerIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        uid: metadata.uid(),
    };

    let stop = Arc::new(AtomicBool::new(false));
    let worker_count = Arc::new(AtomicUsize::new(0));
    let thread_stop = Arc::clone(&stop);
    let handler = Arc::new(handler);
    let listener_thread = thread::Builder::new()
        .name("cutting-board-control-listener".into())
        .spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let Some(guard) = WorkerGuard::try_new(Arc::clone(&worker_count)) else {
                            let _ = write_response(
                                stream,
                                &ControlResponse::error(
                                    "busy",
                                    "The control server is handling its maximum number of requests.",
                                ),
                            );
                            continue;
                        };
                        let handler = Arc::clone(&handler);
                        let _ = thread::Builder::new()
                            .name("cutting-board-control-request".into())
                            .spawn(move || {
                                let _guard = guard;
                                serve_connection(stream, handler.as_ref());
                            });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) if thread_stop.load(Ordering::Acquire) => break,
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                }
            }
        })
        .map_err(|error| format!("Could not start the control listener: {error}"))?;

    Ok(ControlServer {
        stop,
        listener_thread: Some(listener_thread),
        socket_path,
        identity,
        _instance_lock: instance_lock,
    })
}

#[cfg(unix)]
struct WorkerGuard(Arc<AtomicUsize>);

#[cfg(unix)]
impl WorkerGuard {
    fn try_new(count: Arc<AtomicUsize>) -> Option<Self> {
        let mut current = count.load(Ordering::Acquire);
        loop {
            if current >= MAX_CONNECTIONS {
                return None;
            }
            match count.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self(count)),
                Err(next) => current = next,
            }
        }
    }
}

#[cfg(unix)]
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(unix)]
fn serve_connection<F>(mut stream: std::os::unix::net::UnixStream, handler: &F)
where
    F: Fn(ControlRequest) -> ControlResponse,
{
    if stream.set_read_timeout(Some(IO_TIMEOUT)).is_err()
        || stream.set_write_timeout(Some(IO_TIMEOUT)).is_err()
        || !peer_user_is_current(&stream)
    {
        let _ = write_response(
            stream,
            &ControlResponse::error("unavailable", "The control peer could not be verified."),
        );
        return;
    }
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(message) => {
            let _ = write_response(stream, &ControlResponse::error("invalid_request", message));
            return;
        }
    };
    if let Err(message) = request.validate() {
        let _ = write_response(stream, &ControlResponse::error("invalid_request", message));
        return;
    }
    let response = handler(request);
    let _ = write_response(stream, &response);
}

#[cfg(unix)]
fn read_request(stream: &mut std::os::unix::net::UnixStream) -> Result<ControlRequest, String> {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    let read = (&mut reader)
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|error| format!("Could not read the request: {error}"))?;
    if read == 0 {
        return Err("The request was empty.".into());
    }
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("The request exceeds the 65536-byte limit.".into());
    }
    if bytes.last() != Some(&b'\n') {
        return Err("The request must end with a newline.".into());
    }
    bytes.pop();
    serde_json::from_slice(&bytes).map_err(|error| format!("Invalid control request: {error}"))
}

#[cfg(unix)]
fn write_response(
    mut stream: std::os::unix::net::UnixStream,
    response: &ControlResponse,
) -> io::Result<()> {
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let bytes = match serde_json::to_vec(response) {
        Ok(bytes) if bytes.len() + 1 <= MAX_RESPONSE_BYTES => bytes,
        _ => serde_json::to_vec(&ControlResponse::error(
            "response_too_large",
            "The response exceeds the 1048576-byte limit.",
        ))
        .unwrap_or_else(|_| b"{}".to_vec()),
    };
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

#[cfg(unix)]
fn acquire_instance_lock(control_dir: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    };

    let lock_path = control_dir.join(LOCK_NAME);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&lock_path)
        .map_err(|error| format!("Could not open the control lock: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("Could not inspect the control lock: {error}"))?;
    if !metadata.is_file() || metadata.uid() != effective_uid() {
        return Err("The control lock has an unsafe owner or file type.".into());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Could not secure the control lock: {error}"))?;
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked != 0 {
        return Err("A Cutting Board control server is already active.".into());
    }
    Ok(file)
}

#[cfg(unix)]
fn ensure_private_control_directory(control_dir: &Path) -> Result<(), String> {
    use std::os::unix::{fs::MetadataExt, fs::PermissionsExt};

    std::fs::create_dir_all(control_dir)
        .map_err(|error| format!("Could not create the control directory: {error}"))?;
    let metadata = std::fs::symlink_metadata(control_dir)
        .map_err(|error| format!("Could not inspect the control directory: {error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != effective_uid()
    {
        return Err("The control directory has an unsafe owner or file type.".into());
    }
    std::fs::set_permissions(control_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not secure the control directory: {error}"))
}

#[cfg(unix)]
fn remove_stale_socket(socket_path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not inspect the control socket: {error}")),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
        return Err("Refusing to replace a non-socket or foreign-owned control path.".into());
    }
    match std::os::unix::net::UnixStream::connect(socket_path) {
        Ok(_) => Err("A Cutting Board control server is already active.".into()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            let current = std::fs::symlink_metadata(socket_path)
                .map_err(|error| format!("Could not recheck the stale control socket: {error}"))?;
            if !current.file_type().is_socket()
                || current.uid() != effective_uid()
                || current.dev() != metadata.dev()
                || current.ino() != metadata.ino()
            {
                return Err("The control socket changed while checking for a stale path.".into());
            }
            std::fs::remove_file(socket_path)
                .map_err(|error| format!("Could not remove the stale control socket: {error}"))
        }
        Err(error) => Err(format!(
            "Could not check the existing control socket: {error}"
        )),
    }
}

#[cfg(unix)]
fn remove_socket_if_unchanged(socket_path: &Path, identity: &ServerIdentity) {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let Ok(current) = std::fs::symlink_metadata(socket_path) else {
        return;
    };
    if current.file_type().is_socket()
        && current.uid() == identity.uid
        && current.dev() == identity.device
        && current.ino() == identity.inode
    {
        let _ = std::fs::remove_file(socket_path);
    }
}

#[cfg(unix)]
fn ensure_socket_path_fits(socket_path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;

    if socket_path.as_os_str().as_bytes().len() > 103 {
        return Err("The control socket path is too long for a Unix-domain socket.".into());
    }
    Ok(())
}

#[cfg(unix)]
fn peer_user_is_current(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd;

    #[cfg(target_os = "linux")]
    {
        let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut credentials as *mut _ as *mut libc::c_void,
                &mut length,
            )
        };
        return result == 0 && credentials.uid == effective_uid();
    }
    #[cfg(target_os = "macos")]
    {
        let mut uid = 0;
        let mut gid = 0;
        return unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0
            && uid == effective_uid();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = stream;
        true
    }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

pub(crate) fn control_socket_path() -> Result<PathBuf, String> {
    let config_root = dirs::config_dir()
        .ok_or_else(|| "The user configuration directory is unavailable.".to_string())?;
    let config: Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .map_err(|error| format!("Could not read the application identifier: {error}"))?;
    let identifier = config
        .get("identifier")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && !value.contains('/')
                && !value.contains('\\')
                && *value != "."
                && *value != ".."
        })
        .ok_or_else(|| "The application identifier is invalid.".to_string())?;
    Ok(config_root
        .join(identifier)
        .join(CONTROL_DIRECTORY)
        .join(SOCKET_NAME))
}

#[cfg(unix)]
pub(crate) fn run_cli(command: ControlCommand) -> i32 {
    match command {
        ControlCommand::Help => {
            crate::cli::print_control_help();
            0
        }
        ControlCommand::List => run_read_request(ControlRequest::List {}),
        ControlCommand::Status {
            profile_id,
            task_name,
        } => run_read_request(ControlRequest::Status {
            profile_id,
            task_name,
        }),
        ControlCommand::Logs {
            profile_id,
            task_name,
            lines,
            max_bytes,
            current_run,
            prepare,
        } => run_read_request(ControlRequest::Logs {
            profile_id,
            task_name,
            lines,
            bytes: max_bytes,
            current_run,
            prepare,
        }),
        ControlCommand::Wait {
            profile_id,
            task_name,
            log_pattern,
            timeout_seconds,
        } => run_wait(
            ControlRequest::Readiness {
                profile_id,
                task_name,
                log_pattern,
            },
            timeout_seconds,
        ),
        ControlCommand::Show {
            profile_id,
            task_name,
        } => run_read_request(ControlRequest::Show {
            profile_id,
            task_name,
        }),
        ControlCommand::Update {
            profile_id,
            task_name,
            command,
            cwd,
            expected_port,
            clear_expected_port,
            clear_prepare,
            timeout_seconds,
        } => run_action(
            ControlRequest::Update {
                profile_id,
                task_name,
                command,
                cwd,
                expected_port,
                clear_expected_port,
                clear_prepare,
            },
            timeout_seconds,
        ),
        ControlCommand::Operation { operation_id } => {
            run_read_request(ControlRequest::Operation { operation_id })
        }
        ControlCommand::Start {
            profile_id,
            task_name,
            timeout_seconds,
        } => run_action(
            ControlRequest::Start {
                profile_id,
                task_name,
            },
            timeout_seconds,
        ),
        ControlCommand::Stop {
            profile_id,
            task_name,
            timeout_seconds,
        } => run_action(
            ControlRequest::Stop {
                profile_id,
                task_name,
            },
            timeout_seconds,
        ),
        ControlCommand::Restart {
            profile_id,
            task_name,
            timeout_seconds,
        } => run_action(
            ControlRequest::Restart {
                profile_id,
                task_name,
            },
            timeout_seconds,
        ),
    }
}

#[cfg(not(unix))]
pub(crate) fn run_cli(_command: ControlCommand) -> i32 {
    print_response(&ControlResponse::error(
        "unavailable",
        "External control is unsupported on this platform.",
    ));
    EXIT_UNAVAILABLE
}

pub(crate) fn print_invalid_cli_error(message: &str) -> i32 {
    print_response(&ControlResponse::error("invalid_request", message));
    EXIT_INVALID
}

#[cfg(unix)]
fn run_read_request(request: ControlRequest) -> i32 {
    match send_request_with_deadline(&request, Instant::now() + IO_TIMEOUT) {
        Ok(response) => {
            let code = response_exit_code(&response);
            print_response(&response);
            code
        }
        Err(error) => {
            let code = if error.timed_out {
                "timeout"
            } else if error.busy {
                "busy"
            } else {
                "unavailable"
            };
            print_response(&ControlResponse::error(code, error.message));
            if code == "timeout" {
                EXIT_TIMEOUT
            } else if code == "busy" {
                EXIT_BUSY
            } else {
                EXIT_UNAVAILABLE
            }
        }
    }
}

#[cfg(unix)]
fn run_action(request: ControlRequest, timeout_seconds: u32) -> i32 {
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout_seconds));
    let mut response = match send_request_with_deadline(&request, deadline) {
        Ok(response) => response,
        Err(error) if error.acknowledgement_unknown => {
            print_response(&ControlResponse::error(
                "acknowledgement_unknown",
                format!(
                    "The request may have been accepted, but no response arrived: {}. Check task status before retrying.",
                    error.message
                ),
            ));
            return EXIT_UNAVAILABLE;
        }
        Err(error) if error.busy => {
            print_response(&ControlResponse::error("busy", error.message));
            return EXIT_BUSY;
        }
        Err(error) if error.timed_out => {
            print_response(&ControlResponse::error("timeout", error.message));
            return EXIT_TIMEOUT;
        }
        Err(error) => {
            print_response(&ControlResponse::error("unavailable", error.message));
            return EXIT_UNAVAILABLE;
        }
    };
    if !response.ok || response.completed {
        let code = response_exit_code(&response);
        print_response(&response);
        return code;
    }
    let Some(operation_id) = response.operation_id.clone() else {
        print_response(&ControlResponse::error(
            "action_failed",
            "The server accepted the request without returning an operation ID.",
        ));
        return EXIT_ACTION_FAILED;
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            print_response(&response);
            return EXIT_TIMEOUT;
        }
        thread::sleep(OPERATION_POLL_INTERVAL.min(remaining));
        if Instant::now() >= deadline {
            print_response(&response);
            return EXIT_TIMEOUT;
        }
        match send_request_with_deadline(
            &ControlRequest::Operation {
                operation_id: operation_id.clone(),
            },
            deadline,
        ) {
            Ok(next) => {
                response = next;
                if response.completed || !response.ok {
                    let code = response_exit_code(&response);
                    print_response(&response);
                    return code;
                }
            }
            Err(_) if Instant::now() >= deadline => {
                print_response(&response);
                return EXIT_TIMEOUT;
            }
            Err(_) => continue,
        }
    }
}

#[cfg(unix)]
fn run_wait(request: ControlRequest, timeout_seconds: u32) -> i32 {
    let response = wait_for_readiness(
        &request,
        Duration::from_secs(u64::from(timeout_seconds)),
        send_request_with_deadline,
    );
    print_response(&response);
    response_exit_code(&response)
}

/// Asks the app once per interval. The server only ever answers for the moment it is asked, so a
/// slow service holds no connection slot, and giving up here cancels nothing.
#[cfg(unix)]
fn wait_for_readiness(
    request: &ControlRequest,
    timeout: Duration,
    mut send: impl FnMut(&ControlRequest, Instant) -> Result<ControlResponse, ControlTransportError>,
) -> ControlResponse {
    let deadline = Instant::now() + timeout;
    let mut last_judgement = None;
    let mut reached_app = false;
    let mut transport_error = None;
    let mut nothing_listening_since: Option<Instant> = None;
    loop {
        match send(request, deadline.min(Instant::now() + IO_TIMEOUT)) {
            Ok(response) => {
                reached_app = true;
                nothing_listening_since = None;
                let is_busy = response
                    .error
                    .as_ref()
                    .is_some_and(|error| error.code == "busy");
                if !response.ok && !is_busy {
                    return response;
                }
                if response.ok {
                    let ready = response
                        .result
                        .as_ref()
                        .and_then(|result| result.get("ready"))
                        .and_then(Value::as_bool);
                    if ready != Some(false) {
                        return response;
                    }
                    last_judgement = response.result;
                }
            }
            Err(error) => {
                // A request that went out found something listening, even if no answer came back.
                reached_app |= error.acknowledgement_unknown;
                // Only a failed connection says the app is gone. One that is merely slow or busy
                // to answer is still there.
                if error.acknowledgement_unknown || error.timed_out || error.busy {
                    nothing_listening_since = None;
                } else {
                    nothing_listening_since.get_or_insert_with(Instant::now);
                }
                transport_error = Some(error.message);
            }
        }
        if reached_app
            && nothing_listening_since.is_some_and(|since| since.elapsed() >= APP_GONE_GRACE)
        {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(READINESS_POLL_INTERVAL.min(remaining));
        if Instant::now() >= deadline {
            break;
        }
    }
    if !reached_app || nothing_listening_since.is_some() {
        return ControlResponse::error(
            "unavailable",
            transport_error.unwrap_or_else(|| "The control socket did not respond.".into()),
        );
    }
    ControlResponse {
        ok: false,
        accepted: true,
        completed: false,
        operation_id: None,
        result: last_judgement,
        error: Some(ControlError {
            code: "timeout".into(),
            message: format!(
                "The task was not ready within {} seconds. Nothing was cancelled; it may still become ready.",
                timeout.as_secs()
            ),
        }),
    }
}

/// A newer CLI talking to an app that has not been restarted since the update.
#[cfg(unix)]
fn explain_unsupported_request(mut response: ControlResponse) -> ControlResponse {
    if let Some(error) = response.error.as_mut().filter(|error| {
        error.code == "invalid_request"
            && error.message.starts_with("Invalid control request:")
            && (error.message.contains("unknown variant")
                || error.message.contains("unknown field"))
    }) {
        error.message =
            "The running Cutting Board app does not support this request. Restart the app so it matches this CLI.".into();
    }
    response
}

#[cfg(unix)]
struct ControlTransportError {
    message: String,
    acknowledgement_unknown: bool,
    timed_out: bool,
    busy: bool,
}

#[cfg(unix)]
impl ControlTransportError {
    fn before_send(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            acknowledgement_unknown: false,
            timed_out: false,
            busy: false,
        }
    }

    fn before_deadline(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            acknowledgement_unknown: false,
            timed_out: true,
            busy: false,
        }
    }

    fn before_busy(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            acknowledgement_unknown: false,
            timed_out: false,
            busy: true,
        }
    }

    fn after_send(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            acknowledgement_unknown: true,
            timed_out: false,
            busy: false,
        }
    }

    fn after_deadline(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            acknowledgement_unknown: true,
            timed_out: true,
            busy: false,
        }
    }

    fn after_send_io(context: &str, error: io::Error) -> Self {
        let timed_out = error.kind() == io::ErrorKind::TimedOut;
        Self {
            message: format!("{context}: {error}"),
            acknowledgement_unknown: true,
            timed_out,
            busy: false,
        }
    }
}

#[cfg(unix)]
fn connect_unix_socket(
    path: &Path,
    deadline: Instant,
) -> io::Result<std::os::unix::net::UnixStream> {
    use std::os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
        unix::{ffi::OsStrExt, net::UnixStream},
    };

    let path_bytes = path.as_os_str().as_bytes();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let path_capacity = std::mem::size_of_val(&address.sun_path);
    if path_bytes.is_empty() || path_bytes.contains(&0) || path_bytes.len() + 1 > path_capacity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "The Unix socket path is invalid or too long.",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    unsafe {
        std::ptr::copy_nonoverlapping(
            path_bytes.as_ptr(),
            address.sun_path.as_mut_ptr().cast::<u8>(),
            path_bytes.len(),
        );
    }
    let address_length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + path_bytes.len() + 1;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = address_length as u8;
    }
    let raw_fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let fd = socket.as_raw_fd();
    let status_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if status_flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFL, status_flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor_flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if descriptor_flags < 0
        || unsafe { libc::fcntl(fd, libc::F_SETFD, descriptor_flags | libc::FD_CLOEXEC) } < 0
    {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe {
        libc::connect(
            fd,
            &address as *const _ as *const libc::sockaddr,
            address_length as libc::socklen_t,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        let pending = matches!(
            error.raw_os_error(),
            Some(libc::EINPROGRESS) | Some(libc::EALREADY) | Some(libc::EINTR)
        );
        if !pending {
            return Err(error);
        }
        wait_for_poll(fd, libc::POLLOUT, deadline)?;
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The Unix socket connection deadline expired.",
            ));
        }
        let mut socket_error: libc::c_int = 0;
        let mut socket_error_length = std::mem::size_of_val(&socket_error) as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                &mut socket_error as *mut _ as *mut libc::c_void,
                &mut socket_error_length,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if socket_error != 0 {
            return Err(io::Error::from_raw_os_error(socket_error));
        }
    }
    verify_socket_peer(fd)?;
    if Instant::now() >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "The Unix socket connection deadline expired.",
        ));
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, status_flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let raw_fd = socket.into_raw_fd();
    Ok(unsafe { UnixStream::from_raw_fd(raw_fd) })
}

#[cfg(unix)]
fn verify_socket_peer(fd: std::os::fd::RawFd) -> io::Result<()> {
    let mut peer: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut peer_length = std::mem::size_of_val(&peer) as libc::socklen_t;
    let result = unsafe {
        libc::getpeername(
            fd,
            &mut peer as *mut _ as *mut libc::sockaddr,
            &mut peer_length,
        )
    };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOTCONN) {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "The local control socket is not connected yet.",
        ));
    }
    Err(error)
}

#[cfg(unix)]
fn wait_for_poll(
    fd: std::os::fd::RawFd,
    events: libc::c_short,
    deadline: Instant,
) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The Unix socket connection deadline expired.",
            ));
        }
        let timeout_ms = remaining
            .as_nanos()
            .saturating_add(999_999)
            .checked_div(1_000_000)
            .unwrap_or(1)
            .clamp(1, i32::MAX as u128) as libc::c_int;
        let mut descriptor = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result > 0 {
            return Ok(());
        }
        if result == 0 {
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn send_request_with_deadline(
    request: &ControlRequest,
    deadline: Instant,
) -> Result<ControlResponse, ControlTransportError> {
    let path = control_socket_path().map_err(ControlTransportError::before_send)?;
    send_request_at(&path, request, deadline)
}

#[cfg(unix)]
fn send_request_at(
    path: &Path,
    request: &ControlRequest,
    deadline: Instant,
) -> Result<ControlResponse, ControlTransportError> {
    validate_client_socket_path(path).map_err(ControlTransportError::before_send)?;
    let mut stream = connect_unix_socket(path, deadline).map_err(|error| {
        if error.kind() == io::ErrorKind::TimedOut {
            ControlTransportError::before_deadline(format!(
                "The connection deadline expired before the request was sent: {error}"
            ))
        } else if error.kind() == io::ErrorKind::WouldBlock {
            ControlTransportError::before_busy(
                "The control socket is temporarily busy; retry shortly.",
            )
        } else {
            ControlTransportError::before_send(format!(
                "No active Cutting Board control socket is available: {error}"
            ))
        }
    })?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ControlTransportError::before_deadline(
            "The control request deadline expired before it could be sent.",
        ));
    }
    stream.set_read_timeout(Some(remaining)).map_err(|error| {
        ControlTransportError::before_send(format!(
            "Could not set the control response timeout: {error}"
        ))
    })?;
    stream.set_write_timeout(Some(remaining)).map_err(|error| {
        ControlTransportError::before_send(format!(
            "Could not set the control request timeout: {error}"
        ))
    })?;
    let mut bytes = serde_json::to_vec(request).map_err(|error| {
        ControlTransportError::before_send(format!("Could not encode the control request: {error}"))
    })?;
    if bytes.len() + 1 > MAX_REQUEST_BYTES {
        return Err(ControlTransportError::before_send(
            "The control request exceeds the 65536-byte limit.",
        ));
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|error| {
        ControlTransportError::after_send_io("Could not send the control request", error)
    })?;
    stream.flush().map_err(|error| {
        ControlTransportError::after_send_io("Could not flush the control request", error)
    })?;

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ControlTransportError::after_deadline(
            "The request was sent, but the response deadline expired.",
        ));
    }
    stream.set_read_timeout(Some(remaining)).map_err(|error| {
        ControlTransportError::after_send(format!(
            "Could not set the control response timeout: {error}"
        ))
    })?;
    let mut reader = BufReader::new(stream);
    let mut response_bytes = Vec::new();
    let read = (&mut reader)
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_until(b'\n', &mut response_bytes)
        .map_err(|error| {
            ControlTransportError::after_send_io("Could not read the control response", error)
        })?;
    if read == 0 || response_bytes.last() != Some(&b'\n') {
        return Err(ControlTransportError::after_send(
            "The control server returned an incomplete response.",
        ));
    }
    if response_bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ControlTransportError::after_send(
            "The control response exceeds the 1048576-byte limit.",
        ));
    }
    response_bytes.pop();
    serde_json::from_slice(&response_bytes)
        .map(explain_unsupported_request)
        .map_err(|error| {
            ControlTransportError::after_send(format!(
                "The control server returned invalid JSON: {error}"
            ))
        })
}

#[cfg(unix)]
fn validate_client_socket_path(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let parent = path
        .parent()
        .ok_or_else(|| "The control socket path has no parent directory.".to_string())?;
    let parent_metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| format!("No active Cutting Board control socket is available: {error}"))?;
    if !parent_metadata.is_dir()
        || parent_metadata.file_type().is_symlink()
        || parent_metadata.uid() != effective_uid()
        || parent_metadata.mode() & 0o077 != 0
    {
        return Err("The control directory has unsafe ownership or permissions.".into());
    }
    ensure_socket_path_fits(path)?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("No active Cutting Board control socket is available: {error}"))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid()
        || metadata.mode() & 0o077 != 0
    {
        return Err("The control socket has unsafe ownership, type, or permissions.".into());
    }
    Ok(())
}

fn response_exit_code(response: &ControlResponse) -> i32 {
    if response.ok {
        return 0;
    }
    let code = response
        .error
        .as_ref()
        .map(|error| error.code.as_str())
        .unwrap_or("action_failed");
    match code {
        "invalid_request" | "readiness_unavailable" | "run_boundary_unknown" => EXIT_INVALID,
        "busy" => EXIT_BUSY,
        "unknown_profile" | "unknown_task" | "unknown_operation" | "not_found" => EXIT_NOT_FOUND,
        "timeout" => EXIT_TIMEOUT,
        "unavailable" | "socket_unavailable" => EXIT_UNAVAILABLE,
        _ => EXIT_ACTION_FAILED,
    }
}

pub(crate) fn print_response(response: &ControlResponse) {
    let mut output = io::stdout().lock();
    if serde_json::to_writer(&mut output, response).is_ok() {
        let _ = output.write_all(b"\n");
    }
}

const EXIT_INVALID: i32 = 2;
const EXIT_BUSY: i32 = 3;
const EXIT_NOT_FOUND: i32 = 4;
const EXIT_TIMEOUT: i32 = 5;
const EXIT_ACTION_FAILED: i32 = 6;
const EXIT_UNAVAILABLE: i32 = 7;

/// The CLI's `wait` and action paths against a socket of the test's own.
#[cfg(all(test, unix))]
pub(crate) mod test_client {
    use super::*;

    pub(crate) fn send(path: &Path, request: &ControlRequest) -> (ControlResponse, i32) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let response = send_request_at(path, request, deadline).map_err(|error| error.message);
        let response = response.unwrap();
        let code = response_exit_code(&response);
        (response, code)
    }

    pub(crate) fn wait(
        path: &Path,
        request: &ControlRequest,
        timeout: Duration,
    ) -> (ControlResponse, i32) {
        let response = wait_for_readiness(request, timeout, |request, deadline| {
            send_request_at(path, request, deadline)
        });
        let code = response_exit_code(&response);
        (response, code)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{
        connect_unix_socket, explain_unsupported_request, response_exit_code, send_request_at,
        start_server_at, wait_for_poll, wait_for_readiness, ControlError, ControlRequest,
        ControlResponse, ControlTransportError, EXIT_ACTION_FAILED, EXIT_INVALID, EXIT_TIMEOUT,
        EXIT_UNAVAILABLE, MAX_REQUEST_BYTES,
    };
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixStream,
        path::PathBuf,
        thread,
        time::Duration,
    };

    fn response(result: serde_json::Value) -> ControlResponse {
        ControlResponse {
            ok: true,
            accepted: true,
            completed: true,
            operation_id: None,
            result: Some(result),
            error: None,
        }
    }

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let control_dir = temporary.path().join("control");
        std::fs::create_dir(&control_dir).unwrap();
        (temporary, control_dir.join("control.sock"))
    }

    #[test]
    fn dummy_socket_round_trips_typed_request_and_json_result() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path.clone(), |request| match request {
            ControlRequest::Status {
                profile_id,
                task_name,
            } => response(json!({
                "profile_id": profile_id,
                "task_name": task_name,
                "state": "running"
            })),
            _ => ControlResponse::error("invalid_request", "unexpected request"),
        })
        .unwrap();
        let client = UnixStream::connect(&path).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut client = client;
        client
            .write_all(b"{\"op\":\"status\",\"profile_id\":\"p\",\"task_name\":\"api\"}\n")
            .unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["result"]["state"], "running");
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn duplicate_server_does_not_unlink_the_active_socket() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path.clone(), |_| response(json!({"alive": true}))).unwrap();
        assert!(start_server_at(path.clone(), |_| response(json!({}))).is_err());
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_unknown_fields_and_oversized_requests() {
        let (_temporary, path) = socket_path();
        let _server =
            start_server_at(path.clone(), |_| response(json!({"unexpected": false}))).unwrap();
        for request in [
            b"{\"op\":\"list\",\"command\":\"rm -rf\"}\n".to_vec(),
            vec![b'a'; MAX_REQUEST_BYTES + 2],
        ] {
            let mut client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            client.write_all(&request).unwrap();
            if request.last() != Some(&b'\n') {
                client.write_all(b"\n").unwrap();
            }
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(parsed["ok"], false);
            assert_eq!(parsed["error"]["code"], "invalid_request");
        }
    }

    #[test]
    fn server_rejects_unsafe_existing_socket_paths() {
        let (_temporary, path) = socket_path();
        std::fs::write(&path, "not a socket").unwrap();
        assert!(start_server_at(path.clone(), |_| response(json!({}))).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "not a socket");
    }

    #[test]
    fn server_replaces_only_an_owned_stale_socket() {
        use std::os::unix::net::UnixListener;

        let (_temporary, path) = socket_path();
        let stale = UnixListener::bind(&path).unwrap();
        drop(stale);
        let server = start_server_at(path.clone(), |_| response(json!({"alive": true}))).unwrap();
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn server_refuses_a_symlink_at_the_socket_path() {
        use std::os::unix::fs::symlink;

        let (_temporary, path) = socket_path();
        let target = path.parent().unwrap().join("target");
        std::fs::write(&target, "keep").unwrap();
        symlink(&target, &path).unwrap();
        assert!(start_server_at(path.clone(), |_| response(json!({}))).is_err());
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
    }

    #[test]
    fn nonblocking_connect_to_an_unavailable_socket_fails_without_waiting_for_deadline() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("missing.sock");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let started = std::time::Instant::now();
        let error = match connect_unix_socket(&path, deadline) {
            Ok(_) => panic!("connection unexpectedly succeeded"),
            Err(error) => error,
        };
        assert_ne!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nonblocking_connect_obeys_deadline_when_listener_backlog_is_full() {
        use std::os::{
            fd::AsRawFd,
            unix::net::{UnixListener, UnixStream},
        };

        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("backlog.sock");
        let listener = UnixListener::bind(&path).unwrap();
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 1) }, 0);
        let mut clients: Vec<UnixStream> = Vec::new();
        let mut bounded_failure = false;
        for _ in 0..8 {
            let deadline = std::time::Instant::now() + Duration::from_millis(40);
            let started = std::time::Instant::now();
            match connect_unix_socket(&path, deadline) {
                Ok(client) => clients.push(client),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                    bounded_failure = true;
                    assert!(started.elapsed() < Duration::from_secs(1));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    bounded_failure = true;
                    assert!(started.elapsed() < Duration::from_secs(1));
                    break;
                }
                Err(error) => panic!("unexpected local socket connection error: {error}"),
            }
        }
        assert!(
            bounded_failure,
            "the unaccepted listener backlog did not fill"
        );
        drop(clients);
        drop(listener);
    }

    #[test]
    fn poll_wait_obeys_a_short_deadline_for_a_nonwritable_descriptor() {
        let mut descriptors = [0; 2];
        assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
        let read_fd = descriptors[0];
        let write_fd = descriptors[1];
        let original_flags = unsafe { libc::fcntl(write_fd, libc::F_GETFL) };
        assert!(original_flags >= 0);
        assert_eq!(
            unsafe { libc::fcntl(write_fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) },
            0
        );

        let bytes = [0u8; 4096];
        loop {
            let written = unsafe { libc::write(write_fd, bytes.as_ptr().cast(), bytes.len()) };
            if written > 0 {
                continue;
            }
            assert_ne!(written, 0, "pipe write unexpectedly returned zero");
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            break;
        }

        let started = std::time::Instant::now();
        let result = wait_for_poll(write_fd, libc::POLLOUT, started + Duration::from_millis(80));
        let elapsed = started.elapsed();
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn listener_drop_waits_for_accept_loop_to_finish() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path, |_| response(json!({}))).unwrap();
        let started = std::time::Instant::now();
        drop(server);
        assert!(started.elapsed() < Duration::from_secs(1));
        thread::yield_now();
    }

    fn readiness_request() -> ControlRequest {
        ControlRequest::Readiness {
            profile_id: "p".into(),
            task_name: "api".into(),
            log_pattern: None,
        }
    }

    fn waiting() -> ControlResponse {
        response(json!({ "ready": false, "waiting_for": "port" }))
    }

    fn failed(code: &str) -> ControlResponse {
        ControlResponse::error(code, "message")
    }

    /// Answers from `script` and repeats its last entry.
    fn scripted_wait(
        script: Vec<Result<ControlResponse, ControlTransportError>>,
        timeout: Duration,
    ) -> (ControlResponse, usize) {
        let mut script = std::collections::VecDeque::from(script);
        let mut calls = 0;
        let response = wait_for_readiness(&readiness_request(), timeout, |_, _| {
            calls += 1;
            if script.len() > 1 {
                return script.pop_front().unwrap();
            }
            match script.front().unwrap() {
                Ok(response) => {
                    Ok(serde_json::from_value(serde_json::to_value(response).unwrap()).unwrap())
                }
                Err(error) => Err(ControlTransportError {
                    message: error.message.clone(),
                    ..*error
                }),
            }
        });
        (response, calls)
    }

    #[test]
    fn wait_polls_through_waiting_busy_and_transport_errors_until_ready() {
        let ready = response(json!({ "ready": true, "signal": "port" }));
        let (result, calls) = scripted_wait(
            vec![
                Err(ControlTransportError::before_send("no socket")),
                Ok(failed("busy")),
                Ok(waiting()),
                Ok(ready),
            ],
            Duration::from_secs(10),
        );
        assert!(result.ok);
        assert_eq!(result.result.unwrap()["signal"], "port");
        assert_eq!(calls, 4);
    }

    #[test]
    fn wait_timeout_carries_the_last_judgement_and_cancels_nothing() {
        let (result, _) = scripted_wait(vec![Ok(waiting())], Duration::from_millis(1100));
        assert!(!result.ok);
        assert!(result.accepted);
        assert!(!result.completed);
        assert_eq!(response_exit_code(&result), EXIT_TIMEOUT);
        assert_eq!(result.result.as_ref().unwrap()["waiting_for"], "port");
        assert!(result
            .error
            .unwrap()
            .message
            .contains("Nothing was cancelled"));
    }

    #[test]
    fn wait_reports_an_app_it_never_reached_as_unavailable() {
        let (result, calls) = scripted_wait(
            vec![Err(ControlTransportError::before_send("no socket"))],
            Duration::from_millis(1100),
        );
        assert_eq!(response_exit_code(&result), EXIT_UNAVAILABLE);
        assert!(calls >= 2, "transport errors must be retried");
        assert_eq!(result.error.unwrap().message, "no socket");
    }

    #[test]
    fn wait_reports_an_app_that_never_answered_in_time_as_a_timeout() {
        for error in [
            ControlTransportError::after_send("incomplete response"),
            ControlTransportError::after_deadline("response deadline expired"),
            ControlTransportError::after_send_io(
                "Could not read the control response",
                std::io::ErrorKind::TimedOut.into(),
            ),
        ] {
            let (result, _) = scripted_wait(vec![Err(error)], Duration::from_millis(10));
            assert_eq!(response_exit_code(&result), EXIT_TIMEOUT);
            assert_eq!(result.error.unwrap().code, "timeout");
        }
    }

    #[test]
    fn wait_reports_a_socket_it_could_not_send_to_as_unavailable() {
        for error in [
            ControlTransportError::before_send("no socket"),
            ControlTransportError::before_deadline("connection deadline expired"),
            ControlTransportError::before_busy("temporarily busy"),
        ] {
            let (result, _) = scripted_wait(vec![Err(error)], Duration::from_millis(10));
            assert_eq!(response_exit_code(&result), EXIT_UNAVAILABLE);
            assert_eq!(result.error.unwrap().code, "unavailable");
        }
    }

    #[test]
    fn wait_reports_an_app_that_quit_as_unavailable_without_waiting_out_the_timeout() {
        let started = std::time::Instant::now();
        let (result, _) = scripted_wait(
            vec![
                Ok(waiting()),
                Err(ControlTransportError::before_send("no socket")),
            ],
            Duration::from_secs(60),
        );
        assert_eq!(response_exit_code(&result), EXIT_UNAVAILABLE);
        assert_eq!(result.error.unwrap().message, "no socket");
        assert!(started.elapsed() < Duration::from_secs(15));
    }

    #[test]
    fn wait_ending_inside_the_grace_period_still_reports_an_app_that_quit() {
        let (result, _) = scripted_wait(
            vec![
                Ok(waiting()),
                Err(ControlTransportError::before_send("no socket")),
            ],
            Duration::from_millis(1100),
        );
        assert_eq!(response_exit_code(&result), EXIT_UNAVAILABLE);
    }

    #[test]
    fn wait_survives_a_quick_app_restart() {
        let (result, calls) = scripted_wait(
            vec![
                Ok(waiting()),
                Err(ControlTransportError::before_send("no socket")),
                Err(ControlTransportError::before_send("no socket")),
                Ok(response(json!({ "ready": true, "signal": "port" }))),
            ],
            Duration::from_secs(60),
        );
        assert!(result.ok);
        assert_eq!(calls, 4);
    }

    #[test]
    fn wait_does_not_mistake_a_slow_or_busy_app_for_one_that_quit() {
        for error in [
            ControlTransportError::before_deadline("connection deadline expired"),
            ControlTransportError::before_busy("temporarily busy"),
            ControlTransportError::after_deadline("response deadline expired"),
            ControlTransportError::after_send("incomplete response"),
        ] {
            let (result, _) = scripted_wait(
                vec![
                    Err(ControlTransportError::before_send("no socket")),
                    Ok(waiting()),
                    Err(error),
                ],
                Duration::from_millis(2100),
            );
            assert_eq!(response_exit_code(&result), EXIT_TIMEOUT);
        }
    }

    #[test]
    fn wait_notices_a_server_that_goes_away_over_a_real_socket() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path.clone(), |_| {
            response(json!({ "ready": false, "waiting_for": "port" }))
        })
        .unwrap();
        let stop = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            drop(server);
        });
        let result = wait_for_readiness(
            &readiness_request(),
            Duration::from_millis(1500),
            |request, deadline| send_request_at(&path, request, deadline),
        );
        stop.join().unwrap();
        assert_eq!(response_exit_code(&result), EXIT_UNAVAILABLE);
    }

    #[test]
    fn wait_tells_a_slow_app_from_a_missing_socket_over_a_real_socket() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path.clone(), |_| {
            thread::sleep(Duration::from_secs(2));
            response(json!({ "ready": true }))
        })
        .unwrap();
        let wait = || {
            wait_for_readiness(
                &readiness_request(),
                Duration::from_millis(200),
                |request, deadline| send_request_at(&path, request, deadline),
            )
        };

        let slow = wait();
        assert_eq!(response_exit_code(&slow), EXIT_TIMEOUT);
        assert!(slow.result.is_none());

        drop(server);
        let missing = wait();
        assert_eq!(response_exit_code(&missing), EXIT_UNAVAILABLE);
    }

    #[test]
    fn wait_stops_at_the_first_real_failure() {
        for (code, exit) in [
            ("prepare_failed", EXIT_ACTION_FAILED),
            ("readiness_unavailable", EXIT_INVALID),
            ("run_boundary_unknown", EXIT_INVALID),
        ] {
            let (result, calls) = scripted_wait(vec![Ok(failed(code))], Duration::from_secs(10));
            assert_eq!(calls, 1);
            assert_eq!(response_exit_code(&result), exit);
        }
    }

    #[test]
    fn validation_covers_the_new_requests() {
        let update = |command: Option<&str>, port: Option<u16>, clear_port: bool| {
            ControlRequest::Update {
                profile_id: "p".into(),
                task_name: "api".into(),
                command: command.map(str::to_owned),
                cwd: None,
                expected_port: port,
                clear_expected_port: clear_port,
                clear_prepare: false,
            }
            .validate()
        };
        assert!(update(Some("run"), None, false).is_ok());
        assert!(update(None, Some(80), false).is_ok());
        assert!(update(None, None, true).is_ok());
        assert!(update(None, None, false).is_err());
        assert!(update(Some(" "), None, false).is_err());
        assert!(update(Some("a\nb"), None, false).is_err());
        assert!(update(None, Some(0), false).is_err());
        assert!(update(None, Some(80), true).is_err());

        let wait = |pattern: &str| {
            ControlRequest::Readiness {
                profile_id: "p".into(),
                task_name: "api".into(),
                log_pattern: Some(pattern.into()),
            }
            .validate()
        };
        assert!(wait("Started App").is_ok());
        assert!(wait("  ").is_err());
        assert!(wait(&"x".repeat(1025)).is_err());
    }

    #[test]
    fn plain_logs_request_stays_readable_by_an_older_app() {
        let plain = ControlRequest::Logs {
            profile_id: "p".into(),
            task_name: "api".into(),
            lines: None,
            bytes: None,
            current_run: false,
            prepare: false,
        };
        let plain = serde_json::to_value(&plain).unwrap();
        assert!(plain.get("current_run").is_none());
        assert!(plain.get("prepare").is_none());
    }

    #[test]
    fn a_request_an_older_app_rejects_gets_an_understandable_message() {
        let (_temporary, path) = socket_path();
        let server = start_server_at(path.clone(), |_| response(json!({}))).unwrap();
        // What an app without this request answers, spoken to like the CLI does.
        for request in [
            r#"{"op":"from_the_future","profile_id":"p"}"#,
            r#"{"op":"status","profile_id":"p","task_name":"api","new_field":1}"#,
        ] {
            let client = UnixStream::connect(&path).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut client = client;
            writeln!(client, "{request}").unwrap();
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            let raw: ControlResponse = serde_json::from_str(line.trim()).unwrap();
            let explained = explain_unsupported_request(raw);
            assert_eq!(response_exit_code(&explained), EXIT_INVALID);
            assert!(explained
                .error
                .unwrap()
                .message
                .contains("does not support this request"));
        }
        drop(server);

        let other = explain_unsupported_request(ControlResponse {
            error: Some(ControlError {
                code: "invalid_request".into(),
                message: "Log limits are off.".into(),
            }),
            ..failed("invalid_request")
        });
        assert_eq!(other.error.unwrap().message, "Log limits are off.");
    }
}
