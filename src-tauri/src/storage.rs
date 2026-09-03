#[path = "storage/json.rs"]
mod json;
#[path = "storage/migration.rs"]
mod migration;
#[path = "storage/profiles.rs"]
mod profiles;

pub use json::{load_settings, save_settings};
pub use profiles::{delete_profile, demo_profiles, load_profiles, save_profile};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{LaunchProfile, LaunchTask, UiSettings};
    use std::fs;

    #[test]
    fn rejects_duplicate_task_names() {
        let profile = LaunchProfile {
            id: "x".into(),
            name: "Example".into(),
            project_root: "/tmp/example".into(),
            tasks: vec![
                LaunchTask {
                    name: "API".into(),
                    cwd: ".".into(),
                    command: "one".into(),
                    expected_port: None,
                    container: None,
                    prepare: None,
                },
                LaunchTask {
                    name: "api".into(),
                    cwd: ".".into(),
                    command: "two".into(),
                    expected_port: None,
                    container: None,
                    prepare: None,
                },
            ],
        };
        assert!(profiles::validate_profile(&profile).is_err());
    }

    #[test]
    fn a_container_task_is_saved_without_a_command() {
        let profile = |command: &str, container: Option<&str>| LaunchProfile {
            id: "x".into(),
            name: "Example".into(),
            project_root: "/tmp/example".into(),
            tasks: vec![LaunchTask {
                name: "app-db".into(),
                cwd: ".".into(),
                command: command.into(),
                expected_port: Some(5432),
                container: container.map(str::to_owned),
                prepare: None,
            }],
        };
        assert!(profiles::validate_profile(&profile("", Some("app-db"))).is_ok());
        assert!(profiles::validate_profile(&profile("", None)).is_err());
    }

    #[test]
    fn settings_are_clamped() {
        let value = UiSettings {
            theme_mode: "invalid".into(),
            scan_interval_ms: 1,
            window_width: 1,
            window_height: 1,
            window_x: None,
            window_y: None,
            window_geometry_logical: false,
        }
        .normalized();
        assert_eq!(value.theme_mode, "dark");
        assert_eq!(value.scan_interval_ms, 500);
        assert_eq!(value.window_width, 560);
    }

    #[test]
    fn migrates_redacted_spring_boot_main_class_on_load() {
        let temporary = tempfile::tempdir().unwrap();
        let source = temporary
            .path()
            .join("src/main/kotlin/com/example/DemoApplication.kt");
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
                container: None,
                prepare: None,
            }],
        };
        json::write_json_atomic(&profile_path, &vec![profile]).unwrap();

        let profiles = load_profiles(&profile_path).unwrap();

        assert_eq!(
            profiles[0].tasks[0].command,
            "java -cp /cache/spring-boot.jar com.example.DemoApplication --spring.profiles.active=dev"
        );
    }

    #[test]
    fn migrates_truncated_spring_boot_java_command_to_gradle_wrapper() {
        let temporary = tempfile::tempdir().unwrap();
        let backend = temporary.path().join("backend");
        fs::create_dir(&backend).unwrap();
        fs::write(backend.join("gradlew"), "#!/bin/sh\n").unwrap();
        fs::write(
            backend.join("build.gradle"),
            "plugins { id 'org.springframework.boot' version '2.7.18' }\n",
        )
        .unwrap();
        let profile_path = temporary.path().join("profiles.json");
        let profile = LaunchProfile {
            id: "profile".into(),
            name: "Demo".into(),
            project_root: temporary.path().to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "backend".into(),
                cwd: "backend".into(),
                command: "/opt/java/bin/java -cp /cache/spring-boot.jar:/cache/liquibase/li…"
                    .into(),
                expected_port: Some(8080),
                container: None,
                prepare: None,
            }],
        };
        json::write_json_atomic(&profile_path, &vec![profile]).unwrap();

        let profiles = load_profiles(&profile_path).unwrap();

        assert_eq!(profiles[0].tasks[0].command, "./gradlew bootRun");
        let persisted = json::read_json_or_default::<Vec<LaunchProfile>>(&profile_path).unwrap();
        assert_eq!(persisted[0].tasks[0].command, "./gradlew bootRun");
    }
}
