use crate::models::{ServiceIdentity, ServiceSnapshot, TerminationResult};
use std::{thread, time::Duration};
use sysinfo::{Pid, System};

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
    let system = System::new_all();
    let process = system
        .process(Pid::from_u32(identity.pid))
        .ok_or_else(|| missing_message.to_string())?;
    if !process_start_time_matches(process.start_time(), identity.start_time) {
        return Err(reused_message.into());
    }
    Ok(())
}

fn process_is_current(identity: &ServiceIdentity) -> Result<bool, String> {
    let system = System::new_all();
    let Some(process) = system.process(Pid::from_u32(identity.pid)) else {
        return Ok(false);
    };
    if !process_start_time_matches(process.start_time(), identity.start_time) {
        return Err("The PID was reused by another process. Refresh before stopping it.".into());
    }
    Ok(true)
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
    use super::process_start_time_matches;

    #[test]
    fn process_start_time_matching_rejects_a_reused_pid() {
        assert!(process_start_time_matches(42, 42));
        assert!(!process_start_time_matches(43, 42));
    }
}
