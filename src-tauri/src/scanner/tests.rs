use super::*;

fn project() -> ProjectInfo {
    ProjectInfo {
        id: "project-test".into(),
        name: "test-project".into(),
        root_path: "/work/test-project".into(),
        detection_source: "Cargo.toml".into(),
        workspace_root_path: "/work/test-project".into(),
        workspace_name: "test-project".into(),
    }
}

#[test]
fn parses_ipv4_and_ipv6_listeners() {
    assert_eq!(parse_endpoint("127.0.0.1:5173").unwrap().port, 5173);
    assert_eq!(parse_endpoint("[::1]:8080").unwrap().scope, "loopback");
    assert_eq!(parse_endpoint("*:3000").unwrap().scope, "wildcard");
}

#[test]
fn redacts_secret_values() {
    let args = vec![
        "server".into(),
        "--token=abc".into(),
        "--password".into(),
        "value".into(),
    ];
    let value = truncate_command(&redact_command_full(&args, "server"));
    assert!(!value.contains("abc"));
    assert!(!value.contains("value"));
}

#[test]
fn keeps_full_redacted_commands_separate_from_display_truncation() {
    let args = vec![
        "java".into(),
        "-cp".into(),
        "a".repeat(700),
        "com.example.Application".into(),
    ];
    let full = redact_command_full(&args, "java");
    let display = truncate_command(&full);

    assert!(full.len() > 700);
    assert_eq!(display.chars().count(), 600);
    assert!(display.ends_with('…'));
    assert!(full.ends_with("com.example.Application"));
}

#[test]
fn does_not_redact_secret_words_inside_classpaths() {
    let args = vec![
        "java".into(),
        "-cp".into(),
        "/cache/io.jsonwebtoken/jjwt-impl/0.12.6/jjwt-impl.jar".into(),
        "com.example.Application".into(),
    ];

    assert_eq!(redact_command_full(&args, "java"), args.join(" "));
}

#[test]
fn redacts_sensitive_jvm_properties() {
    let args = vec!["java".into(), "-Dspring.datasource.password=secret".into()];
    let value = redact_command_full(&args, "java");

    assert_eq!(value, "java -Dspring.datasource.password=•••");
}

#[test]
fn reads_spring_context_path() {
    let args = vec!["java".into(), "--server.servlet.context-path=/api".into()];
    assert_eq!(spring_context_path(&args).as_deref(), Some("/api"));
}

#[test]
fn never_treats_home_as_a_project_root() {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
    assert!(is_excluded_project_root(&home));
    assert!(is_excluded_project_root(Path::new("/tmp")));
}

#[test]
fn arbitrary_high_port_is_not_development_evidence() {
    let java = classify(
        "java",
        Some("/usr/bin/java"),
        &["java".into()],
        &[33_357],
        None,
    );
    assert_eq!(java.relevance, "noise");

    let unknown = classify("helper", Some("/home/dev/bin/helper"), &[], &[38_383], None);
    assert_eq!(unknown.relevance, "noise");
}

#[test]
fn generic_runtime_in_a_real_project_remains_visible() {
    let project = project();
    let result = classify(
        "node",
        Some("/usr/bin/node"),
        &["node".into(), "server.js".into()],
        &[31_337],
        Some(&project),
    );
    assert_eq!(result.relevance, "dev");
    assert_eq!(result.tech, "nodejs");
}

#[test]
fn applications_executables_are_noise() {
    let result = classify(
        "node",
        Some("/Applications/Visual Studio Code.app/Contents/MacOS/Electron"),
        &["node".into(), "server.js".into()],
        &[3000],
        Some(&project()),
    );
    assert_eq!(result.relevance, "noise");
    assert_eq!(result.category, "other");
}

#[test]
fn non_application_executables_keep_their_existing_classification() {
    let ollama = classify(
        "ollama",
        Some("/opt/homebrew/bin/ollama"),
        &["ollama".into(), "serve".into()],
        &[11_434],
        None,
    );
    assert_eq!(ollama.relevance, "dev");
    assert_eq!(ollama.tech, "ollama");

    let project_binary = classify(
        "node",
        Some("/work/test-project/node_modules/.bin/node"),
        &["node".into(), "/Applications/project/server.js".into()],
        &[31_337],
        Some(&project()),
    );
    assert_eq!(project_binary.relevance, "dev");
    assert_eq!(project_binary.tech, "nodejs");

    let similarly_named_directory = classify(
        "ollama",
        Some("/Applications2/Ollama.app/Contents/MacOS/ollama"),
        &["ollama".into(), "serve".into()],
        &[11_434],
        None,
    );
    assert_eq!(similarly_named_directory.relevance, "dev");
}

