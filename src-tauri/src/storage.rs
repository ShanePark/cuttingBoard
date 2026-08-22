use crate::models::{LaunchProfile, LaunchTask, UiSettings};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub fn read_json_or_default<T>(path: &Path) -> Result<T, String>
where
    T: DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(&text)
        .map_err(|error| format!("Could not parse {}: {error}", path.display()))
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    let temporary = temporary_path(path);
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("Could not serialize {}: {error}", path.display()))?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("Could not create {}: {error}", temporary.display()))?;
        file.write_all(&bytes)
            .and_then(|_| file.write_all(b"\n"))
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
    }
    fs::rename(&temporary, path)
        .map_err(|error| format!("Could not replace {}: {error}", path.display()))?;
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_default()
        .to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

pub fn load_settings(path: &Path) -> Result<UiSettings, String> {
    read_json_or_default::<UiSettings>(path).map(UiSettings::normalized)
}

pub fn save_settings(path: &Path, settings: UiSettings) -> Result<UiSettings, String> {
    let normalized = settings.normalized();
    write_json_atomic(path, &normalized)?;
    Ok(normalized)
}

pub fn load_profiles(path: &Path) -> Result<Vec<LaunchProfile>, String> {
    let mut profiles = read_json_or_default::<Vec<LaunchProfile>>(path)?;
    let migrated = profiles
        .iter_mut()
        .map(migrate_legacy_profile)
        .any(|changed| changed);
    for profile in &profiles {
        validate_profile(profile)?;
    }
    if migrated {
        write_json_atomic(path, &profiles)?;
    }
    Ok(profiles)
}

fn migrate_legacy_profile(profile: &mut LaunchProfile) -> bool {
    let project_root = PathBuf::from(profile.project_root.trim());
    profile
        .tasks
        .iter_mut()
        .map(|task| repair_legacy_java_command(&project_root, task))
        .any(|changed| changed)
}

fn repair_legacy_java_command(project_root: &Path, task: &mut LaunchTask) -> bool {
    let mut parts = task.command.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let Some(classpath_index) = parts.iter().position(|part| matches!(part.as_str(), "-cp" | "-classpath" | "--class-path")) else {
        return false;
    };
    let Some(classpath) = parts.get(classpath_index + 1) else { return false };
    if !classpath.contains("spring-boot") || parts.get(classpath_index + 2).map(String::as_str) != Some("•••") {
        return false;
    }
    let Some(main_class) = find_spring_boot_main_class(project_root) else { return false };
    parts[classpath_index + 2] = main_class;
    task.command = parts.join(" ");
    true
}

fn find_spring_boot_main_class(project_root: &Path) -> Option<String> {
    ["src/main/kotlin", "src/main/java"]
        .into_iter()
        .map(|relative| project_root.join(relative))
        .find_map(|root| find_spring_boot_main_class_in(&root))
}

fn find_spring_boot_main_class_in(directory: &Path) -> Option<String> {
    let entries = fs::read_dir(directory).ok()?.filter_map(Result::ok).collect::<Vec<_>>();
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_spring_boot_main_class_in(&path) {
                return Some(found);
            }
            continue;
        }
        if !matches!(path.extension().and_then(|value| value.to_str()), Some("java" | "kt")) {
            continue;
        }
        let text = fs::read_to_string(&path).ok()?;
        if !text.contains("@SpringBootApplication") {
            continue;
        }
        let class_name = text.lines().find_map(|line| {
            let name = line.split_once("class ")?.1.split(|character: char| !character.is_ascii_alphanumeric() && character != '_').next()?;
            (!name.is_empty()).then_some(name.to_string())
        })?;
        let package = text.lines().find_map(|line| line.trim().strip_prefix("package "))
            .map(|value| value.trim().trim_end_matches(';'))
            .filter(|value| !value.is_empty());
        return Some(package.map_or(class_name.clone(), |value| format!("{value}.{class_name}")));
    }
    None
}

