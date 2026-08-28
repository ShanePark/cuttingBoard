use crate::models::{ServiceIdentity, ServiceSnapshot, TerminationResult};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::os::unix::{
    ffi::{OsStrExt, OsStringExt},
    process::CommandExt,
};
#[cfg(windows)]
use std::os::windows::{
    ffi::{OsStrExt, OsStringExt},
    process::CommandExt,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Debug, Clone)]
struct ProcessLaunchSpec {
    executable: PathBuf,
    argv: Vec<OsString>,
    cwd: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

pub(crate) fn service_identity(service: &ServiceSnapshot) -> Result<ServiceIdentity, String> {
    let process = service.process.as_ref().ok_or_else(|| {
        "The service has no current process identity. Refresh and try again.".to_string()
    })?;
    Ok(ServiceIdentity {
        pid: process.pid,
        start_time: process.create_time,
        uid: process.uid,
        display_name: service.display_name.clone(),
    })
}

pub(crate) fn validate_service_log_identity(identity: &ServiceIdentity) -> Result<(), String> {
    validate_pid_and_uid(
        identity,
        "Cutting Board refused to inspect that process.",
        "Cutting Board only reads logs from processes owned by the current user.",
    )?;
    ensure_process_identity(
        identity,
        "The process already exited. Refresh and try again.",
        "The PID was reused by another process. Refresh before reading its logs.",
    )
}

pub(crate) fn terminate_discovered_service(
    identity: ServiceIdentity,
) -> Result<TerminationResult, String> {
    validate_termination_identity(&identity)?;

    send_signal(identity.pid, libc::SIGTERM)?;
    for _ in 0..25 {
        thread::sleep(Duration::from_millis(80));
        if !process_is_current(&identity)? {
            return Ok(TerminationResult {
                success: true,
                message: format!("Stopped {}.", identity.display_name),
            });
        }
    }

    // Revalidate immediately before escalating. A reused PID must never receive
    // a signal belonging to the previous service.
    validate_termination_identity(&identity)?;
    send_signal(identity.pid, libc::SIGKILL)?;
    for _ in 0..10 {
        thread::sleep(Duration::from_millis(60));
        if !process_is_current(&identity)? {
            return Ok(TerminationResult {
                success: true,
                message: format!(
                    "Forced {} to stop after it ignored SIGTERM.",
                    identity.display_name
                ),
            });
        }
    }
    Ok(TerminationResult {
        success: false,
        message: format!("{} did not stop.", identity.display_name),
    })
}

pub(crate) fn restart_discovered_service(
    service: &ServiceSnapshot,
    identity: ServiceIdentity,
) -> Result<(), String> {
    let scanned_identity = service_identity(service)?;
    if scanned_identity.pid != identity.pid
        || scanned_identity.start_time != identity.start_time
        || scanned_identity.uid != identity.uid
    {
        return Err(
            "The service identity changed since the last scan. Refresh and try again.".into(),
        );
    }
    validate_termination_identity(&identity)?;
    let launch = capture_live_process(&identity)?;

    let result = terminate_discovered_service(identity)?;
    if !result.success {
        return Err(result.message);
    }

    spawn_process(&launch)
}

fn capture_live_process(identity: &ServiceIdentity) -> Result<ProcessLaunchSpec, String> {
    let pid = Pid::from_u32(identity.pid);
    let refresh_kind = ProcessRefreshKind::nothing()
        .without_tasks()
        .with_cmd(UpdateKind::Always)
        .with_exe(UpdateKind::Always)
        .with_cwd(UpdateKind::Always)
        .with_environ(UpdateKind::Always);
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, refresh_kind);
    let process = system
        .process(pid)
        .ok_or_else(|| "The service process already exited. Refresh and try again.".to_string())?;
    if !process_start_time_matches(process.start_time(), identity.start_time) {
        return Err("The PID was reused by another process. Refresh before restarting it.".into());
    }