#[test]
fn known_framework_remains_visible_on_an_arbitrary_port() {
    let result = classify(
        "node",
        Some("/usr/bin/node"),
        &["node".into(), "node_modules/vite/bin/vite.js".into()],
        &[31_337],
        None,
    );
    assert_eq!(result.relevance, "dev");
    assert_eq!(result.tech, "vite");
}

#[test]
fn build_daemons_and_ide_helpers_are_noise() {
    let gradle = classify(
        "java",
        Some("/usr/lib/jvm/java-17/bin/java"),
        &[
            "java".into(),
            "-cp".into(),
            "/home/dev/.gradle/lib/gradle-daemon-main-8.14.3.jar".into(),
            "org.gradle.launcher.daemon.bootstrap.GradleDaemon".into(),
        ],
        &[42_469],
        Some(&project()),
    );
    assert_eq!(gradle.relevance, "noise");

    let idea = classify(
        "java",
        Some("/usr/lib/jvm/java-17/bin/java"),
        &["java".into(), "com.intellij.idea.Main".into()],
        &[44_641],
        Some(&project()),
    );
    assert_eq!(idea.relevance, "noise");
}

#[test]
fn gradle_classpath_does_not_hide_the_application() {
    let project = project();
    let result = classify(
        "java",
        Some("/usr/bin/java"),
        &[
            "java".into(),
            "-cp".into(),
            "/home/dev/.gradle/caches/spring-boot-3.2.0.jar".into(),
            "com.acme.Application".into(),
        ],
        &[35_000],
        Some(&project),
    );
    assert_eq!(result.relevance, "dev");
    assert_eq!(result.tech, "spring");
}

#[test]
fn invite_path_does_not_match_vite() {
    let result = classify(
        "node",
        Some("/usr/bin/node"),
        &["node".into(), "/work/invite-app/server.js".into()],
        &[35_000],
        None,
    );
    assert_eq!(result.relevance, "noise");
}

#[test]
fn nested_service_keeps_its_name_and_uses_the_vcs_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("oasis26");
    let backend = workspace.join("backend");
    fs::create_dir_all(&backend).unwrap();
    fs::create_dir(workspace.join(".git")).unwrap();
    fs::write(backend.join("build.gradle"), "plugins {}\n").unwrap();

    let project = detect_project(Some(&backend), &[]).unwrap();

    assert_eq!(project.name, "backend");
    assert_eq!(Path::new(&project.root_path), backend);
    assert_eq!(project.workspace_name, "oasis26");
    assert_eq!(Path::new(&project.workspace_root_path), workspace);
}

#[test]
fn spring_config_selects_http_port_and_context_and_hides_livereload() {
    let temporary = tempfile::tempdir().unwrap();
    let resources = temporary.path().join("src/main/resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(
        resources.join("application.properties"),
        "server.port=48080\nserver.servlet.context-path=/oasis\n",
    )
    .unwrap();
    let settings =
        resolve_spring_settings(&[], &[], Some(temporary.path()), Some(temporary.path()));
    assert_eq!(settings.port, Some(48_080));
    assert_eq!(settings.context_path, "/oasis/");

    let listener = |port| ListenerRecord {
        pid: 42,
        uid: Some(1000),
        process_name: "java".into(),
        endpoint: Endpoint {
            family: "IPv4".into(),
            address: "127.0.0.1".into(),
            port,
            scope: "loopback".into(),
            protocol: "TCP".into(),
        },
    };
    let mut listeners = vec![listener(35_729), listener(48_080)];
    assert!(refine_spring_listeners(&mut listeners, &settings));
    let endpoints = listeners
        .into_iter()
        .map(|record| record.endpoint)
        .collect::<Vec<_>>();
    assert_eq!(
        endpoints
            .iter()
            .map(|endpoint| endpoint.port)
            .collect::<Vec<_>>(),
        vec![48_080]
    );
    assert_eq!(
        browser_url("api", &endpoints, &[], Some(&settings)).as_deref(),
        Some("http://localhost:48080/oasis/")
    );
}

#[test]
fn spring_argv_and_environment_override_file_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(
        temporary.path().join("application.yml"),
        "server:\n  port: 8080\n  servlet:\n    context-path: /base\n",
    )
    .unwrap();
    let settings = resolve_spring_settings(
        &["java".into(), "--server.port=49000".into()],
        &["SERVER_SERVLET_CONTEXT_PATH=/env".into()],
        Some(temporary.path()),
        Some(temporary.path()),
    );
    assert_eq!(settings.port, Some(49_000));
    assert_eq!(settings.context_path, "/env/");
}

