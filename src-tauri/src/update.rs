#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::launch::shell_command;
use serde::Serialize;
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod build_output;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use tauri::Emitter;

const EMBEDDED_SOURCE_ROOT: &str = env!("CUTTING_BOARD_SOURCE_ROOT");
const EMBEDDED_BUILD_COMMIT: &str = env!("CUTTING_BOARD_BUILD_COMMIT");
const UPDATE_HELPER_ARGUMENT: &str = "--update-helper";
const UPDATE_PROGRESS_EVENT: &str = "update-progress";
const UPDATE_PROGRESS_TOTAL: u8 = 4;

#[cfg(target_os = "macos")]
const UPDATE_BUILD_COMMAND: &str = "npm run tauri build -- --bundles app";

#[cfg(target_os = "linux")]
const UPDATE_BUILD_COMMAND: &str = "npm run tauri build -- --no-bundle";

#[cfg(any(target_os = "macos", target_os = "linux"))]
const UPDATE_PREPARING_MIN_DURATION: Duration = Duration::from_millis(800);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const UPDATE_RESTARTING_MIN_DURATION: Duration = Duration::from_millis(1_000);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const HELPER_WAIT_ATTEMPTS: usize = 120;
#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
            detail: None,
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn building_with_detail(detail: String) -> Self {
        Self::building().with_detail(detail)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

/// Whether this binary has the source metadata needed for the local updater.
pub(crate) fn is_supported() -> bool {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        !EMBEDDED_SOURCE_ROOT.is_empty()
            && !EMBEDDED_BUILD_COMMIT.is_empty()
            && Path::new(EMBEDDED_SOURCE_ROOT).is_dir()
            && Path::new(EMBEDDED_SOURCE_ROOT).join(".git").exists()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Check the repository HEAD on every invocation. The caller intentionally
/// does not cache this result because the frontend invokes it from its poll.
pub(crate) fn current_status() -> UpdateStatus {
    let current_commit = EMBEDDED_BUILD_COMMIT.to_string();

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let latest_commit = git_head(Path::new(EMBEDDED_SOURCE_ROOT)).unwrap_or_default();
        UpdateStatus {
            available: commits_differ(&current_commit, &latest_commit),
            current_commit,
            latest_commit,
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        UpdateStatus {
            available: false,
            current_commit,
            latest_commit: String::new(),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn check_for_update() -> UpdateStatus {
    current_status()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[tauri::command]
pub(crate) async fn update_and_restart() -> Result<(), String> {
    Err("Self-updating is unavailable on this platform.".into())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tauri::command]
pub(crate) async fn update_and_restart(app: tauri::AppHandle) -> Result<(), String> {
    let _guard = UpdateGuard::acquire()?;
    emit_update_progress(&app, UpdateProgress::validating());
    let progress_app = app.clone();
    let plan = tauri::async_runtime::spawn_blocking(move || prepare_update(&progress_app))
        .await
        .map_err(|error| format!("Update task failed: {error}"))??;

    let preparing_started_at = Instant::now();
    emit_update_progress(&app, UpdateProgress::preparing());
    spawn_update_helper(&plan)?;
    sleep_remaining(preparing_started_at, UPDATE_PREPARING_MIN_DURATION).await;

    // The helper waits for this process to exit before replacing the installed
    // bundle. ExitRequested still persists window state; managed launch tasks
    // intentionally survive the app restart and are recovered afterward.
    let restarting_started_at = Instant::now();
    emit_update_progress(&app, UpdateProgress::restarting());
    // Give the webview a bounded moment to paint the final stage before this
    // process exits and the detached helper takes over.
    sleep_remaining(restarting_started_at, UPDATE_RESTARTING_MIN_DURATION).await;
    app.exit(0);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn remaining_stage_delay(elapsed: Duration, minimum: Duration) -> Duration {
    minimum.saturating_sub(elapsed)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn sleep_remaining(started_at: Instant, minimum: Duration) {
    let remaining = remaining_stage_delay(started_at.elapsed(), minimum);
    if remaining.is_zero() {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || thread::sleep(remaining)).await;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    let stable_bundle = stable_install_path(&current_executable)?;
    let old_bundle = old_install_path(&current_executable)?;
    let built_bundle = release_bundle_path(source_root);
    if paths_refer_to_same_entry(&old_bundle, &built_bundle) {
        return Err(
            "Self-updating is unavailable while running directly from the release build output."
                .into(),
        );
    }

    emit_update_progress(app, UpdateProgress::building());
    run_release_build(app, source_root)?;

    // A commit or worktree edit during the build means the produced app is no
    // longer a reproducible build of the commit checked above. Leave the
    // current app installed and ask the user to retry from a stable checkout.
    let ending_commit = git_head(source_root)?;
    ensure_worktree_clean(source_root)?;
    if ending_commit != starting_commit {
        return Err("The source HEAD changed while building. Retry the update.".into());
    }
    if !artifact_path_is_available(&built_bundle) {
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_release_build(app: &tauri::AppHandle, source_root: &Path) -> Result<(), String> {
    // Use the same configured login-interactive shell as managed tasks so
    // Node version managers initialized from an interactive rc file are
    // available when the app is launched from a desktop environment.
    let child = release_build_command(source_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            format!("Could not start the release build ({UPDATE_BUILD_COMMAND}): {error}")
        })?;

    let result = build_output::collect(app, child)?;
    if !result.status.success() {
        let diagnostics = result.diagnostics;
        let mut message = if diagnostics.is_empty() {
            format!(
                "Release build failed ({UPDATE_BUILD_COMMAND}, status {}).",
                result.status
            )
        } else {
            format!("Release build failed ({UPDATE_BUILD_COMMAND}): {diagnostics}")
        };
        if let Some(error) = result.reader_error {
            message.push_str(&format!(" Output capture failed: {error}"));
        }
        return Err(message);
    }
    if let Some(error) = result.reader_error {
        return Err(format!(
            "Release build output could not be captured: {error}"
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn release_build_command(source_root: &Path) -> Command {
    let target_directory = release_target_directory(source_root);
    let mut command = shell_command(UPDATE_BUILD_COMMAND);
    command
        .env("CARGO_TARGET_DIR", &target_directory)
        .current_dir(source_root);
    command
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn run_helper_if_requested() -> bool {
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl InstallTransaction {
    fn install(&mut self) -> Result<(), String> {
        if !artifact_path_is_available(&self.arguments.built_bundle) {
            if fs::symlink_metadata(&self.arguments.built_bundle).is_ok() {
                reject_symlink(&self.arguments.built_bundle)?;
            }
            return Err(format!(
                "The release artifact is missing: {}.",
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
            if !artifact_path_is_available(&self.arguments.stable_bundle) {
                return Err(format!(
                    "The installed app path is not a valid release artifact: {}.",
                    self.arguments.stable_bundle.display()
                ));
            }
        }

        // Copy to the same parent directory as the stable app first. This
        // handles repositories on an external volume; the final renames are
        // then atomic regardless of the source checkout's volume.
        let staging_bundle = unique_staging_path(&self.stable_parent);
        #[cfg(target_os = "macos")]
        {
            self.staging_bundle = Some(staging_bundle.clone());
        }
        copy_artifact(&self.arguments.built_bundle, &staging_bundle)?;
        #[cfg(target_os = "linux")]
        {
            self.staging_bundle = Some(staging_bundle.clone());
        }

        if stable_exists {
            let backup_bundle = unique_backup_path(&self.stable_parent);
            #[cfg(target_os = "macos")]
            fs::rename(&self.arguments.stable_bundle, &backup_bundle).map_err(|error| {
                format!("Could not stage the existing app before installing the update: {error}")
            })?;
            #[cfg(target_os = "linux")]
            copy_artifact(&self.arguments.stable_bundle, &backup_bundle)?;
            self.backup_bundle = Some(backup_bundle);
        }

        fs::rename(&staging_bundle, &self.arguments.stable_bundle)
            .map_err(|error| format!("Could not install the release bundle: {error}"))?;
        self.staging_bundle = None;
        self.installed_bundle = true;

        launch_artifact(&self.arguments.stable_bundle)
    }

    fn recover(&mut self) -> RecoveryReport {
        let mut cleanup_errors = Vec::new();

        if let Some(staging_bundle) = self.staging_bundle.take() {
            if artifact_path_is_available(&staging_bundle) {
                let quarantine = unique_backup_path(&self.stable_parent);
                if let Err(error) = fs::rename(&staging_bundle, &quarantine) {
                    cleanup_errors.push(format!(
                        "Could not quarantine the incomplete update {}: {error}",
                        staging_bundle.display()
                    ));
                }
            }
        }

        if self.installed_bundle && artifact_path_is_available(&self.arguments.stable_bundle) {
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
            if artifact_path_is_available(&self.arguments.stable_bundle) {
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

        let old_available = artifact_path_is_available(&self.arguments.old_bundle);
        let backup_available = self
            .backup_bundle
            .as_deref()
            .is_some_and(artifact_path_is_available);
        let stable_available = artifact_path_is_available(&self.arguments.stable_bundle);
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
            launch_artifact(path).err()
        });

        RecoveryReport {
            target,
            launch_error,
            cleanup_errors,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn validate_helper_arguments(arguments: &HelperArguments) -> Result<(), String> {
    let expected_built_bundle = release_bundle_path(Path::new(EMBEDDED_SOURCE_ROOT));
    let helper_executable = env::current_exe()
        .map_err(|error| format!("Could not determine the update helper executable: {error}"))?;
    let expected_stable_bundle = stable_install_path(&helper_executable)?;
    let expected_old_bundle = old_install_path(&helper_executable)?;
    if !helper_paths_match(
        arguments,
        &expected_built_bundle,
        &expected_stable_bundle,
        &expected_old_bundle,
    ) {
        return Err("The update helper rejected unexpected app paths.".into());
    }
    for path in [
        &arguments.built_bundle,
        &arguments.stable_bundle,
        &arguments.old_bundle,
    ] {
        reject_existing_symlink(path)?;
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn reject_existing_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => reject_symlink(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Could not inspect {}: {error}", path.display())),
    }
}

#[cfg(target_os = "macos")]
fn copy_artifact(source: &Path, destination: &Path) -> Result<(), String> {
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
    if !artifact_path_is_available(destination) {
        return Err(format!(
            "ditto did not produce the staging bundle {}.",
            destination.display()
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn copy_artifact(source: &Path, destination: &Path) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut source_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)
        .map_err(|error| format!("Could not open the release binary: {error}"))?;
    let source_metadata = source_file
        .metadata()
        .map_err(|error| format!("Could not inspect the release binary: {error}"))?;
    if !source_metadata.file_type().is_file() {
        return Err(format!(
            "The release binary is not a regular file: {}.",
            source.display()
        ));
    }

    let mut destination_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(source_metadata.permissions().mode())
        .open(destination)
        .map_err(|error| {
            format!(
                "Could not create the release binary staging path {}: {error}",
                destination.display()
            )
        })?;
    let copy_result: Result<(), String> = (|| {
        std::io::copy(&mut source_file, &mut destination_file)
            .map_err(|error| format!("Could not copy the release binary: {error}"))?;
        destination_file
            .set_permissions(fs::Permissions::from_mode(
                source_metadata.permissions().mode(),
            ))
            .map_err(|error| format!("Could not preserve release binary permissions: {error}"))?;
        destination_file
            .flush()
            .map_err(|error| format!("Could not flush the release binary copy: {error}"))?;
        destination_file
            .sync_all()
            .map_err(|error| format!("Could not sync the release binary copy: {error}"))?;
        Ok(())
    })();
    drop(destination_file);

    if let Err(error) = copy_result {
        let cleanup = fs::remove_file(destination).map_err(|cleanup_error| {
            format!(
                " Could not remove the incomplete release binary copy {}: {cleanup_error}.",
                destination.display()
            )
        });
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup_error) => Err(format!("{error}.{cleanup_error}")),
        };
    }

    if !artifact_path_is_available(destination) {
        let error = format!(
            "Copying the release binary did not produce {}.",
            destination.display()
        );
        return match fs::remove_file(destination) {
            Ok(()) => Err(error),
            Err(cleanup_error) if cleanup_error.kind() == std::io::ErrorKind::NotFound => {
                Err(error)
            }
            Err(cleanup_error) => Err(format!(
                "{error} Could not remove the incomplete release binary copy {}: {cleanup_error}.",
                destination.display()
            )),
        };
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn artifact_path_is_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn artifact_path_is_available(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink() && metadata.is_file())
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    Err("The previous app did not exit in time; its installed artifact was left unchanged.".into())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn process_is_alive(pid: u32) -> bool {
    // The helper only checks the process created by this app. Treat EPERM as
    // alive so an unexpected permission boundary never leads to replacement.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(target_os = "macos")]
fn launch_artifact(bundle: &Path) -> Result<(), String> {
    if !artifact_path_is_available(bundle) {
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

#[cfg(target_os = "linux")]
fn launch_artifact(binary: &Path) -> Result<(), String> {
    if !artifact_path_is_available(binary) {
        return Err(format!(
            "Application binary does not exist: {}",
            binary.display()
        ));
    }

    // Desktop launches use this wrapper to focus an existing window. Reuse it
    // when the stable binary follows the normal per-user installation layout;
    // custom binary locations can still be relaunched directly.
    let launcher = linux_launcher_path(binary);
    Command::new(&launcher)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch {}: {error}", binary.display()))
}

#[cfg(target_os = "linux")]
fn linux_launcher_path(binary: &Path) -> PathBuf {
    let launcher = (binary.file_name() == Some(OsStr::new("cutting-board")))
        .then(|| {
            binary
                .parent()
                .map(|parent| parent.join("cutting-board-launch"))
        })
        .flatten()
        .filter(|path| {
            use std::os::unix::fs::PermissionsExt;

            fs::symlink_metadata(path)
                .map(|metadata| {
                    !metadata.file_type().is_symlink()
                        && metadata.is_file()
                        && metadata.permissions().mode() & 0o111 != 0
                })
                .unwrap_or(false)
        });
    launcher.unwrap_or_else(|| binary.to_path_buf())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

pub(crate) fn worktree_status_is_clean(status: &str) -> bool {
    status.lines().all(|line| line.trim().is_empty())
}

pub(crate) fn commits_differ(current_commit: &str, latest_commit: &str) -> bool {
    !current_commit.is_empty() && !latest_commit.is_empty() && current_commit != latest_commit
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn paths_refer_to_same_entry(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    if let (Ok(left), Ok(right)) = (fs::canonicalize(left), fs::canonicalize(right)) {
        return left == right;
    }
    false
}

pub(crate) fn release_bundle_path(source_root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        release_target_directory(source_root).join("release/bundle/macos/Cutting Board.app")
    }

    #[cfg(target_os = "linux")]
    {
        release_target_directory(source_root).join("release/cutting-board")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        release_target_directory(source_root).join("release/cutting-board")
    }
}

pub(crate) fn release_target_directory(source_root: &Path) -> PathBuf {
    source_root.join("src-tauri/target")
}

#[cfg(target_os = "macos")]
pub(crate) fn bundle_path_from_executable(executable: &Path) -> Option<PathBuf> {
    executable.ancestors().find_map(|ancestor| {
        let name = ancestor.file_name()?.to_str()?;
        name.ends_with(".app").then(|| ancestor.to_path_buf())
    })
}

#[cfg(target_os = "macos")]
fn stable_install_path(_current_executable: &Path) -> Result<PathBuf, String> {
    dirs::home_dir()
        .map(|home| home.join("Applications/Cutting Board.app"))
        .ok_or_else(|| "Could not determine the current user's home directory.".into())
}

#[cfg(target_os = "macos")]
fn old_install_path(current_executable: &Path) -> Result<PathBuf, String> {
    bundle_path_from_executable(current_executable)
        .ok_or_else(|| "The updater is not running from a macOS app bundle.".to_string())
}

#[cfg(target_os = "linux")]
fn stable_install_path(_current_executable: &Path) -> Result<PathBuf, String> {
    // The desktop entry and launcher script use this per-user path. Keeping it
    // fixed lets an app started from a debug or another custom location update
    // the same installed binary and relaunch through its normal wrapper.
    dirs::home_dir()
        .map(|home| home.join(".local/bin/cutting-board"))
        .ok_or_else(|| "Could not determine the current user's home directory.".into())
}

#[cfg(target_os = "linux")]
fn old_install_path(current_executable: &Path) -> Result<PathBuf, String> {
    Ok(current_executable.to_path_buf())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

#[cfg(any(target_os = "macos", target_os = "linux"))]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_release_path_targets_the_unbundled_binary() {
        assert_eq!(
            release_bundle_path(Path::new("/checkout")),
            PathBuf::from("/checkout/src-tauri/target/release/cutting-board")
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_stable_path_uses_the_fixed_install_location() {
        let executable = Path::new("/home/example/.local/bin/cutting-board");
        assert_eq!(
            stable_install_path(executable).unwrap(),
            dirs::home_dir().unwrap().join(".local/bin/cutting-board")
        );
        assert_eq!(old_install_path(executable).unwrap(), executable);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_artifact_checks_reject_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("cutting-board");
        let link = directory.path().join("link");
        fs::write(&binary, b"binary").unwrap();
        std::os::unix::fs::symlink(&binary, &link).unwrap();

        assert!(artifact_path_is_available(&binary));
        assert!(!artifact_path_is_available(&link));
        assert!(reject_existing_symlink(&link).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_binary_copy_is_exclusive_and_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        let existing = directory.path().join("existing");
        let existing_target = directory.path().join("existing-target");
        let existing_link = directory.path().join("existing-link");
        fs::write(&source, b"new").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o751)).unwrap();

        copy_artifact(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o7777,
            0o751
        );

        fs::write(&existing, b"old").unwrap();
        assert!(copy_artifact(&source, &existing).is_err());
        assert_eq!(fs::read(&existing).unwrap(), b"old");

        fs::write(&existing_target, b"old-target").unwrap();
        std::os::unix::fs::symlink(&existing_target, &existing_link).unwrap();
        assert!(copy_artifact(&source, &existing_link).is_err());
        assert_eq!(fs::read(&existing_target).unwrap(), b"old-target");

        let source_link = directory.path().join("source-link");
        let link_destination = directory.path().join("link-destination");
        std::os::unix::fs::symlink(&source, &source_link).unwrap();
        assert!(copy_artifact(&source_link, &link_destination).is_err());
        assert!(!link_destination.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_launcher_requires_an_executable_regular_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("cutting-board");
        let launcher = directory.path().join("cutting-board-launch");
        fs::write(&binary, b"binary").unwrap();
        fs::write(&launcher, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(linux_launcher_path(&binary), binary);

        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(linux_launcher_path(&binary), launcher);

        let launcher_target = directory.path().join("launcher-target");
        fs::write(&launcher_target, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&launcher_target, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(&launcher).unwrap();
        std::os::unix::fs::symlink(&launcher_target, &launcher).unwrap();
        assert_eq!(linux_launcher_path(&binary), binary);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_failed_launch_restores_the_previous_binary() {
        let directory = tempfile::tempdir().unwrap();
        let built = directory.path().join("built");
        let stable = directory.path().join("stable");
        fs::write(&built, b"new").unwrap();
        fs::write(&stable, b"old").unwrap();
        use std::os::unix::fs::PermissionsExt;

        let executable_permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&built, executable_permissions.clone()).unwrap();
        fs::set_permissions(&stable, executable_permissions).unwrap();

        let mut transaction = InstallTransaction {
            arguments: HelperArguments {
                built_bundle: built,
                stable_bundle: stable.clone(),
                old_bundle: stable.clone(),
                old_pid: 1,
            },
            stable_parent: directory.path().to_path_buf(),
            staging_bundle: None,
            backup_bundle: None,
            installed_bundle: false,
        };

        assert!(transaction.install().is_err());
        let recovery = transaction.recover();
        assert_eq!(recovery.target, Some(RecoveryTarget::Stable));
        assert!(recovery.launch_error.is_some());
        assert_eq!(fs::read(stable).unwrap(), b"old");
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
        assert!(progress.iter().all(|item| item.detail.is_none()));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn remaining_stage_delay_only_covers_unelapsed_time() {
        assert_eq!(
            remaining_stage_delay(Duration::from_millis(200), Duration::from_millis(800)),
            Duration::from_millis(600)
        );
        assert_eq!(
            remaining_stage_delay(Duration::from_millis(800), Duration::from_millis(800)),
            Duration::ZERO
        );
        assert_eq!(
            remaining_stage_delay(Duration::from_millis(900), Duration::from_millis(800)),
            Duration::ZERO
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn release_build_uses_the_configured_login_interactive_shell() {
        let source_root = Path::new("/checkout/with spaces");
        let target_directory = release_target_directory(source_root);
        let command = release_build_command(source_root);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(args, ["-l", "-i", "-c", UPDATE_BUILD_COMMAND]);
        assert_eq!(command.get_current_dir(), Some(source_root));
        assert!(command.get_envs().any(|(key, value)| {
            key == OsStr::new("CARGO_TARGET_DIR") && value == Some(target_directory.as_os_str())
        }));
    }

    #[cfg(target_os = "macos")]
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
