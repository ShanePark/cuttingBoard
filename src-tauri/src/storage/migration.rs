use crate::models::{LaunchProfile, LaunchTask};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn migrate_legacy_profile(profile: &mut LaunchProfile) -> bool {
    let project_root = PathBuf::from(profile.project_root.trim());
    let mut changed = false;
    for task in &mut profile.tasks {
        changed |= repair_legacy_java_command(&project_root, task);
        changed |= repair_truncated_spring_gradle_command(&project_root, task);
    }
    changed
}

fn repair_legacy_java_command(project_root: &Path, task: &mut LaunchTask) -> bool {
    let mut parts = task
        .command
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let Some(classpath_index) = parts
        .iter()
        .position(|part| matches!(part.as_str(), "-cp" | "-classpath" | "--class-path"))
    else {
        return false;
    };
    let Some(classpath) = parts.get(classpath_index + 1) else {
        return false;
    };
    if !classpath.contains("spring-boot")
        || parts.get(classpath_index + 2).map(String::as_str) != Some("•••")
    {
        return false;
    }
    let Some(main_class) = find_spring_boot_main_class(project_root) else {
        return false;
    };
    parts[classpath_index + 2] = main_class;
    task.command = parts.join(" ");
    true
}

fn repair_truncated_spring_gradle_command(project_root: &Path, task: &mut LaunchTask) -> bool {
    let command = task.command.trim();
    if !command.ends_with('…') {
        return false;
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let is_java = parts
        .first()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        == Some("java");
    let classpath = parts
        .iter()
        .position(|part| matches!(*part, "-cp" | "-classpath" | "--class-path"))
        .and_then(|index| parts.get(index + 1));
    if !is_java || !classpath.is_some_and(|value| value.contains("spring-boot")) {
        return false;
    }

    let cwd = PathBuf::from(task.cwd.trim());
    let task_root = if cwd.is_absolute() {
        cwd
    } else {
        project_root.join(cwd)
    };
    let build_file = [
        task_root.join("build.gradle"),
        task_root.join("build.gradle.kts"),
    ]
    .into_iter()
    .find(|path| path.is_file());
    let Some(build_file) = build_file else {
        return false;
    };
    if !task_root.join("gradlew").is_file() {
        return false;
    }
    let Ok(build_script) = fs::read_to_string(build_file) else {
        return false;
    };
    if !build_script.contains("org.springframework.boot") && !build_script.contains("bootRun") {
        return false;
    }

    task.command = "./gradlew bootRun".into();
    true
}

fn find_spring_boot_main_class(project_root: &Path) -> Option<String> {
    ["src/main/kotlin", "src/main/java"]
        .into_iter()
        .map(|relative| project_root.join(relative))
        .find_map(|root| find_spring_boot_main_class_in(&root))
}

fn find_spring_boot_main_class_in(directory: &Path) -> Option<String> {
    let mut entries = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_spring_boot_main_class_in(&path) {
                return Some(found);
            }
            continue;
        }
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("java" | "kt")
        ) {
            continue;
        }
        let text = fs::read_to_string(&path).ok()?;
        if !text.contains("@SpringBootApplication") {
            continue;
        }
        let class_name = text.lines().find_map(|line| {
            let name = line
                .split_once("class ")?
                .1
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .next()?;
            (!name.is_empty()).then_some(name.to_string())
        })?;
        let package = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("package "))
            .map(|value| value.trim().trim_end_matches(';'))
            .filter(|value| !value.is_empty());
        return Some(package.map_or(class_name.clone(), |value| format!("{value}.{class_name}")));
    }
    None
}