#[test]
fn active_profiles_follow_command_environment_and_file_priority() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(
        temporary.path().join("application.properties"),
        "spring.profiles.active=file-a, file-b\n",
    )
    .unwrap();

    let command = resolve_spring_settings(
        &[
            "java".into(),
            "--spring.profiles.active=command-a,command-b".into(),
        ],
        &["SPRING_PROFILES_ACTIVE=environment".into()],
        Some(temporary.path()),
        Some(temporary.path()),
    );
    assert_eq!(command.active_profiles, vec!["command-a", "command-b"]);

    let environment = resolve_spring_settings(
        &["java".into()],
        &["SPRING_PROFILES_ACTIVE=environment-a, environment-b".into()],
        Some(temporary.path()),
        Some(temporary.path()),
    );
    assert_eq!(
        environment.active_profiles,
        vec!["environment-a", "environment-b"]
    );

    let file = resolve_spring_settings(
        &["java".into()],
        &[],
        Some(temporary.path()),
        Some(temporary.path()),
    );
    assert_eq!(file.active_profiles, vec!["file-a", "file-b"]);
}

#[test]
fn active_profile_environment_placeholder_selects_profile_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(
        temporary.path().join("application.properties"),
        "spring.profiles.active=${APP_PROFILE:dev}\nserver.port=8080\n",
    )
    .unwrap();
    fs::write(
        temporary.path().join("application-local.properties"),
        "server.port=49000\n",
    )
    .unwrap();
    fs::write(
        temporary.path().join("application-dev.properties"),
        "server.port=48000\n",
    )
    .unwrap();

    let settings = resolve_spring_settings(
        &[],
        &["APP_PROFILE=local".into()],
        Some(temporary.path()),
        Some(temporary.path()),
    );

    assert_eq!(settings.active_profiles, vec!["local"]);
    assert_eq!(settings.port, Some(49_000));
}

#[test]
fn active_profile_default_placeholder_selects_profile_configuration() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(
        temporary.path().join("application.properties"),
        "spring.profiles.active=${APP_PROFILE:dev}\nserver.port=8080\n",
    )
    .unwrap();
    fs::write(
        temporary.path().join("application-dev.properties"),
        "server.port=48000\n",
    )
    .unwrap();

    let settings =
        resolve_spring_settings(&[], &[], Some(temporary.path()), Some(temporary.path()));

    assert_eq!(settings.active_profiles, vec!["dev"]);
    assert_eq!(settings.port, Some(48_000));
}

#[test]
fn spring_yaml_only_applies_the_active_profile_document() {
    let properties = parse_spring_yaml(
            "server:\n  port: 8080\n---\nspring:\n  config:\n    activate:\n      on-profile: local\nserver:\n  port: 48080\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nserver:\n  port: 8443\n",
            &["local".into()],
        );
    assert_eq!(
        properties.get("server.port").map(String::as_str),
        Some("48080")
    );
}

#[test]
fn missing_configured_spring_port_preserves_observed_listener() {
    let settings = SpringSettings {
        port: Some(48_080),
        ..SpringSettings::default()
    };
    let mut companion = vec![ListenerRecord {
        pid: 7,
        uid: Some(1000),
        process_name: "java".into(),
        endpoint: Endpoint {
            family: "IPv4".into(),
            address: "127.0.0.1".into(),
            port: 40_801,
            scope: "loopback".into(),
            protocol: "TCP".into(),
        },
    }];
    assert!(refine_spring_listeners(&mut companion, &settings));
    assert_eq!(companion[0].endpoint.port, 40_801);
}

#[test]
fn management_port_equal_to_server_port_keeps_the_application() {
    let settings = SpringSettings {
        port: Some(48_080),
        management_port: Some(48_080),
        ..SpringSettings::default()
    };
    let listener = |port| ListenerRecord {
        pid: 8,
        uid: Some(1000),
        process_name: "java".into(),
        endpoint: Endpoint {
            family: "IPv4".into(),
            address: "127.0.0.1".into(),
            port,
            scope: "loopback".into(),
            protocol: "TCP".into(),
        },
    };
    let mut listeners = vec![listener(35_729), listener(48_080)];
    assert!(refine_spring_listeners(&mut listeners, &settings));
    assert_eq!(listeners.len(), 1);
    assert_eq!(listeners[0].endpoint.port, 48_080);
}