    let executable = process.exe().map(Path::to_path_buf).ok_or_else(|| {
        "Cutting Board could not safely determine the service executable. Refresh and try again."
            .to_string()
    })?;
    let argv = process.cmd().to_vec();
    let cwd = process.cwd().map(Path::to_path_buf).ok_or_else(|| {
        "Cutting Board could not safely determine the service directory. Refresh and try again."
            .to_string()
    })?;
    let environment = capture_environment(process.environ())?;

    validate_launch_metadata(&executable, &argv, &cwd)?;
    Ok(ProcessLaunchSpec {
        executable,
        argv,
        cwd,
        environment,
    })
}

fn capture_environment(values: &[OsString]) -> Result<Vec<(OsString, OsString)>, String> {
    if values.is_empty() {
        return Err(
            "Cutting Board could not safely read the service environment. Refresh and try again."
                .into(),
        );
    }

    values
        .iter()
        .map(|value| {
            let (key, value) = split_environment_entry(value).ok_or_else(|| {
                "Cutting Board could not safely read the service environment. Refresh and try again."
                    .to_string()
            })?;
            if key.is_empty() {
                return Err(
                    "Cutting Board could not safely read the service environment. Refresh and try again."
                        .into(),
                );
            }
            Ok((key, value))
        })
        .collect()
}

fn split_environment_entry(value: &OsStr) -> Option<(OsString, OsString)> {
    #[cfg(unix)]
    {
        let bytes = value.as_bytes();
        let separator = bytes.iter().position(|byte| *byte == b'=')?;
        return Some((
            OsString::from_vec(bytes[..separator].to_vec()),
            OsString::from_vec(bytes[separator + 1..].to_vec()),
        ));
    }

    #[cfg(windows)]
    {
        let wide = value.encode_wide().collect::<Vec<_>>();
        let separator = wide
            .iter()
            .position(|character| *character == u16::from(b'='))?;
        return Some((
            OsString::from_wide(&wide[..separator]),
            OsString::from_wide(&wide[separator + 1..]),
        ));
    }

    #[cfg(not(any(unix, windows)))]
    {
        let (key, value) = value.to_str()?.split_once('=')?;
        Some((OsString::from(key), OsString::from(value)))
    }
}

fn validate_launch_metadata(
    executable: &Path,
    argv: &[OsString],
    cwd: &Path,
) -> Result<(), String> {
    if executable.as_os_str().is_empty() || !executable.is_absolute() || !executable.is_file() {
        return Err(
            "Cutting Board could not safely determine the service executable. Refresh and try again."
                .into(),
        );
    }
    command_arguments(argv)?;
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(
            "Cutting Board could not safely determine the service directory. Refresh and try again."
                .into(),
        );
    }
    Ok(())
}

fn command_arguments(argv: &[OsString]) -> Result<&[OsString], String> {
    if argv.first().map_or(true, |value| value.is_empty()) {
        return Err(
            "Cutting Board could not safely determine the service arguments. Refresh and try again."
                .into(),
        );
    }
    Ok(&argv[1..])
}

