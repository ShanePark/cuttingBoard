use std::{
    env,
    ffi::OsStr,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const NOT_FOUND_MESSAGE: &str = "Docker CLI was not found in PATH or standard macOS locations.";

pub(crate) fn not_found_message() -> &'static str {
    NOT_FOUND_MESSAGE
}

pub(crate) fn run<I, S>(executable: &Path, args: I) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(executable).args(args).output()
}

/// `run` with a limit: past `timeout` the child is killed and the error kind is `TimedOut`.
pub(crate) fn run_with_timeout<I, S>(
    executable: &Path,
    args: I,
    timeout: Duration,
) -> io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    let timed_out = || io::Error::from(io::ErrorKind::TimedOut);
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Read while it runs: output larger than the pipe buffer would otherwise block the child until
    // the timeout, and a long container listing can be that large.
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(timed_out());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
    };
    // Bounded too: a process the child left behind can hold the pipes open after it exits.
    let collect = |output: Receiver<Vec<u8>>| {
        output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| timed_out())
    };
    Ok(Output {
        status,
        stdout: collect(stdout)?,
        stderr: collect(stderr)?,
    })
}

fn read_in_background<R>(pipe: Option<R>) -> Receiver<Vec<u8>>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(bytes);
    });
    receiver
}

/// Starts Docker Compose with piped output so it selects a project's containers by label only.
/// Compose would otherwise load `COMPOSE_FILE` or a compose file found in the working directory or
/// its parents and act on that file's services instead.
pub(crate) fn spawn_compose_by_label<I, S>(executable: &Path, args: I) -> io::Result<Child>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(executable)
        .args(args)
        .current_dir("/")
        .env_remove("COMPOSE_FILE")
        .env_remove("COMPOSE_PROJECT_NAME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

pub(crate) fn resolve_executable() -> Option<PathBuf> {
    let path = env::var_os("PATH");
    let home = dirs::home_dir();
    docker_executable_candidates(path.as_deref(), home.as_deref())
        .into_iter()
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn error_detail(output: &Output) -> String {
    if let Some(detail) = stderr_detail(output) {
        return detail;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    output.status.code().map_or_else(
        || "Docker exited without a status code.".into(),
        |code| format!("Docker exited with status code {code}."),
    )
}

pub(crate) fn stderr_detail(output: &Output) -> Option<String> {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
    (!detail.is_empty()).then_some(detail)
}

fn docker_executable_candidates(path: Option<&OsStr>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = path
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|entry| !entry.as_os_str().is_empty())
        .map(|entry| entry.join("docker"))
        .collect::<Vec<_>>();

    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/docker"),
        PathBuf::from("/usr/local/bin/docker"),
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
    ]);
    if let Some(home) = home {
        candidates.push(home.join("Applications/Docker.app/Contents/Resources/bin/docker"));
    }
    candidates
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_path_candidates_prefer_gui_process_path_before_fallbacks() {
        let path = env::join_paths([
            PathBuf::from("/tmp/custom-bin"),
            PathBuf::from("/tmp/another-bin"),
        ])
        .unwrap();
        let candidates =
            docker_executable_candidates(Some(path.as_os_str()), Some(Path::new("/Users/example")));

        assert_eq!(candidates[0], PathBuf::from("/tmp/custom-bin/docker"));
        assert_eq!(candidates[1], PathBuf::from("/tmp/another-bin/docker"));
        assert_eq!(candidates[2], PathBuf::from("/opt/homebrew/bin/docker"));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_kills_a_command_that_does_not_finish() {
        let temporary = tempfile::tempdir().unwrap();
        let pid_path = temporary.path().join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pid_path.display());

        let error = run_with_timeout(
            Path::new("sh"),
            ["-c", script.as_str()],
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let pid: i32 = std::fs::read_to_string(&pid_path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0, "the child must be gone");
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_reads_more_output_than_a_pipe_buffer_holds() {
        let output = run_with_timeout(
            Path::new("sh"),
            ["-c", "head -c 200000 /dev/zero; echo failed >&2; exit 3"],
            Duration::from_secs(30),
        )
        .unwrap();

        assert_eq!(output.stdout.len(), 200_000);
        assert_eq!(output.stderr, b"failed\n");
        assert_eq!(output.status.code(), Some(3));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_does_not_wait_for_a_process_left_holding_the_pipes() {
        let started = Instant::now();
        let error = run_with_timeout(
            Path::new("sh"),
            ["-c", "sleep 5 & exit 0"],
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(4));
    }
}