#[test]
fn project_spring_evidence_classifies_plain_java_jar() {
    let temporary = tempfile::tempdir().unwrap();
    fs::write(
        temporary.path().join("build.gradle"),
        "plugins { id 'org.springframework.boot' version '3.3.0' }\n",
    )
    .unwrap();
    let project = ProjectInfo {
        root_path: temporary.path().to_string_lossy().into_owned(),
        ..project()
    };
    let result = classify(
        "java",
        Some("/usr/lib/jvm/java-17/bin/java"),
        &["java".into(), "-jar".into(), "backend.jar".into()],
        &[48_080],
        Some(&project),
    );
    assert_eq!(result.tech, "spring");
    assert_eq!(result.category, "api");
    assert_eq!(result.relevance, "dev");
}

#[test]
fn spring_application_config_is_project_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let resources = temporary.path().join("src/main/resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(
        resources.join("application.properties"),
        "server.port=48080\n",
    )
    .unwrap();
    let project = ProjectInfo {
        root_path: temporary.path().to_string_lossy().into_owned(),
        ..project()
    };
    assert!(project_has_spring_evidence(&project));
}

#[test]
fn nested_repository_uses_its_nearest_vcs_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let outer = temporary.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(inner.join("src")).unwrap();
    fs::create_dir(outer.join(".git")).unwrap();
    fs::create_dir(inner.join(".git")).unwrap();
    fs::write(
        outer.join(".gitmodules"),
        "[submodule \"other\"]\n\tpath = other\n\turl = https://example.com/other.git\n",
    )
    .unwrap();
    fs::write(inner.join("Cargo.toml"), "[package]\nname = \"inner\"\n").unwrap();

    let project = detect_project(Some(&inner.join("src")), &[]).unwrap();

    assert_eq!(Path::new(&project.workspace_root_path), inner);
    assert_eq!(project.workspace_name, "inner");
}

#[test]
fn registered_sibling_submodules_share_the_superproject_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let superproject = temporary.path().join("OASIS");
    let front = superproject.join("front");
    let server = superproject.join("services/server");
    fs::create_dir_all(front.join("src")).unwrap();
    fs::create_dir_all(server.join("src")).unwrap();
    fs::create_dir(superproject.join(".git")).unwrap();
    fs::write(front.join(".git"), "gitdir: ../.git/modules/front\n").unwrap();
    fs::write(
        server.join(".git"),
        "gitdir: ../../.git/modules/services/server\n",
    )
    .unwrap();
    fs::write(
        superproject.join(".gitmodules"),
        "# Registered child repositories\n\n[submodule \"front\"]\n\tpath = front\n\turl = https://example.com/front.git\n\n[submodule \"server\"]\n\tpath = \"services/server\" # A nested submodule path\n\turl = https://example.com/server.git\n",
    )
    .unwrap();

    for child in [&front, &server] {
        let project = detect_project(Some(&child.join("src")), &[]).unwrap();

        assert_eq!(Path::new(&project.root_path), *child);
        assert_eq!(Path::new(&project.workspace_root_path), superproject);
        assert_eq!(project.workspace_name, "OASIS");
    }
}

#[test]
fn concrete_argv_module_beats_cwd_repository_root() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("oasis26");
    let backend = workspace.join("backend");
    let classes = backend.join("build/classes");
    fs::create_dir_all(&classes).unwrap();
    fs::create_dir(workspace.join(".git")).unwrap();
    fs::write(backend.join("build.gradle"), "plugins {}\n").unwrap();

    let project =
        detect_project(Some(&workspace), &[classes.to_string_lossy().into_owned()]).unwrap();

    assert_eq!(Path::new(&project.root_path), backend);
    assert_eq!(project.name, "backend");
    assert_eq!(Path::new(&project.workspace_root_path), workspace);
}

#[test]
fn recognizes_windows_absolute_paths_and_daemon_classpaths() {
    assert!(looks_like_absolute_path(r"C:\work\oasis26\backend"));
    assert!(looks_like_absolute_path(r"\\server\share\backend"));
    assert!(is_build_daemon(
        &["java".into()],
        &[
            "java".into(),
            "-cp".into(),
            r"C:\gradle\lib\gradle-daemon-main-8.14.3.jar;C:\gradle\lib\other.jar".into(),
        ],
    ));
}

#[test]
fn ssh_tunnel_in_a_project_is_a_proxy_service() {
    let result = classify(
        "ssh",
        Some("/usr/bin/ssh"),
        &[
            "ssh".into(),
            "-L".into(),
            "48983:localhost:8983".into(),
            "oasis".into(),
        ],
        &[48_983],
        Some(&project()),
    );
    assert_eq!(result.relevance, "dev");
    assert_eq!(result.tech, "ssh");
    assert_eq!(result.category, "proxy");
}