pub fn save_profile(path: &Path, profile: LaunchProfile) -> Result<Vec<LaunchProfile>, String> {
    validate_profile(&profile)?;
    let mut profiles = load_profiles(path)?;
    if let Some(existing) = profiles.iter_mut().find(|item| item.id == profile.id) {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
    profiles.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    write_json_atomic(path, &profiles)?;
    Ok(profiles)
}

pub fn delete_profile(path: &Path, profile_id: &str) -> Result<Vec<LaunchProfile>, String> {
    let mut profiles = load_profiles(path)?;
    let before = profiles.len();
    profiles.retain(|profile| profile.id != profile_id);
    if profiles.len() == before {
        return Err("The launch profile no longer exists.".into());
    }
    write_json_atomic(path, &profiles)?;
    Ok(profiles)
}

pub fn validate_profile(profile: &LaunchProfile) -> Result<(), String> {
    if profile.id.trim().is_empty() || profile.id.len() > 128 {
        return Err("A launch profile needs a stable identifier.".into());
    }
    if profile.name.trim().is_empty() || profile.name.len() > 80 {
        return Err("Profile names must contain 1 to 80 characters.".into());
    }
    let root = Path::new(profile.project_root.trim());
    if profile.project_root.trim().is_empty() || !root.is_absolute() {
        return Err("The project root must be an absolute path.".into());
    }
    if profile.tasks.is_empty() {
        return Err("A launch profile needs at least one task.".into());
    }
    let mut names = HashSet::new();
    for task in &profile.tasks {
        if task.name.trim().is_empty() || task.name.len() > 80 {
            return Err("Task names must contain 1 to 80 characters.".into());
        }
        if !names.insert(task.name.trim().to_lowercase()) {
            return Err(format!("Task names must be unique: {}", task.name));
        }
        if task.cwd.trim().is_empty() {
            return Err(format!("{} needs a working directory.", task.name));
        }
        if task.command.trim().is_empty() {
            return Err(format!("{} needs a command.", task.name));
        }
    }
    Ok(())
}

pub fn demo_profiles() -> Vec<LaunchProfile> {
    vec![LaunchProfile {
        id: "demo-profile".into(),
        name: "Cutting Board Demo".into(),
        project_root: "/Users/shane/Developer/cutting-board-demo".into(),
        tasks: vec![
            LaunchTask {
                name: "API".into(),
                cwd: "backend".into(),
                command: "./gradlew bootRun".into(),
                expected_port: Some(8080),
            },
            LaunchTask {
                name: "Frontend".into(),
                cwd: "frontend".into(),
                command: "npm run dev".into(),
                expected_port: Some(5173),
            },
        ],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_duplicate_task_names() {
        let profile = LaunchProfile {
            id: "x".into(),
            name: "Example".into(),
            project_root: "/tmp/example".into(),
            tasks: vec![
                LaunchTask { name: "API".into(), cwd: ".".into(), command: "one".into(), expected_port: None },
                LaunchTask { name: "api".into(), cwd: ".".into(), command: "two".into(), expected_port: None },
            ],
        };
        assert!(validate_profile(&profile).is_err());
    }

    #[test]
    fn settings_are_clamped() {
        let value = UiSettings { theme_mode: "invalid".into(), scan_interval_ms: 1, window_width: 1, window_height: 1, window_x: None, window_y: None, window_geometry_logical: false }.normalized();
        assert_eq!(value.theme_mode, "dark");
        assert_eq!(value.scan_interval_ms, 500);
        assert_eq!(value.window_width, 560);
    }

    #[test]
    fn migrates_redacted_spring_boot_main_class_on_load() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary.path().join("src/main/kotlin/com/example/DemoApplication.kt");
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(
            &source,
            "package com.example\n\n@SpringBootApplication\nclass DemoApplication\n",
        )
        .unwrap();
        let profile_path = temporary.path().join("profiles.json");
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "Demo".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "api".into(),
                cwd: ".".into(),
                command: "java -cp /cache/spring-boot.jar ••• --spring.profiles.active=dev".into(),
                expected_port: Some(8080),
            }],
        };
        write_json_atomic(&profile_path, &vec![profile]).unwrap();

        let profiles = load_profiles(&profile_path).unwrap();

        assert_eq!(
            profiles[0].tasks[0].command,
            "java -cp /cache/spring-boot.jar com.example.DemoApplication --spring.profiles.active=dev"
        );
    }
}