fn spawn_process(launch: &ProcessLaunchSpec) -> Result<(), String> {
    let args = command_arguments(&launch.argv)?;
    let mut command = Command::new(&launch.executable);
    command
        .args(args)
        .current_dir(&launch.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env_clear();
    command.envs(launch.environment.iter().map(|(key, value)| (key, value)));

    #[cfg(unix)]
    unsafe {
        // Keep the restarted service independent of the Cutting Board process
        // and preserve a non-standard argv[0] when the platform supports it.
        command.arg0(&launch.argv[0]);
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }

    #[cfg(windows)]
    {
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
    }

    let child = command.spawn().map_err(|error| {
        format!(
            "Could not restart the service from {}: {error}",
            launch.executable.display()
        )
    })?;
    thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    Ok(())
}

fn validate_termination_identity(identity: &ServiceIdentity) -> Result<(), String> {
    validate_pid_and_uid(
        identity,
        "Cutting Board refused to stop that process.",
        "Cutting Board only stops processes owned by the current user.",
    )?;
    ensure_process_identity(
        identity,
        "The process already exited.",
        "The PID was reused by another process. Refresh before stopping it.",
    )
}

fn validate_pid_and_uid(
    identity: &ServiceIdentity,
    invalid_pid_message: &str,
    wrong_uid_message: &str,
) -> Result<(), String> {
    if identity.pid <= 1 || identity.pid == std::process::id() {
        return Err(invalid_pid_message.into());
    }
    let current_uid = effective_uid();
    if identity.uid.is_some() && current_uid.is_some() && identity.uid != current_uid {
        return Err(wrong_uid_message.into());
    }
    Ok(())
}

fn ensure_process_identity(
    identity: &ServiceIdentity,
    missing_message: &str,
    reused_message: &str,
) -> Result<(), String> {
    let start_time =
        current_process_start_time(identity.pid).ok_or_else(|| missing_message.to_string())?;
    if !process_start_time_matches(start_time, identity.start_time) {
        return Err(reused_message.into());
    }
    Ok(())
}

fn process_is_current(identity: &ServiceIdentity) -> Result<bool, String> {
    let Some(start_time) = current_process_start_time(identity.pid) else {
        return Ok(false);
    };
    if !process_start_time_matches(start_time, identity.start_time) {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }
    Ok(true)
}

fn current_process_start_time(pid: u32) -> Option<u64> {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );
    system.process(pid).map(|process| process.start_time())
}

fn process_start_time_matches(actual: u64, expected: u64) -> bool {
    actual == expected
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: i32) -> Result<(), String> {
    let result = unsafe { libc::kill(pid as i32, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(format!("Could not signal PID {pid}: {error}"))
    }
}

#[cfg(not(unix))]
fn send_signal(pid: u32, _signal: i32) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::{
        capture_environment, command_arguments, process_start_time_matches,
        split_environment_entry, validate_launch_metadata,
    };
    use std::{
        ffi::{OsStr, OsString},
        fs,
        path::Path,
    };

    #[test]
    fn process_start_time_matching_rejects_a_reused_pid() {
        assert!(process_start_time_matches(42, 42));
        assert!(!process_start_time_matches(43, 42));
    }

    #[test]
    fn command_arguments_preserve_argument_boundaries_without_argv0() {
        let argv = vec![
            OsString::from("node"),
            OsString::from("server.js"),
            OsString::from("--label"),
            OsString::from("hello world"),
        ];

        assert_eq!(command_arguments(&argv).unwrap(), &argv[1..]);
        assert!(command_arguments(&[]).is_err());
    }

    #[test]
    fn environment_capture_preserves_values_containing_equals() {
        let entry = OsString::from("TOKEN=part=two");
        let (key, value) = split_environment_entry(OsStr::new(&entry)).unwrap();

        assert_eq!(key, OsString::from("TOKEN"));
        assert_eq!(value, OsString::from("part=two"));
        assert_eq!(capture_environment(&[entry]).unwrap().len(), 1);
        assert!(capture_environment(&[OsString::from("MALFORMED")]).is_err());
        assert!(capture_environment(&[]).is_err());
    }

    #[test]
    fn launch_metadata_rejects_missing_or_unusable_restart_fields() {
        let temporary = tempfile::tempdir().unwrap();
        let executable = temporary.path().join("service");
        fs::write(&executable, b"service").unwrap();
        let argv = vec![OsString::from("service")];

        assert!(validate_launch_metadata(&executable, &argv, temporary.path()).is_ok());
        assert!(validate_launch_metadata(Path::new("service"), &argv, temporary.path()).is_err());
        assert!(validate_launch_metadata(&executable, &[], temporary.path()).is_err());
        assert!(validate_launch_metadata(&executable, &argv, Path::new("/missing")).is_err());
    }
}
