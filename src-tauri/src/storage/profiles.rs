use super::{
    json::{read_json_or_default, write_json_atomic},
    migration::migrate_legacy_profile,
};
use crate::models::{LaunchProfile, LaunchTask};
use std::{collections::HashSet, path::Path};

pub fn load_profiles(path: &Path) -> Result<Vec<LaunchProfile>, String> {
    let mut profiles = read_json_or_default::<Vec<LaunchProfile>>(path)?;
    let migrated = profiles.iter_mut().any(migrate_legacy_profile);
    for profile in &profiles {
        validate_profile(profile)?;
    }
    if migrated {
        write_json_atomic(path, &profiles)?;
    }
    Ok(profiles)
}

pub fn save_profile(path: &Path, profile: LaunchProfile) -> Result<Vec<LaunchProfile>, String> {
    validate_profile(&profile)?;
    let mut profiles = load_profiles(path)?;
    if let Some(existing) = profiles.iter_mut().find(|item| item.id == profile.id) {
        *existing = profile;
    } else {
        profiles.push(profile);
    }
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
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

pub(super) fn validate_profile(profile: &LaunchProfile) -> Result<(), String> {
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
