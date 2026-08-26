use std::{
    env,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::{Command, Output},
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
}
