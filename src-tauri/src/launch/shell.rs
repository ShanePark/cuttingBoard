use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(unix)]
use std::{env, ffi::CStr};

pub(super) fn shell_command(value: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/D", "/S", "/C", value]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new(configured_shell());
        command.args(["-l", "-i", "-c", value]);
        command
    }
}

#[cfg(unix)]
fn configured_shell() -> PathBuf {
    let shell = env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|path| is_usable_shell(path))
        .or_else(login_shell_from_passwd);
    shell_path_or_fallback(shell)
}

#[cfg(unix)]
fn login_shell_from_passwd() -> Option<PathBuf> {
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    if passwd.is_null() || unsafe { (*passwd).pw_shell.is_null() } {
        return None;
    }
    let shell = unsafe { CStr::from_ptr((*passwd).pw_shell) }
        .to_string_lossy()
        .into_owned();
    (!shell.is_empty()).then(|| PathBuf::from(shell))
}

#[cfg(unix)]
fn shell_path_or_fallback(candidate: Option<PathBuf>) -> PathBuf {
    candidate
        .filter(|path| is_usable_shell(path))
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

#[cfg(unix)]
fn is_usable_shell(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn managed_shell_is_login_interactive() {
        let command = shell_command("echo ok");
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args, vec!["-l", "-i", "-c", "echo ok"]);
    }

    #[cfg(unix)]
    #[test]
    fn invalid_configured_shell_uses_safe_fallback() {
        assert_eq!(
            shell_path_or_fallback(Some(PathBuf::from("/definitely/missing/shell"))),
            PathBuf::from("/bin/sh")
        );
    }
}
