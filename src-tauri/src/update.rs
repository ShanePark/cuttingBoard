use serde::Serialize;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "macos")]
use tauri::Emitter;

const EMBEDDED_SOURCE_ROOT: &str = env!("CUTTING_BOARD_SOURCE_ROOT");
const EMBEDDED_BUILD_COMMIT: &str = env!("CUTTING_BOARD_BUILD_COMMIT");
const UPDATE_HELPER_ARGUMENT: &str = "--update-helper";
const UPDATE_BUILD_COMMAND: &str = "npm run tauri build -- --bundles app";
const UPDATE_PROGRESS_EVENT: &str = "update-progress";
const UPDATE_PROGRESS_TOTAL: u8 = 4;
const UPDATE_RESTART_GRACE: Duration = Duration::from_millis(180);
const HELPER_WAIT_ATTEMPTS: usize = 120;
const HELPER_WAIT_INTERVAL: Duration = Duration::from_millis(250);

static UPDATE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpdateStatus {
    pub available: bool,
    pub current_commit: String,
    pub latest_commit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UpdateProgress {
    pub stage: String,
    pub step: u8,
    pub total: u8,
    pub message: String,
}

impl UpdateProgress {
    fn validating() -> Self {
        Self::new("validating", 1, "Checking the local source for updates…")
    }

    fn building() -> Self {
        Self::new("building", 2, "Building the latest release…")
    }

    fn preparing() -> Self {
        Self::new("preparing", 3, "Preparing the updated app…")
    }

    fn restarting() -> Self {
        Self::new("restarting", 4, "Restarting Cutting Board…")
    }

    fn new(stage: &str, step: u8, message: &str) -> Self {
        Self {
            stage: stage.into(),
            step,
            total: UPDATE_PROGRESS_TOTAL,
            message: message.into(),
        }
    }
}

#[cfg(target_os = "macos")]
fn emit_update_progress(app: &tauri::AppHandle, progress: UpdateProgress) {
    if let Err(error) = app.emit(UPDATE_PROGRESS_EVENT, progress) {
        // Progress is advisory. A webview listener can disappear while the
        // app is shutting down, but that must not prevent the safe updater
        // helper from completing its work.
        eprintln!("Could not report update progress: {error}");
    }
}

#[derive(Debug)]
struct UpdatePlan {
    built_bundle: PathBuf,
    stable_bundle: PathBuf,
    old_bundle: PathBuf,
    old_pid: u32,
}

#[derive(Debug, PartialEq, Eq)]
struct HelperArguments {
    built_bundle: PathBuf,
    stable_bundle: PathBuf,
    old_bundle: PathBuf,
    old_pid: u32,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct InstallTransaction {
    arguments: HelperArguments,
    stable_parent: PathBuf,
    staging_bundle: Option<PathBuf>,
    backup_bundle: Option<PathBuf>,
    installed_bundle: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryTarget {
    Stable,
    Old,
    Backup,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct RecoveryReport {
    target: Option<RecoveryTarget>,
    launch_error: Option<String>,
    cleanup_errors: Vec<String>,
}

struct UpdateGuard;

impl UpdateGuard {
    fn acquire() -> Result<Self, String> {
        UPDATE_IN_PROGRESS
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| "An update is already in progress.".to_string())
    }
}

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        UPDATE_IN_PROGRESS.store(false, Ordering::Release);
    }
}

/// Whether this binary has the source metadata needed for the macOS updater.
pub(crate) fn is_supported() -> bool {
    #[cfg(target_os = "macos")]
    {
        !EMBEDDED_SOURCE_ROOT.is_empty()
            && !EMBEDDED_BUILD_COMMIT.is_empty()
            && Path::new(EMBEDDED_SOURCE_ROOT).is_dir()
            && Path::new(EMBEDDED_SOURCE_ROOT).join(".git").exists()
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Check the repository HEAD on every invocation. The caller intentionally
/// does not cache this result because the frontend invokes it from its poll.
pub(crate) fn current_status() -> UpdateStatus {
    let current_commit = EMBEDDED_BUILD_COMMIT.to_string();

    #[cfg(target_os = "macos")]
    {
        let latest_commit = git_head(Path::new(EMBEDDED_SOURCE_ROOT)).unwrap_or_default();
        return UpdateStatus {
            available: commits_differ(&current_commit, &latest_commit),
            current_commit,
            latest_commit,
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        UpdateStatus {
            available: false,
            current_commit,
            latest_commit: String::new(),
        }
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn check_for_update() -> UpdateStatus {
    // `git` can be slow on a large checkout and must not block the webview
    // thread. It still performs a fresh HEAD lookup for every command call.
    tauri::async_runtime::spawn_blocking(current_status)
        .await
        .unwrap_or_else(|_| UpdateStatus {
            available: false,
            current_commit: EMBEDDED_BUILD_COMMIT.to_string(),
            latest_commit: String::new(),
        })
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub(crate) async fn check_for_update() -> UpdateStatus {
    current_status()
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub(crate) async fn update_and_restart() -> Result<(), String> {
    Err("Self-updating is only available on macOS.".into())
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub(crate) async fn update_and_restart(app: tauri::AppHandle) -> Result<(), String> {
    let _guard = UpdateGuard::acquire()?;
    emit_update_progress(&app, UpdateProgress::validating());
    let progress_app = app.clone();
    let plan = tauri::async_runtime::spawn_blocking(move || prepare_update(&progress_app))
        .await
        .map_err(|error| format!("Update task failed: {error}"))??;

    emit_update_progress(&app, UpdateProgress::preparing());
    spawn_update_helper(&plan)?;

    // The helper waits for this process to exit before replacing the installed
    // bundle. ExitRequested still runs through the existing shutdown path,
    // which stops managed tasks and persists window state.
    emit_update_progress(&app, UpdateProgress::restarting());
    // Give the webview a bounded moment to paint the final stage before this
    // process exits and the detached helper takes over.
    let _ = tauri::async_runtime::spawn_blocking(|| thread::sleep(UPDATE_RESTART_GRACE)).await;
    app.exit(0);
    Ok(())
}

#[cfg(target_os = "macos")]
fn prepare_update(app: &tauri::AppHandle) -> Result<UpdatePlan, String> {
    if !is_supported() {
        return Err(
            "Self-updating is unavailable because the source repository could not be found.".into(),
        );
    }

    let source_root = Path::new(EMBEDDED_SOURCE_ROOT);
    let starting_commit = git_head(source_root)?;
    if starting_commit == EMBEDDED_BUILD_COMMIT {
        return Err("No committed source update is available.".into());
    }
    ensure_worktree_clean(source_root)?;

    let current_executable = env::current_exe()
        .map_err(|error| format!("Could not determine the running app: {error}"))?;
    let stable_bundle = stable_app_path()?;
    let old_bundle = bundle_path_from_executable(&current_executable).ok_or_else(|| {
        "Self-updating is only available when running from a macOS app bundle.".to_string()
    })?;
    let built_bundle = release_bundle_path(source_root);
    if old_bundle == built_bundle {
        return Err(
            "Self-updating is unavailable while running directly from the release build output."
                .into(),
        );
    }

    emit_update_progress(app, UpdateProgress::building());
    run_release_build(source_root)?;

    // A commit or worktree edit during the build means the produced app is no
    // longer a reproducible build of the commit checked above. Leave the
    // current app installed and ask the user to retry from a stable checkout.
    let ending_commit = git_head(source_root)?;
    ensure_worktree_clean(source_root)?;
    if ending_commit != starting_commit {
        return Err("The source HEAD changed while building. Retry the update.".into());
    }
    if !built_bundle.is_dir() {
        return Err(format!(
            "The release build did not produce {}.",
            built_bundle.display()
        ));
    }
    reject_symlink(&built_bundle)?;

    Ok(UpdatePlan {
        built_bundle,
        stable_bundle,
        old_bundle,
        old_pid: std::process::id(),
    })
}

#[cfg(target_os = "macos")]
fn run_release_build(source_root: &Path) -> Result<(), String> {
    // A login shell supplies the user's Node/npm installation when the app was
    // launched from Finder, where the GUI environment usually has a minimal
    // PATH. The checkout path is passed as $1 instead of interpolated into the
    // shell script, so spaces and shell metacharacters remain harmless.
    let target_directory = release_target_directory(source_root);
    let output = Command::new("/bin/zsh")
        .arg("-l")
        .arg("-c")
        .arg("cd -- \"$1\" && export CARGO_TARGET_DIR=\"$2\" && exec npm run tauri build -- --bundles app")
        .arg("cutting-board-update")
        .arg(source_root)
        .arg(&target_directory)
        .env("CARGO_TARGET_DIR", &target_directory)
        .current_dir(source_root)
        .output()
        .map_err(|error| {
            format!("Could not start the release build ({UPDATE_BUILD_COMMAND}): {error}")
        })?;
    if output.status.success() {
        return Ok(());
    }

    let diagnostics = command_diagnostics(&output.stdout, &output.stderr);
    if diagnostics.is_empty() {
        Err(format!(
            "Release build failed ({UPDATE_BUILD_COMMAND}, status {}).",
            output.status
        ))
    } else {
        Err(format!(
            "Release build failed ({UPDATE_BUILD_COMMAND}): {diagnostics}"
        ))
    }
}

#[cfg(target_os = "macos")]
fn spawn_update_helper(plan: &UpdatePlan) -> Result<(), String> {
    let executable = env::current_exe()
        .map_err(|error| format!("Could not locate the running app for restart: {error}"))?;
    let mut command = Command::new(executable);
    command
        .arg(UPDATE_HELPER_ARGUMENT)
        .arg(&plan.built_bundle)
        .arg(&plan.stable_bundle)
        .arg(&plan.old_bundle)
        .arg(plan.old_pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not start the update helper: {error}"))
}

#[cfg(target_os = "macos")]
pub(crate) fn run_helper_if_requested() -> bool {
    let mut arguments = env::args_os().skip(1);
    let Some(first) = arguments.next() else {
        return false;
    };
    if first != OsStr::new(UPDATE_HELPER_ARGUMENT) {
        return false;
    }

    let values = arguments.collect::<Vec<_>>();
    match parse_helper_arguments(&values).and_then(install_and_launch) {
        Ok(()) => std::process::exit(0),
        Err(error) => {
            eprintln!("Cutting Board update failed: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run_helper_if_requested() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn install_and_launch(arguments: HelperArguments) -> Result<(), String> {
    // Validate all paths before waiting for (and therefore losing) the
    // running app. The helper is detached, so accepting a forged path here
    // would otherwise let an arbitrary invocation replace another app.
    validate_helper_arguments(&arguments)?;
    wait_for_process_exit(arguments.old_pid)?;

    let stable_parent = arguments
        .stable_bundle
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "The installed app path has no parent directory.".to_string())?;
    let mut transaction = InstallTransaction {
        arguments,
        stable_parent,
        staging_bundle: None,
        backup_bundle: None,
        installed_bundle: false,
    };

    match transaction.install() {
        Ok(()) => Ok(()),
        Err(error) => {
            let recovery = transaction.recover();
            Err(format_transaction_failure(error, recovery))
        }
    }
}

#[cfg(target_os = "macos")]
impl InstallTransaction {
    fn install(&mut self) -> Result<(), String> {
        if !self.arguments.built_bundle.is_dir() {
            return Err(format!(
                "The release bundle is missing: {}.",
                self.arguments.built_bundle.display()
            ));
        }
        reject_symlink(&self.arguments.built_bundle)?;

        fs::create_dir_all(&self.stable_parent).map_err(|error| {
            format!(
                "Could not create the Applications directory {}: {error}",
                self.stable_parent.display()
            )
        })?;

        let stable_exists = match fs::symlink_metadata(&self.arguments.stable_bundle) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!(
                    "Could not inspect the installed app path {}: {error}",
                    self.arguments.stable_bundle.display()
                ));
            }
        };
        if stable_exists {
            reject_symlink(&self.arguments.stable_bundle)?;
            if !self.arguments.stable_bundle.is_dir() {
                return Err(format!(
                    "The installed app path is not a bundle directory: {}.",
                    self.arguments.stable_bundle.display()
                ));
            }
        }

        // Copy to the same parent directory as the stable app first. This
        // handles repositories on an external volume; the final renames are
        // then atomic regardless of the source checkout's volume.
        let staging_bundle = unique_staging_path(&self.stable_parent);
        self.staging_bundle = Some(staging_bundle.clone());
        copy_bundle(&self.arguments.built_bundle, &staging_bundle)?;

        if stable_exists {
            let backup_bundle = unique_backup_path(&self.stable_parent);
            fs::rename(&self.arguments.stable_bundle, &backup_bundle).map_err(|error| {
                format!("Could not stage the existing app before installing the update: {error}")
            })?;
            self.backup_bundle = Some(backup_bundle);
        }

        fs::rename(&staging_bundle, &self.arguments.stable_bundle)
            .map_err(|error| format!("Could not install the release bundle: {error}"))?;
        self.staging_bundle = None;
        self.installed_bundle = true;

        launch_bundle(&self.arguments.stable_bundle)
    }

    fn recover(&mut self) -> RecoveryReport {
        let mut cleanup_errors = Vec::new();

        if let Some(staging_bundle) = self.staging_bundle.take() {
            if path_is_available(&staging_bundle) {
                let quarantine = unique_backup_path(&self.stable_parent);
                if let Err(error) = fs::rename(&staging_bundle, &quarantine) {
                    cleanup_errors.push(format!(
                        "Could not quarantine the incomplete update {}: {error}",
                        staging_bundle.display()
                    ));
                }
            }
        }

        if self.installed_bundle && path_is_available(&self.arguments.stable_bundle) {
            let quarantine = unique_backup_path(&self.stable_parent);
            if let Err(error) = fs::rename(&self.arguments.stable_bundle, &quarantine) {
                cleanup_errors.push(format!(
                    "Could not quarantine the failed update {}: {error}",
                    self.arguments.stable_bundle.display()
                ));
            }
        }

        let backup_bundle = self.backup_bundle.take();
        let restored_stable = if let Some(backup_bundle) = backup_bundle {
            if path_is_available(&self.arguments.stable_bundle) {
                cleanup_errors.push(format!(
                    "Could not restore the previous app because {} is still present.",
                    self.arguments.stable_bundle.display()
                ));
                self.backup_bundle = Some(backup_bundle);
                false
            } else {
                match fs::rename(&backup_bundle, &self.arguments.stable_bundle) {
                    Ok(()) => true,
                    Err(error) => {
                        cleanup_errors.push(format!(
                            "Could not restore the previous app from {}: {error}",
                            backup_bundle.display()
                        ));
                        self.backup_bundle = Some(backup_bundle);
                        false
                    }
                }
            }
        } else {
            false
        };

        let old_available = path_is_available(&self.arguments.old_bundle);
        let backup_available = self.backup_bundle.as_deref().is_some_and(path_is_available);
        let stable_available = path_is_available(&self.arguments.stable_bundle);
        let target = select_recovery_target(
            restored_stable,
            old_available,
            self.arguments.old_bundle == self.arguments.stable_bundle,
            backup_available,
            stable_available,
        );
        let launch_error = target.and_then(|target| {
            let path = match target {
                RecoveryTarget::Stable => &self.arguments.stable_bundle,
                RecoveryTarget::Old => &self.arguments.old_bundle,
                RecoveryTarget::Backup => self.backup_bundle.as_deref()?,
            };
            launch_bundle(path).err()
        });

        RecoveryReport {
            target,
            launch_error,
            cleanup_errors,
        }
    }
}

#[cfg(target_os = "macos")]
fn format_transaction_failure(error: String, recovery: RecoveryReport) -> String {
    let mut message = error;
    match (recovery.target, recovery.launch_error) {
        (Some(_), None) => message.push_str(" The previous app was relaunched."),
        (Some(_), Some(error)) => {
            message.push_str(&format!(" Relaunching the previous app failed: {error}."));
        }
        (None, _) => message.push_str(" The previous app could not be found to relaunch."),
    }
    for error in recovery.cleanup_errors {
        message.push(' ');
        message.push_str(&error);
        message.push('.');
    }
    message
}

#[cfg(target_os = "macos")]
fn validate_helper_arguments(arguments: &HelperArguments) -> Result<(), String> {
    let expected_built_bundle = release_bundle_path(Path::new(EMBEDDED_SOURCE_ROOT));
    let expected_stable_bundle = stable_app_path()?;
    let helper_executable = env::current_exe()
        .map_err(|error| format!("Could not determine the update helper executable: {error}"))?;
    let expected_old_bundle = bundle_path_from_executable(&helper_executable)
        .ok_or_else(|| "The update helper is not running from a macOS app bundle.".to_string())?;
    if !helper_paths_match(
        arguments,
        &expected_built_bundle,
        &expected_stable_bundle,
        &expected_old_bundle,
    ) {
        return Err("The update helper rejected unexpected app paths.".into());
    }
    if arguments.old_pid == 0 {
        return Err("The update helper received an invalid previous app PID.".into());
    }
    Ok(())
}

fn helper_paths_match(
    arguments: &HelperArguments,
    expected_built_bundle: &Path,
    expected_stable_bundle: &Path,
    expected_old_bundle: &Path,
) -> bool {
    arguments.built_bundle == expected_built_bundle
        && arguments.stable_bundle == expected_stable_bundle
        && arguments.old_bundle == expected_old_bundle
}

#[cfg(target_os = "macos")]
fn copy_bundle(source: &Path, destination: &Path) -> Result<(), String> {
    let status = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(destination)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Could not copy the release bundle with ditto: {error}"))?;
    if !status.success() {
        return Err(format!(
            "Could not copy the release bundle with ditto (status {status})."
        ));
    }
    if !path_is_available(destination) {
        return Err(format!(
            "ditto did not produce the staging bundle {}.",
            destination.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn path_is_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
        .unwrap_or(false)
}

fn select_recovery_target(
    restored_stable: bool,
    old_available: bool,
    old_is_stable: bool,
    backup_available: bool,
    stable_available: bool,
) -> Option<RecoveryTarget> {
    if restored_stable && stable_available {
        Some(RecoveryTarget::Stable)
    } else if old_available && !old_is_stable {
        Some(RecoveryTarget::Old)
    } else if backup_available {
        Some(RecoveryTarget::Backup)
    } else if stable_available {
        Some(RecoveryTarget::Stable)
    } else if old_available {
        Some(RecoveryTarget::Old)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn wait_for_process_exit(pid: u32) -> Result<(), String> {
    if pid == 0 {
        return Err("The running app PID is invalid.".into());
    }
    for _ in 0..HELPER_WAIT_ATTEMPTS {
        if !process_is_alive(pid) {
            return Ok(());
        }
        thread::sleep(HELPER_WAIT_INTERVAL);
    }
    Err("The previous app did not exit in time; its bundle was left unchanged.".into())
}

#[cfg(target_os = "macos")]
fn process_is_alive(pid: u32) -> bool {
    // The helper only checks the process created by this app. Treat EPERM as
    // alive so an unexpected permission boundary never leads to replacement.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(target_os = "macos")]
fn launch_bundle(bundle: &Path) -> Result<(), String> {
    if !bundle.is_dir() {
        return Err(format!("App bundle does not exist: {}", bundle.display()));
    }
    let status = Command::new("/usr/bin/open")
        .arg("-n")
        .arg(bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("Could not launch {}: {error}", bundle.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "The system refused to launch {} (status {status}).",
            bundle.display()
        ))
    }
}

#[cfg(target_os = "macos")]
fn reject_symlink(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("Refusing to replace symlink {}.", path.display()));
    }
    Ok(())
}

fn parse_helper_arguments(arguments: &[OsString]) -> Result<HelperArguments, String> {
    if arguments.len() != 4 {
        return Err(
            "The update helper received invalid arguments and did not alter the installed app."
                .into(),
        );
    }
    let old_pid = arguments[3]
        .to_str()
        .ok_or_else(|| "The update helper received an invalid process ID.".to_string())?
        .parse::<u32>()
        .map_err(|_| "The update helper received an invalid process ID.".to_string())?;
    Ok(HelperArguments {
        built_bundle: PathBuf::from(&arguments[0]),
        stable_bundle: PathBuf::from(&arguments[1]),
        old_bundle: PathBuf::from(&arguments[2]),
        old_pid,
    })
}

fn git_head(source_root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg("HEAD")
        .current_dir(source_root)
        .output()
        .map_err(|error| format!("Could not run git in {}: {error}", source_root.display()))?;
    if !output.status.success() {
        return Err(format!(
            "Could not read the source HEAD in {}.",
            source_root.display()
        ));
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if commit.is_empty() {
        Err(format!(
            "The source repository in {} has no commit.",
            source_root.display()
        ))
    } else {
        Ok(commit)
    }
}

fn ensure_worktree_clean(source_root: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .arg("status")
        .arg("--porcelain=v1")
        .arg("--untracked-files=all")
        .current_dir(source_root)
        .output()
        .map_err(|error| format!("Could not inspect the source worktree: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Could not inspect the source worktree in {}.",
            source_root.display()
        ));
    }
    let status = String::from_utf8_lossy(&output.stdout);
    if !worktree_status_is_clean(&status) {
        return Err(
            "The source worktree has uncommitted or untracked files. Commit or remove them before updating."
                .into(),
        );
    }
    Ok(())
}

fn command_diagnostics(stdout: &[u8], stderr: &[u8]) -> String {
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    );
    let mut lines = combined.lines().rev().take(24).collect::<Vec<_>>();
    lines.reverse();
    let diagnostics = lines.join(" ").trim().to_string();
    const MAX_DIAGNOSTICS: usize = 2_000;
    if diagnostics.len() > MAX_DIAGNOSTICS {
        let start = diagnostics
            .char_indices()
            .rev()
            .find_map(|(index, _)| (index <= diagnostics.len() - MAX_DIAGNOSTICS).then_some(index))
            .unwrap_or(0);
        diagnostics[start..].to_string()
    } else {
        diagnostics
    }
}

pub(crate) fn worktree_status_is_clean(status: &str) -> bool {
    status.lines().all(|line| line.trim().is_empty())
}

pub(crate) fn commits_differ(current_commit: &str, latest_commit: &str) -> bool {
    !current_commit.is_empty() && !latest_commit.is_empty() && current_commit != latest_commit
}

pub(crate) fn release_bundle_path(source_root: &Path) -> PathBuf {
    release_target_directory(source_root).join("release/bundle/macos/Cutting Board.app")
}

pub(crate) fn release_target_directory(source_root: &Path) -> PathBuf {
    source_root.join("src-tauri/target")
}

pub(crate) fn bundle_path_from_executable(executable: &Path) -> Option<PathBuf> {
    executable.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?.to_str()?;
        name.ends_with(".app").then(|| ancestor.to_path_buf())
    })
}

#[cfg(target_os = "macos")]
fn stable_app_path() -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join("Applications/Cutting Board.app"))
        .ok_or_else(|| "Could not determine the current user's home directory.".into())
}

#[cfg(target_os = "macos")]
fn unique_backup_path(parent: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let prefix = format!(
        ".Cutting Board.update-backup-{}-{timestamp}",
        std::process::id()
    );
    let mut candidate = parent.join(&prefix);
    let mut suffix = 0_u32;
    while candidate.exists() {
        suffix = suffix.saturating_add(1);
        candidate = parent.join(format!("{prefix}-{suffix}"));
    }
    candidate
}

#[cfg(target_os = "macos")]
fn unique_staging_path(parent: &Path) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let prefix = format!(
        ".Cutting Board.update-staging-{}-{timestamp}",
        std::process::id()
    );
    let mut candidate = parent.join(&prefix);
    let mut suffix = 0_u32;
    while candidate.exists() {
        suffix = suffix.saturating_add(1);
        candidate = parent.join(format!("{prefix}-{suffix}"));
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_non_empty_git_status_as_dirty() {
        assert!(worktree_status_is_clean(""));
        assert!(worktree_status_is_clean("\n  \n"));
        assert!(!worktree_status_is_clean(" M src/main.ts\n"));
        assert!(!worktree_status_is_clean("?? scratch.txt\n"));
    }

    #[test]
    fn compares_non_empty_commits() {
        assert!(!commits_differ("abc", "abc"));
        assert!(commits_differ("abc", "def"));
        assert!(!commits_differ("", "def"));
        assert!(!commits_differ("abc", ""));
    }

    #[test]
    fn progress_stages_are_ordered_and_complete() {
        let progress = [
            UpdateProgress::validating(),
            UpdateProgress::building(),
            UpdateProgress::preparing(),
            UpdateProgress::restarting(),
        ];

        assert_eq!(
            progress
                .iter()
                .map(|item| item.stage.as_str())
                .collect::<Vec<_>>(),
            ["validating", "building", "preparing", "restarting"]
        );
        assert_eq!(
            progress.iter().map(|item| item.step).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert!(progress
            .iter()
            .all(|item| item.total == UPDATE_PROGRESS_TOTAL));
        assert!(progress.iter().all(|item| !item.message.is_empty()));
    }

    #[test]
    fn derives_bundle_path_from_macos_executable() {
        let executable =
            Path::new("/Users/example/Applications/Cutting Board.app/Contents/MacOS/cutting-board");
        assert_eq!(
            bundle_path_from_executable(executable),
            Some(PathBuf::from(
                "/Users/example/Applications/Cutting Board.app"
            ))
        );
        assert_eq!(
            bundle_path_from_executable(Path::new("/tmp/cutting-board")),
            None
        );
    }

    #[test]
    fn parses_helper_arguments_without_shell_interpolation() {
        let arguments = vec![
            OsString::from("/tmp/with spaces/Cutting Board.app"),
            OsString::from("/Users/me/Applications/Cutting Board.app"),
            OsString::from("/old/Cutting Board.app"),
            OsString::from("42"),
        ];
        let parsed = parse_helper_arguments(&arguments).unwrap();
        assert_eq!(parsed.old_pid, 42);
        assert_eq!(parsed.built_bundle, PathBuf::from(&arguments[0]));
    }

    #[test]
    fn rejects_unexpected_helper_argument_count() {
        assert!(parse_helper_arguments(&[]).is_err());
        assert!(parse_helper_arguments(&[
            OsString::from("a"),
            OsString::from("b"),
            OsString::from("c"),
            OsString::from("not-a-pid"),
        ])
        .is_err());
    }

    #[test]
    fn recovery_prefers_the_restored_stable_bundle_once() {
        assert_eq!(
            select_recovery_target(true, true, true, false, true),
            Some(RecoveryTarget::Stable)
        );
    }

    #[test]
    fn recovery_uses_previous_bundle_when_stable_restore_failed() {
        assert_eq!(
            select_recovery_target(false, true, false, true, false),
            Some(RecoveryTarget::Old)
        );
        assert_eq!(
            select_recovery_target(false, false, true, true, false),
            Some(RecoveryTarget::Backup)
        );
    }

    #[test]
    fn recovery_returns_no_target_when_no_safe_bundle_exists() {
        assert_eq!(
            select_recovery_target(false, false, true, false, false),
            None
        );
    }

    #[test]
    fn helper_paths_must_match_all_trusted_paths() {
        let expected_built =
            Path::new("/checkout/src-tauri/target/release/bundle/macos/Cutting Board.app");
        let expected_stable = Path::new("/Users/me/Applications/Cutting Board.app");
        let expected_old = Path::new("/Users/me/Applications/Cutting Board.app");
        let arguments = HelperArguments {
            built_bundle: expected_built.to_path_buf(),
            stable_bundle: expected_stable.to_path_buf(),
            old_bundle: expected_old.to_path_buf(),
            old_pid: 9,
        };
        assert!(helper_paths_match(
            &arguments,
            expected_built,
            expected_stable,
            expected_old
        ));

        let mut forged = arguments;
        forged.built_bundle = PathBuf::from("/tmp/other.app");
        assert!(!helper_paths_match(
            &forged,
            expected_built,
            expected_stable,
            expected_old
        ));
    }
}
