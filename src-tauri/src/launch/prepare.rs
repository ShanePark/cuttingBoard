//! Project-aware preparation for launch tasks.
//!
//! A task normally remains a user-authored shell command. Spring Boot tasks are a useful
//! exception: the command captured from an IDE can contain a class path pointing at an old local
//! Maven artifact. Preparing the project through its native build tool first keeps shared-module
//! artifacts and the application in sync without requiring a project-specific script.

use super::shell::shell_command;
use crate::models::{
    LaunchBuildTool, LaunchPrepareKind, LaunchPrepareSpec, LaunchProfile, LaunchTask,
};
use sha1::{Digest, Sha1};
use std::{
    collections::HashMap,
    env,
    ffi::{OsStr, OsString},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const PREPARE_CACHE_FILE: &str = ".prepare-cache.json";
const MAX_ANCESTORS: usize = 24;
const EXEC_MAVEN_PLUGIN: &str = "org.codehaus.mojo:exec-maven-plugin:3.5.1:exec";

#[derive(Debug, Default)]
pub(super) struct PrepareCache {
    entries: HashMap<String, String>,
    loaded_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PrepareResult {
    pub(super) skipped: bool,
    pub(super) tool: LaunchBuildTool,
    pub(super) module: Option<String>,
    pub(super) fingerprint: String,
    /// A project-native launch invocation, when enough metadata is available to avoid replaying
    /// an IDE's captured class path. Maven uses exec-maven-plugin's `%classpath` expansion and
    /// Gradle uses the module's BootRun task; callers fall back to the saved shell command when
    /// this is absent.
    pub(super) launch: Option<PreparedLaunch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedLaunch {
    pub(super) program: OsString,
    pub(super) args: Vec<OsString>,
    pub(super) cwd: PathBuf,
    /// Environment assignments captured from the saved command. Project-native launches no
    /// longer replay the shell prefix, so these must be applied explicitly to both the build
    /// tool and the application it starts.
    pub(super) environment: Vec<(OsString, OsString)>,
    /// A JDK explicitly present in the captured Java command, if any. Build tools inherit this
    /// environment so a project that needs (for example) JDK 8 is not silently built with the
    /// desktop's default JDK.
    pub(super) java_home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct PreparePlan {
    tool: LaunchBuildTool,
    build_root: PathBuf,
    module_root: PathBuf,
    module: Option<String>,
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    inputs: Vec<PathBuf>,
    environment: Vec<(OsString, OsString)>,
    java_home: Option<PathBuf>,
}

#[derive(Debug)]
struct ProjectRoots {
    module_root: PathBuf,
    build_root: PathBuf,
}

/// Prepare a Spring Boot task if it has explicit metadata or can be safely inferred from its
/// command and project files. The result is `None` for ordinary shell tasks.
pub(super) fn prepare_task(
    cache: &mut PrepareCache,
    profile: &LaunchProfile,
    task: &LaunchTask,
    logs_dir: &Path,
    log_path: &Path,
) -> Result<Option<PrepareResult>, String> {
    append_log(
        log_path,
        &format!(
            "=== Cutting Board prepare detection started · task={} · cwd={} ===",
            task.name, task.cwd
        ),
    )?;
    let Some(spec) = resolve_spec(profile, task)? else {
        append_log(
            log_path,
            &format!(
                "=== Cutting Board prepare not required · task={} · using saved command ===",
                task.name
            ),
        )?;
        return Ok(None);
    };

    fs::create_dir_all(logs_dir)
        .map_err(|error| format!("Could not create {}: {error}", logs_dir.display()))?;
    let plan = build_plan(profile, task, &spec)?;
    let fingerprint = fingerprint(&plan, task, &spec)?;
    let launch = project_native_launch(&plan, task, &spec);
    let cache_path = logs_dir.join(PREPARE_CACHE_FILE);
    cache.load(&cache_path);
    let cache_key = cache_key(&plan, task, &spec);

    append_log(
        log_path,
        &format!(
            "=== Cutting Board prepare detected · tool={} · module={} · build_root={} ===",
            plan.tool_name(),
            plan.module.as_deref().unwrap_or("(root)"),
            plan.build_root.display()
        ),
    )?;
    if cache.entries.get(&cache_key) == Some(&fingerprint) {
        append_log(
            log_path,
            &format!(
                "=== Cutting Board prepare skipped (unchanged inputs) · cache hit · fingerprint={} ===",
                fingerprint
            ),
        )?;
        return Ok(Some(PrepareResult {
            skipped: true,
            tool: plan.tool,
            module: plan.module,
            fingerprint,
            launch,
        }));
    }

    append_log(
        log_path,
        &format!(
            "=== Cutting Board prepare cache miss · fingerprint={} ===",
            fingerprint
        ),
    )?;
    append_log(
        log_path,
        &format!(
            "=== Cutting Board prepare command · {} ===",
            command_display(&plan)
        ),
    )?;
    append_log(
        log_path,
        "=== Cutting Board prepare output started · stdout/stderr follow ===",
    )?;
    if let Err(error) = run_plan(&plan, log_path, task) {
        let _ = append_log(
            log_path,
            &format!("=== Cutting Board prepare failed · {} ===", error),
        );
        return Err(error);
    }
    append_log(
        log_path,
        &format!(
            "=== Cutting Board prepare completed · tool={} · module={} ===",
            plan.tool_name(),
            plan.module.as_deref().unwrap_or("(root)")
        ),
    )?;
    cache.entries.insert(cache_key, fingerprint.clone());
    cache.save(&cache_path)?;
    Ok(Some(PrepareResult {
        skipped: false,
        tool: plan.tool,
        module: plan.module,
        fingerprint,
        launch,
    }))
}

impl PrepareCache {
    fn load(&mut self, path: &Path) {
        if self.loaded_path.as_deref() == Some(path) {
            return;
        }
        self.loaded_path = Some(path.to_path_buf());
        self.entries = fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let contents = serde_json::to_vec_pretty(&self.entries)
            .map_err(|error| format!("Could not serialize prepare cache: {error}"))?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, contents)
            .map_err(|error| format!("Could not write {}: {error}", temporary.display()))?;
        fs::rename(&temporary, path)
            .map_err(|error| format!("Could not replace {}: {error}", path.display()))
    }
}

fn resolve_spec(
    profile: &LaunchProfile,
    task: &LaunchTask,
) -> Result<Option<LaunchPrepareSpec>, String> {
    if let Some(mut spec) = task.prepare.clone() {
        if spec.kind != LaunchPrepareKind::SpringBoot {
            return Err(format!(
                "{} uses an unsupported launch preparation kind.",
                task.name
            ));
        }
        if spec.build_tool.is_none() {
            spec.build_tool = detect_build_tool(profile, task);
        }
        if spec.profiles.is_empty() {
            spec.profiles = command_active_profiles(&task.command);
        }
        // A project-native Maven launch needs a main class. Generated metadata normally carries
        // it, while old profiles can still be upgraded from their command or source tree. Gradle
        // BootRun resolves its own main class and does not need this field.
        if spec.main_class.is_none() {
            spec.main_class = infer_main_class(profile, task);
        }
        // An explicitly tagged task without a resolvable build tool remains a legacy shell task.
        // This is important for hand-authored profiles that happen to contain Spring wording but
        // live outside a checked-out Maven/Gradle project.
        if spec.build_tool.is_none() {
            return Ok(None);
        }
        return Ok(Some(spec));
    }

    if !looks_like_spring_task(profile, task) {
        return Ok(None);
    }
    let Some(build_tool) = detect_build_tool(profile, task) else {
        // A Java/Spring-looking command in a project without a Maven or Gradle build remains a
        // normal shell task. This preserves old profiles and avoids guessing a new tool.
        return Ok(None);
    };
    let spec = LaunchPrepareSpec {
        kind: LaunchPrepareKind::SpringBoot,
        build_tool: Some(build_tool),
        module: None,
        profiles: command_active_profiles(&task.command),
        main_class: infer_main_class(profile, task),
    };
    Ok(Some(spec))
}

fn looks_like_spring_task(profile: &LaunchProfile, task: &LaunchTask) -> bool {
    let command = task.command.to_lowercase();
    if [
        "spring-boot",
        "springframework",
        "spring_boot",
        "bootrun",
        "spring-boot:run",
    ]
    .iter()
    .any(|needle| command.contains(needle))
    {
        return true;
    }

    let java_command = command_identity(&command)
        .first()
        .is_some_and(|value| matches!(value.as_str(), "java" | "java.exe"));
    java_command && project_has_spring_evidence(profile, task)
}

fn project_has_spring_evidence(profile: &LaunchProfile, task: &LaunchTask) -> bool {
    let task_root = task_root(profile, task);
    for ancestor in ancestors(&task_root) {
        for file in ["pom.xml", "build.gradle", "build.gradle.kts"] {
            let path = ancestor.join(file);
            if fs::read_to_string(path).ok().is_some_and(|contents| {
                let contents = contents.to_lowercase();
                contents.contains("spring-boot") || contents.contains("org.springframework.boot")
            }) {
                return true;
            }
        }
        if ancestor
            .join("src/main/resources/application.properties")
            .is_file()
            || ancestor
                .join("src/main/resources/application.yml")
                .is_file()
            || ancestor
                .join("src/main/resources/application.yaml")
                .is_file()
        {
            return true;
        }
    }
    false
}

fn detect_build_tool(profile: &LaunchProfile, task: &LaunchTask) -> Option<LaunchBuildTool> {
    let task_root = task_root(profile, task);
    for ancestor in ancestors(&task_root) {
        if ancestor.join("pom.xml").is_file()
            || ancestor.join("mvnw").is_file()
            || ancestor.join("mvnw.cmd").is_file()
        {
            return Some(LaunchBuildTool::Maven);
        }
        if ancestor.join("settings.gradle").is_file()
            || ancestor.join("settings.gradle.kts").is_file()
            || ancestor.join("build.gradle").is_file()
            || ancestor.join("build.gradle.kts").is_file()
            || ancestor.join("gradlew").is_file()
            || ancestor.join("gradlew.bat").is_file()
        {
            return Some(LaunchBuildTool::Gradle);
        }
    }
    None
}

fn build_plan(
    profile: &LaunchProfile,
    task: &LaunchTask,
    spec: &LaunchPrepareSpec,
) -> Result<PreparePlan, String> {
    let task_root = task_root(profile, task);
    if !task_root.is_dir() {
        return Err(format!(
            "The task directory does not exist: {}",
            task_root.display()
        ));
    }
    let tool = spec
        .build_tool
        .clone()
        .or_else(|| detect_build_tool(profile, task))
        .ok_or_else(|| {
            format!(
                "Could not find a Maven or Gradle build for Spring Boot task {}.",
                task.name
            )
        })?;
    let roots = project_roots(profile, &tool, spec.module.as_deref(), &task_root)?;
    let module = module_name(
        &roots.build_root,
        &roots.module_root,
        spec.module.as_deref(),
    );
    let (program, mut args) = launcher(&tool, &roots.build_root, &roots.module_root)?;
    match tool {
        LaunchBuildTool::Maven => {
            if let Some(module) = module.as_deref() {
                args.extend([OsString::from("-pl"), OsString::from(module)]);
                args.push(OsString::from("-am"));
            }
            // `install` matters when the captured JVM command points at a local artifact (as
            // IntelliJ often does). `-am` first builds every reactor dependency, including common.
            args.extend([
                OsString::from("install"),
                OsString::from("-Dmaven.test.skip=true"),
            ]);
        }
        LaunchBuildTool::Gradle => {
            let task_name = module
                .as_deref()
                .map(|module| format!(":{}:assemble", module.replace(['/', '\\'], ":")))
                .unwrap_or_else(|| "assemble".into());
            args.push(OsString::from(task_name));
        }
    }
    let inputs = relevant_inputs(&roots.build_root);
    if inputs.is_empty() {
        return Err(format!(
            "Could not find build or source inputs for {}.",
            roots.build_root.display()
        ));
    }
    let environment = command_environment(&task.command);
    let java_home = java_home_for_plan(&task.command, &roots.build_root, &tool);
    Ok(PreparePlan {
        tool,
        build_root: roots.build_root.clone(),
        module_root: roots.module_root,
        module,
        program,
        args,
        cwd: roots.build_root,
        inputs,
        environment,
        java_home,
    })
}

fn project_roots(
    profile: &LaunchProfile,
    tool: &LaunchBuildTool,
    configured_module: Option<&str>,
    task_root: &Path,
) -> Result<ProjectRoots, String> {
    let module_root = configured_module
        .and_then(|module| resolve_configured_module(profile, task_root, module))
        .or_else(|| nearest_build_root(task_root, tool))
        .unwrap_or_else(|| task_root.to_path_buf());

    let build_root = match tool {
        LaunchBuildTool::Maven => find_maven_aggregator(&module_root)
            .unwrap_or_else(|| PathBuf::from(&profile.project_root)),
        LaunchBuildTool::Gradle => {
            find_gradle_root(&module_root).unwrap_or_else(|| PathBuf::from(&profile.project_root))
        }
    };
    let build_root = if build_root.is_dir() {
        build_root
    } else {
        module_root.clone()
    };
    Ok(ProjectRoots {
        module_root,
        build_root,
    })
}

fn resolve_configured_module(
    profile: &LaunchProfile,
    task_root: &Path,
    configured: &str,
) -> Option<PathBuf> {
    let configured = configured.trim().trim_start_matches(':');
    if configured.is_empty() {
        return None;
    }
    let configured_path = Path::new(configured);
    let candidates = [
        PathBuf::from(configured),
        Path::new(&profile.project_root).join(configured_path),
        task_root.join(configured_path),
    ];
    candidates
        .into_iter()
        .map(|candidate| candidate.canonicalize().unwrap_or(candidate))
        .find(|candidate| candidate.is_dir() && has_build_file(candidate))
        .or_else(|| find_module_by_name(Path::new(&profile.project_root), configured))
}

fn find_module_by_name(root: &Path, name: &str) -> Option<PathBuf> {
    let mut found = Vec::new();
    walk_dirs(root, &mut |directory| {
        if directory
            .file_name()
            .is_some_and(|value| value.to_string_lossy() == name)
            && has_build_file(directory)
        {
            found.push(directory.to_path_buf());
        }
    });
    found.into_iter().next()
}

fn nearest_build_root(start: &Path, tool: &LaunchBuildTool) -> Option<PathBuf> {
    ancestors(start).into_iter().find(|directory| match tool {
        LaunchBuildTool::Maven => directory.join("pom.xml").is_file(),
        LaunchBuildTool::Gradle => {
            directory.join("build.gradle").is_file()
                || directory.join("build.gradle.kts").is_file()
                || directory.join("settings.gradle").is_file()
                || directory.join("settings.gradle.kts").is_file()
        }
    })
}

fn find_maven_aggregator(module_root: &Path) -> Option<PathBuf> {
    let mut result = None;
    for ancestor in ancestors(module_root) {
        let pom = ancestor.join("pom.xml");
        if !pom.is_file() {
            continue;
        }
        if ancestor == module_root || pom_declares_module(&pom, module_root) {
            result = Some(ancestor);
        }
    }
    result
}

fn find_gradle_root(module_root: &Path) -> Option<PathBuf> {
    let mut result = None;
    for ancestor in ancestors(module_root) {
        if ancestor.join("settings.gradle").is_file()
            || ancestor.join("settings.gradle.kts").is_file()
        {
            result = Some(ancestor);
        }
    }
    result
}

fn pom_declares_module(pom: &Path, module_root: &Path) -> bool {
    let Some(parent) = pom.parent() else {
        return false;
    };
    let Ok(contents) = fs::read_to_string(pom) else {
        return false;
    };
    xml_tag_values(&contents, "module")
        .into_iter()
        .any(|module| {
            let candidate = parent.join(&module);
            let path = candidate.canonicalize().unwrap_or(candidate);
            path == module_root || module_root.starts_with(&path)
        })
}

fn xml_tag_values(contents: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = contents;
    while let Some(start) = rest.find(&open) {
        let body = &rest[start + open.len()..];
        let Some(end) = body.find(&close) else {
            break;
        };
        let value = body[..end].trim();
        if !value.is_empty() {
            values.push(value.to_owned());
        }
        rest = &body[end + close.len()..];
    }
    values
}

fn has_build_file(directory: &Path) -> bool {
    [
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .any(|file| directory.join(file).is_file())
}

fn module_name(build_root: &Path, module_root: &Path, configured: Option<&str>) -> Option<String> {
    if let Some(module) = configured {
        let module = module.trim().trim_start_matches(':').replace('\\', "/");
        if !module.is_empty() {
            return Some(module);
        }
    }
    let relative = module_root.strip_prefix(build_root).ok()?;
    let value = relative.to_string_lossy().replace('\\', "/");
    (!value.is_empty()).then_some(value)
}

fn launcher(
    tool: &LaunchBuildTool,
    build_root: &Path,
    module_root: &Path,
) -> Result<(OsString, Vec<OsString>), String> {
    let (wrapper, fallback) = match tool {
        LaunchBuildTool::Maven => ("mvnw", "mvn"),
        LaunchBuildTool::Gradle => ("gradlew", "gradle"),
    };
    for candidate in [build_root.join(wrapper), module_root.join(wrapper)] {
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        if !is_executable(&candidate) {
            return Ok((OsString::from("sh"), vec![candidate.into_os_string()]));
        }
        return Ok((candidate.into_os_string(), Vec::new()));
    }
    Ok((resolve_fallback_tool(fallback), Vec::new()))
}

#[cfg(unix)]
fn resolve_fallback_tool(name: &str) -> OsString {
    let mut shell = shell_command(&format!("command -v {name}"));
    let Ok(output) = shell
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return OsString::from(name);
    };
    let candidate = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from);
    candidate
        .filter(|path| path.is_absolute() && path.is_file())
        .map(Into::into)
        .unwrap_or_else(|| OsString::from(name))
}

#[cfg(not(unix))]
fn resolve_fallback_tool(name: &str) -> OsString {
    OsString::from(name)
}

/// Build the command that should own a prepared Spring task. A raw IDE launch command is useful
/// as a fallback, but replaying its `-cp` value is exactly what lets an old `common` jar survive a
/// source edit. Maven's exec plugin expands `%classpath` from the selected module at runtime;
/// Gradle's BootRun task does the same from the project model.
fn project_native_launch(
    plan: &PreparePlan,
    task: &LaunchTask,
    spec: &LaunchPrepareSpec,
) -> Option<PreparedLaunch> {
    let (program, mut args) = launcher(&plan.tool, &plan.build_root, &plan.module_root).ok()?;
    match plan.tool {
        LaunchBuildTool::Maven => {
            let main_class = spec.main_class.as_deref()?.trim();
            if main_class.is_empty() {
                return None;
            }
            let details = java_launch_details(&task.command);
            let java_executable = details
                .as_ref()
                .map(|details| details.executable.clone())
                .unwrap_or_else(|| OsString::from("java"));
            let mut exec_args = details
                .as_ref()
                .map(|details| details.jvm_args.clone())
                .unwrap_or_default();
            exec_args.push("-classpath".into());
            exec_args.push("%classpath".into());
            if !spec.profiles.is_empty() {
                exec_args.push(format!(
                    "-Dspring.profiles.active={}",
                    spec.profiles.join(",")
                ));
            }
            exec_args.push(main_class.into());
            if let Some(details) = details {
                exec_args.extend(details.application_args);
            }

            args.extend([
                OsString::from("compile"),
                OsString::from(EXEC_MAVEN_PLUGIN),
                OsString::from(format!(
                    "-Dexec.executable={}",
                    java_executable.to_string_lossy()
                )),
                OsString::from("-Dexec.classpathScope=runtime"),
                OsString::from(format!("-Dexec.args={}", join_exec_args(&exec_args))),
            ]);
            Some(PreparedLaunch {
                program,
                args,
                cwd: plan.module_root.clone(),
                environment: plan.environment.clone(),
                java_home: plan.java_home.clone(),
            })
        }
        LaunchBuildTool::Gradle => {
            let task_name = plan
                .module
                .as_deref()
                .map(|module| format!(":{}:bootRun", module.replace(['/', '\\'], ":")))
                .unwrap_or_else(|| "bootRun".into());
            args.push(task_name.into());
            if !spec.profiles.is_empty() {
                args.push(
                    format!(
                        "--args=--spring.profiles.active={}",
                        spec.profiles.join(",")
                    )
                    .into(),
                );
            }
            Some(PreparedLaunch {
                program,
                args,
                // Gradle discovers settings.gradle from the invocation directory, so a
                // multi-module build must run from the root even when the task's cwd is a module.
                cwd: plan.build_root.clone(),
                environment: plan.environment.clone(),
                java_home: plan.java_home.clone(),
            })
        }
    }
}

fn join_exec_args(arguments: &[String]) -> String {
    arguments
        .iter()
        .map(|argument| {
            if argument
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '\'' | '"'))
            {
                format!(
                    "\"{}\"",
                    argument.replace('\\', "\\\\").replace('"', "\\\"")
                )
            } else {
                argument.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone)]
struct JavaLaunchDetails {
    executable: OsString,
    jvm_args: Vec<String>,
    application_args: Vec<String>,
    main_class: Option<String>,
}

fn java_launch_details(command: &str) -> Option<JavaLaunchDetails> {
    let tokens = command_tokens(command);
    let java_index = java_command_index(&tokens)?;
    let executable = OsString::from(&tokens[java_index]);
    let mut jvm_args = Vec::new();
    let mut index = java_index + 1;
    let mut main_class = None;
    while index < tokens.len() {
        let token = &tokens[index];
        if matches!(
            token.as_str(),
            "-cp"
                | "-classpath"
                | "--class-path"
                | "-p"
                | "--module-path"
                | "--upgrade-module-path"
        ) {
            index = (index + 2).min(tokens.len());
            continue;
        }
        if matches!(token.as_str(), "-jar" | "--module" | "-m") {
            // A jar/module launch has no stable application class for exec-maven-plugin. The
            // executable and JDK are still useful to a caller that only needs preparation.
            break;
        }
        if token.starts_with('-') {
            jvm_args.push(token.clone());
            if java_option_takes_value(token) && index + 1 < tokens.len() {
                jvm_args.push(tokens[index + 1].clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        main_class = Some(token.clone());
        index += 1;
        break;
    }
    let application_args = if main_class.is_some() {
        tokens[index..].to_vec()
    } else {
        Vec::new()
    };
    Some(JavaLaunchDetails {
        executable,
        jvm_args,
        application_args,
        main_class,
    })
}

fn command_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut started = false;
    let mut characters = command.chars().peekable();

    while let Some(character) = characters.next() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' && !in_single_quote {
            if characters
                .peek()
                .is_some_and(|next| next.is_whitespace() || matches!(next, '\\' | '\'' | '"'))
            {
                escaped = true;
            } else {
                // Preserve ordinary backslashes so quoted Windows paths are not corrupted when
                // a saved command is inspected on another platform.
                current.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                started = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                started = true;
            }
            character if character.is_whitespace() && !in_single_quote && !in_double_quote => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            character => {
                current.push(character);
                started = true;
            }
        }
    }
    if escaped {
        current.push('\\');
    }
    if started {
        tokens.push(current);
    }
    tokens
}

fn java_command_index(tokens: &[String]) -> Option<usize> {
    tokens.iter().position(|token| {
        Path::new(token)
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "java" | "java.exe"))
    })
}

fn command_environment(command: &str) -> Vec<(OsString, OsString)> {
    let tokens = command_tokens(command);
    let Some(java_index) = java_command_index(&tokens) else {
        return Vec::new();
    };

    let mut assignments = Vec::new();
    let mut env_command_seen = false;
    for token in &tokens[..java_index] {
        if is_env_command(token) && !env_command_seen {
            env_command_seen = true;
            continue;
        }
        let Some((name, value)) = environment_assignment(token) else {
            // Only a contiguous assignment prefix is safe to replay without a shell. An
            // unsupported env option or wrapper command must not cause later arbitrary tokens to
            // be reinterpreted as environment variables.
            return Vec::new();
        };
        if let Some(existing) = assignments.iter_mut().find(|(key, _)| key == &name) {
            existing.1 = value;
        } else {
            assignments.push((name, value));
        }
    }
    assignments
}

fn environment_assignment(token: &str) -> Option<(OsString, OsString)> {
    let (name, value) = token.split_once('=')?;
    if !is_environment_name(name) {
        return None;
    }
    Some((name.into(), value.into()))
}

fn is_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn is_env_command(token: &str) -> bool {
    Path::new(token)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "env" | "env.exe"))
}

fn command_active_profiles(command: &str) -> Vec<String> {
    command_environment(command)
        .into_iter()
        .find(|(name, _)| name == "SPRING_PROFILES_ACTIVE")
        .map(|(_, value)| {
            value
                .to_string_lossy()
                .split(',')
                .map(str::trim)
                .filter(|profile| !profile.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn java_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-agentlib"
            | "-javaagent"
            | "-splash"
            | "--add-exports"
            | "--add-opens"
            | "--add-reads"
            | "--patch-module"
            | "--limit-modules"
            | "--add-modules"
    )
}

fn java_home_for_plan(command: &str, build_root: &Path, tool: &LaunchBuildTool) -> Option<PathBuf> {
    let tokens = command_tokens(command);
    let java_index = tokens.iter().position(|token| {
        Path::new(token)
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "java" | "java.exe"))
    });
    let from_command = java_index
        .and_then(|index| java_home_for(&tokens, index, Path::new(&tokens[index])))
        .or_else(|| {
            tokens.iter().find_map(|token| {
                let value = token.strip_prefix("JAVA_HOME=")?;
                let home = PathBuf::from(value.trim_matches(['\'', '"']));
                home.is_dir().then_some(home)
            })
        });
    if from_command.is_some() {
        return from_command;
    }
    matches!(tool, LaunchBuildTool::Maven)
        .then(|| required_java_home(build_root))
        .flatten()
}

fn java_home_for(tokens: &[String], java_index: usize, executable: &Path) -> Option<PathBuf> {
    if let Some(home) = tokens[..java_index].iter().find_map(|token| {
        let value = token.strip_prefix("JAVA_HOME=")?;
        let home = PathBuf::from(value.trim_matches(['\'', '"']));
        home.is_dir().then_some(home)
    }) {
        return Some(home);
    }
    let executable = if executable.is_absolute() {
        executable.to_path_buf()
    } else {
        return None;
    };
    let bin = executable.parent()?;
    if bin.file_name() == Some(OsStr::new("bin")) {
        bin.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn required_java_home(build_root: &Path) -> Option<PathBuf> {
    let version = maven_java_version(build_root)?;
    java_home_for_version(&version, configured_java8_home(), &standard_java8_roots())
}

fn maven_java_version(build_root: &Path) -> Option<String> {
    ancestors(build_root).into_iter().find_map(|ancestor| {
        let contents = fs::read_to_string(ancestor.join("pom.xml")).ok()?;
        [
            "java.version",
            "maven.compiler.release",
            "maven.compiler.source",
            "maven.compiler.target",
        ]
        .into_iter()
        .find_map(|tag| xml_tag_values(&contents, tag).into_iter().next())
    })
}

fn is_java8_version(version: &str) -> bool {
    let version = version.trim();
    version == "8" || version.starts_with("8.") || version == "1.8" || version.starts_with("1.8.")
}

fn configured_java8_home() -> Option<PathBuf> {
    ["CUTTING_BOARD_JAVA8_HOME", "KICE_JAVA8_HOME"]
        .into_iter()
        .find_map(|name| {
            let home = PathBuf::from(env::var_os(name)?);
            is_jdk_home(&home).then_some(home)
        })
}

fn discover_java8_home_from_roots(roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .find_map(|root| discover_java8_home_under(root))
}

fn standard_java8_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![home.join(".sdkman/candidates/java"), home.join(".jdks")]
}

fn java_home_for_version(
    version: &str,
    configured: Option<PathBuf>,
    search_roots: &[PathBuf],
) -> Option<PathBuf> {
    if !is_java8_version(version) {
        return None;
    }
    configured.or_else(|| discover_java8_home_from_roots(search_roots))
}

fn discover_java8_home_under(parent: &Path) -> Option<PathBuf> {
    let mut candidates = fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
            (name.starts_with("8.") || name.contains("1.8")) && is_jdk_home(path)
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.pop()
}

fn is_jdk_home(home: &Path) -> bool {
    home.join("bin/java").is_file() || home.join("bin/java.exe").is_file()
}

fn infer_main_class(profile: &LaunchProfile, task: &LaunchTask) -> Option<String> {
    if let Some(main_class) =
        java_launch_details(&task.command).and_then(|details| details.main_class)
    {
        return Some(main_class);
    }
    for ancestor in ancestors(&task_root(profile, task)) {
        for source_root in [
            ancestor.join("src/main/java"),
            ancestor.join("src/main/kotlin"),
        ] {
            if let Some(main_class) = find_spring_main_class(&source_root) {
                return Some(main_class);
            }
        }
    }
    None
}

fn find_spring_main_class(directory: &Path) -> Option<String> {
    let mut entries = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_spring_main_class(&path) {
                return Some(found);
            }
            continue;
        }
        if !matches!(
            path.extension().and_then(OsStr::to_str),
            Some("java" | "kt")
        ) {
            continue;
        }
        let contents = fs::read_to_string(&path).ok()?;
        if !contents.contains("@SpringBootApplication") {
            continue;
        }
        let class_name = contents.lines().find_map(|line| {
            let name = line
                .split_once("class ")?
                .1
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .next()?;
            (!name.is_empty()).then_some(name.to_owned())
        })?;
        let package = contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("package "))
            .map(|value| value.trim().trim_end_matches(';'))
            .filter(|value| !value.is_empty());
        return Some(package.map_or(class_name.clone(), |value| format!("{value}.{class_name}")));
    }
    None
}

pub(super) fn apply_java_home(command: &mut Command, java_home: Option<&Path>) {
    let Some(java_home) = java_home else {
        return;
    };
    command.env("JAVA_HOME", java_home);
    let mut path = java_home.join("bin").into_os_string();
    if let Some(existing) = env::var_os("PATH") {
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(existing);
    }
    command.env("PATH", path);
}

pub(super) fn apply_environment(command: &mut Command, environment: &[(OsString, OsString)]) {
    command.envs(environment.iter().map(|(name, value)| (name, value)));
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

fn run_plan(plan: &PreparePlan, log_path: &Path, task: &LaunchTask) -> Result<(), String> {
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|error| format!("Could not open {}: {error}", log_path.display()))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| format!("Could not duplicate {}: {error}", log_path.display()))?;
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    apply_java_home(&mut command, plan.java_home.as_deref());
    apply_environment(&mut command, &plan.environment);
    let status = command.status().map_err(|error| {
        format!(
            "Could not prepare {} with {}: {error}",
            task.name,
            command_display(plan)
        )
    })?;
    if !status.success() {
        return Err(format!(
            "Could not prepare {}: {} exited with {status}.",
            task.name,
            command_display(plan)
        ));
    }
    Ok(())
}

pub(super) fn append_log(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    writeln!(file, "{line}").map_err(|error| format!("Could not write {}: {error}", path.display()))
}

fn command_display(plan: &PreparePlan) -> String {
    let mut parts = vec![plan.program.to_string_lossy().into_owned()];
    parts.extend(
        plan.args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned()),
    );
    parts.join(" ")
}

fn cache_key(plan: &PreparePlan, task: &LaunchTask, spec: &LaunchPrepareSpec) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{:?}",
        plan.tool_name(),
        plan.build_root.display(),
        plan.module_root.display(),
        task.command,
        task.cwd,
        spec
    )
}

fn fingerprint(
    plan: &PreparePlan,
    task: &LaunchTask,
    spec: &LaunchPrepareSpec,
) -> Result<String, String> {
    let mut hasher = Sha1::new();
    hasher.update(plan.tool_name().as_bytes());
    hasher.update(plan.build_root.to_string_lossy().as_bytes());
    hasher.update(plan.module_root.to_string_lossy().as_bytes());
    hasher.update(task.command.as_bytes());
    hasher.update(task.cwd.as_bytes());
    hasher.update(format!("{spec:?}").as_bytes());
    for input in &plan.inputs {
        hasher.update(input.to_string_lossy().as_bytes());
        let contents = fs::read(input).map_err(|error| {
            format!("Could not read prepare input {}: {error}", input.display())
        })?;
        hasher.update(contents);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

impl PreparePlan {
    fn tool_name(&self) -> &'static str {
        match self.tool {
            LaunchBuildTool::Maven => "maven",
            LaunchBuildTool::Gradle => "gradle",
        }
    }
}

fn relevant_inputs(root: &Path) -> Vec<PathBuf> {
    let mut inputs = Vec::new();
    walk_files(root, &mut |path| {
        if is_relevant_input(path) {
            inputs.push(path.to_path_buf());
        }
    });
    inputs.sort();
    inputs
}

fn is_relevant_input(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    if matches!(
        name,
        "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "gradle.properties"
            | "mvnw"
            | "mvnw.cmd"
            | "gradlew"
            | "gradlew.bat"
            | "maven.config"
            | "jvm.config"
    ) {
        return true;
    }
    path.components()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|parts| {
            parts[0].as_os_str() == OsStr::new("src") && parts[1].as_os_str() == OsStr::new("main")
        })
        || path.components().any(|part| {
            part.as_os_str() == OsStr::new("gradle") || part.as_os_str() == OsStr::new(".mvn")
        })
}

fn walk_files(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if is_generated_or_hidden(&name) {
            continue;
        }
        if path.is_dir() {
            walk_files(&path, visit);
        } else if path.is_file() {
            visit(&path);
        }
    }
}

fn walk_dirs(root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if is_generated_or_hidden(&name) || !path.is_dir() {
            continue;
        }
        visit(&path);
        walk_dirs(&path, visit);
    }
}

fn is_generated_or_hidden(name: &OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".gradle" | "target" | "build" | "out" | "node_modules" | "dist")
    ) || (name.to_string_lossy().starts_with('.') && name != OsStr::new(".mvn"))
}

fn ancestors(start: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut current = Some(start);
    for _ in 0..MAX_ANCESTORS {
        let Some(path) = current else {
            break;
        };
        result.push(path.to_path_buf());
        current = path.parent();
    }
    result
}

fn task_root(profile: &LaunchProfile, task: &LaunchTask) -> PathBuf {
    let cwd = PathBuf::from(task.cwd.trim());
    if cwd.is_absolute() {
        cwd
    } else {
        Path::new(&profile.project_root).join(cwd)
    }
}

fn command_identity(command: &str) -> Vec<String> {
    let mut tokens = command.split_whitespace();
    let first = tokens.find(|token| {
        !token.starts_with('-')
            && !token.contains('=')
            && !matches!(*token, "env" | "nohup" | "exec" | "sudo")
    });
    let Some(first) = first else {
        return Vec::new();
    };
    vec![Path::new(first)
        .file_name()
        .unwrap_or_else(|| OsStr::new(first))
        .to_string_lossy()
        .to_lowercase()]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{
        fs,
        process::{Command, Stdio},
    };

    fn profile(root: &Path, command: &str, prepare: Option<LaunchPrepareSpec>) -> LaunchProfile {
        LaunchProfile {
            id: "profile".into(),
            name: "profile".into(),
            project_root: root.to_string_lossy().into_owned(),
            tasks: vec![LaunchTask {
                name: "api".into(),
                cwd: ".".into(),
                command: command.into(),
                expected_port: Some(8080),
                container: None,
                prepare,
            }],
        }
    }

    fn spring_spec(tool: LaunchBuildTool) -> LaunchPrepareSpec {
        LaunchPrepareSpec {
            kind: LaunchPrepareKind::SpringBoot,
            build_tool: Some(tool),
            module: None,
            profiles: Vec::new(),
            main_class: None,
        }
    }

    #[test]
    fn command_environment_preserves_only_a_leading_assignment_prefix() {
        let command = r#"env R_HOME="/opt/R with spaces" LD_LIBRARY_PATH='/opt/lib' SPRING_PROFILES_ACTIVE=dev java -cp app.jar App"#;
        assert_eq!(
            command_environment(command),
            vec![
                ("R_HOME".into(), "/opt/R with spaces".into()),
                ("LD_LIBRARY_PATH".into(), "/opt/lib".into()),
                ("SPRING_PROFILES_ACTIVE".into(), "dev".into()),
            ]
        );
        assert_eq!(
            command_environment("R_HOME=/opt/R SPRING_PROFILES_ACTIVE=dev java -cp app.jar App"),
            vec![
                ("R_HOME".into(), "/opt/R".into()),
                ("SPRING_PROFILES_ACTIVE".into(), "dev".into()),
            ]
        );
        assert!(command_environment("env -i R_HOME=/opt/R java -cp app.jar App").is_empty());
        assert!(command_environment("wrapper --set R_HOME=/opt/R java -cp app.jar App").is_empty());
    }

    #[test]
    fn command_tokens_preserve_windows_path_separators() {
        assert_eq!(
            command_tokens(r#""C:\Program Files\Java\bin\java.exe" -cp app.jar App"#)[0],
            r#"C:\Program Files\Java\bin\java.exe"#
        );
    }

    #[test]
    fn command_environment_derives_spring_profiles() {
        assert_eq!(
            command_active_profiles("env SPRING_PROFILES_ACTIVE=dev,test java -cp app.jar App"),
            vec!["dev", "test"]
        );
    }

    #[cfg(unix)]
    fn executable_script(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn gradle_prepare_and_native_launch_receive_saved_environment() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("adm");
        fs::create_dir(&module).unwrap();
        fs::write(root.join("settings.gradle"), "include ':adm'\n").unwrap();
        fs::write(module.join("build.gradle"), "plugins {}\n").unwrap();
        fs::create_dir_all(module.join("src/main/java")).unwrap();
        fs::write(module.join("src/main/java/App.java"), "class App {}\n").unwrap();
        executable_script(
            &root.join("gradlew"),
            "#!/bin/sh\nprintf '%s|%s|%s\n' \"$SPRING_PROFILES_ACTIVE\" \"$R_HOME\" \"$LD_LIBRARY_PATH\" >> env-log\nexit 0\n",
        );

        let r_home = root.join("r-home");
        let library_path = root.join("r-libs");
        let command = format!(
            "env R_HOME={} LD_LIBRARY_PATH={} SPRING_PROFILES_ACTIVE=dev java -cp stale.jar App",
            r_home.display(),
            library_path.display()
        );
        let mut spec = spring_spec(LaunchBuildTool::Gradle);
        spec.module = Some("adm".into());
        let mut profile = profile(root, &command, Some(spec));
        profile.tasks[0].cwd = "adm".into();
        let logs = root.join("logs");
        let log = logs.join("api.log");

        let result = prepare_task(
            &mut PrepareCache::default(),
            &profile,
            &profile.tasks[0],
            &logs,
            &log,
        )
        .unwrap()
        .unwrap();
        let native = result.launch.as_ref().expect("Gradle native launch");
        assert_eq!(
            native.environment,
            vec![
                ("R_HOME".into(), r_home.clone().into_os_string()),
                (
                    "LD_LIBRARY_PATH".into(),
                    library_path.clone().into_os_string()
                ),
                ("SPRING_PROFILES_ACTIVE".into(), "dev".into()),
            ]
        );
        assert!(native
            .args
            .iter()
            .any(|argument| argument == "--args=--spring.profiles.active=dev"));
        assert_eq!(
            fs::read_to_string(root.join("env-log")).unwrap(),
            format!("dev|{}|{}\n", r_home.display(), library_path.display())
        );
    }

    #[cfg(unix)]
    #[test]
    fn maven_prepare_uses_reactor_and_cache_skips_unchanged_inputs() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("common")).unwrap();
        fs::create_dir(root.join("adm")).unwrap();
        fs::write(
            root.join("pom.xml"),
            "<project><modules><module>common</module><module>adm</module></modules></project>",
        )
        .unwrap();
        fs::write(root.join("common/pom.xml"), "<project/>").unwrap();
        fs::write(root.join("adm/pom.xml"), "<project/>").unwrap();
        fs::create_dir_all(root.join("adm/src/main/java")).unwrap();
        fs::write(root.join("adm/src/main/java/App.java"), "class App {}\n").unwrap();
        executable_script(
            &root.join("mvnw"),
            "#!/bin/sh\nprintf '%s\\n' invoked >> prepare-count\nexit 0\n",
        );

        let mut spec = spring_spec(LaunchBuildTool::Maven);
        spec.module = Some("adm".into());
        let mut profile = profile(root, "java -cp spring-boot.jar App", Some(spec));
        profile.tasks[0].cwd = "adm".into();
        let logs = root.join("logs");
        let log = logs.join("api.log");
        let mut cache = PrepareCache::default();

        let first = prepare_task(&mut cache, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(!first.skipped);
        let output = fs::read_to_string(&log).unwrap();
        assert!(output.contains("-pl adm -am install -Dmaven.test.skip=true"));
        let native = first.launch.as_ref().expect("Maven native launch");
        let native_args = native
            .args
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(native_args
            .iter()
            .any(|argument| argument == "org.codehaus.mojo:exec-maven-plugin:3.5.1:exec"));
        assert!(native_args
            .iter()
            .any(|argument| argument.contains("-classpath %classpath")));
        assert!(!native_args
            .iter()
            .any(|argument| argument.contains("spring-boot.jar")));
        assert_eq!(
            fs::read_to_string(root.join("prepare-count"))
                .unwrap()
                .lines()
                .count(),
            1
        );

        let second = prepare_task(&mut cache, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(second.skipped);
        assert_eq!(
            fs::read_to_string(root.join("prepare-count"))
                .unwrap()
                .lines()
                .count(),
            1
        );

        fs::write(
            root.join("adm/src/main/java/App.java"),
            "class App { int changed; }\n",
        )
        .unwrap();
        let third = prepare_task(&mut cache, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(!third.skipped);
        assert_eq!(
            fs::read_to_string(root.join("prepare-count"))
                .unwrap()
                .lines()
                .count(),
            2
        );

        fs::create_dir(root.join(".mvn")).unwrap();
        fs::write(root.join(".mvn/maven.config"), "-q\n").unwrap();
        let fourth = prepare_task(&mut cache, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(!fourth.skipped);
        assert_eq!(
            fs::read_to_string(root.join("prepare-count"))
                .unwrap()
                .lines()
                .count(),
            3
        );
    }

    #[cfg(unix)]
    #[test]
    fn gradle_prepare_uses_module_assemble_and_cache_survives_manager_reload() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("adm");
        fs::create_dir(&module).unwrap();
        fs::write(root.join("settings.gradle"), "include ':adm'\n").unwrap();
        fs::write(module.join("build.gradle"), "plugins {}\n").unwrap();
        fs::create_dir_all(module.join("src/main/java")).unwrap();
        fs::write(module.join("src/main/java/App.java"), "class App {}\n").unwrap();
        executable_script(
            &root.join("gradlew"),
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> prepare-count\nexit 0\n",
        );

        let mut spec = spring_spec(LaunchBuildTool::Gradle);
        spec.module = Some("adm".into());
        let mut profile = profile(root, "./gradlew bootRun", Some(spec));
        profile.tasks[0].cwd = "adm".into();
        let logs = root.join("logs");
        let log = logs.join("api.log");
        let mut cache = PrepareCache::default();
        let first = prepare_task(&mut cache, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(!first.skipped);
        assert!(fs::read_to_string(root.join("prepare-count"))
            .unwrap()
            .contains(":adm:assemble"));
        let native = first.launch.as_ref().expect("Gradle native launch");
        assert!(native
            .args
            .iter()
            .any(|argument| argument == ":adm:bootRun"));

        let mut reloaded = PrepareCache::default();
        let second = prepare_task(&mut reloaded, &profile, &profile.tasks[0], &logs, &log)
            .unwrap()
            .unwrap();
        assert!(second.skipped);
        assert_eq!(
            fs::read_to_string(root.join("prepare-count"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn old_tasks_without_metadata_keep_shell_fallback() {
        let temporary = tempfile::tempdir().unwrap();
        let profile = profile(temporary.path(), "echo ok", None);
        let plan = resolve_spec(&profile, &profile.tasks[0]).unwrap();
        assert_eq!(plan, None);
    }

    #[test]
    fn prepare_metadata_round_trips_with_defaults() {
        let spec = spring_spec(LaunchBuildTool::Maven);
        let encoded = serde_json::to_string(&spec).unwrap();
        let decoded: LaunchPrepareSpec = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, spec);
        let task: LaunchTask = serde_json::from_str(
            r#"{"name":"api","cwd":".","command":"java App","expected_port":8080}"#,
        )
        .unwrap();
        assert_eq!(task.prepare, None);
    }

    #[test]
    fn maven_module_parser_finds_nested_reactor_module() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let module = root.join("services/adm");
        fs::create_dir_all(&module).unwrap();
        let pom = root.join("pom.xml");
        fs::write(
            &pom,
            "<project><modules><module>services/adm</module></modules></project>",
        )
        .unwrap();
        assert!(pom_declares_module(&pom, &module.canonicalize().unwrap()));
    }

    #[test]
    fn java_version_parser_recognizes_java_eight_forms() {
        for version in ["8", "8.0.392", "1.8", "1.8.0_392"] {
            assert!(is_java8_version(version), "{version}");
        }
        for version in ["11", "17", "21", "1.9"] {
            assert!(!is_java8_version(version), "{version}");
        }
    }

    #[test]
    fn java_eight_discovery_prefers_the_newest_valid_sdk_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let candidates = temporary.path().join("java");
        for version in ["8.0.302", "8.0.402", "17.0.1"] {
            fs::create_dir_all(candidates.join(version).join("bin")).unwrap();
            fs::write(candidates.join(version).join("bin/java"), "").unwrap();
        }

        let found = discover_java8_home_from_roots(&[candidates]);

        assert_eq!(found, Some(temporary.path().join("java/8.0.402")));
    }

    #[test]
    fn java_eight_pom_selection_applies_the_selected_home_to_build_tools() {
        let temporary = tempfile::tempdir().unwrap();
        fs::write(
            temporary.path().join("pom.xml"),
            "<project><properties><java.version>1.8</java.version></properties></project>",
        )
        .unwrap();
        let candidates = temporary.path().join("sdkman/java");
        let configured = candidates.join("8.0.402");
        fs::create_dir_all(configured.join("bin")).unwrap();
        fs::write(configured.join("bin/java"), "").unwrap();
        let version = maven_java_version(temporary.path()).unwrap();
        let selected = java_home_for_version(&version, None, &[candidates]).unwrap();

        let mut command = Command::new("sh");
        command.args(["-c", "printf '%s' \"$JAVA_HOME\""]);
        apply_java_home(&mut command, Some(&selected));
        let output = command.output().unwrap();

        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            configured.to_string_lossy()
        );
    }

    #[cfg(unix)]
    #[test]
    fn fallback_build_tool_resolution_uses_a_login_shell_path() {
        let path = resolve_fallback_tool("sh");
        let path = PathBuf::from(path);
        assert!(path.is_absolute());
        assert!(path.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn real_java_caller_fails_with_stale_common_jar_then_succeeds_after_prepare() {
        if Command::new("java")
            .arg("-version")
            .stderr(Stdio::null())
            .status()
            .is_err()
            || Command::new("javac")
                .arg("-version")
                .stderr(Stdio::null())
                .status()
                .is_err()
        {
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let v1 = root.join("v1");
        let v2 = root.join("v2");
        let caller = root.join("caller");
        fs::create_dir_all(v1.join("kr/re/kisti/idr/model")).unwrap();
        fs::create_dir_all(v2.join("kr/re/kisti/idr/model")).unwrap();
        fs::create_dir_all(caller.join("kr/re/kisti/idr/aip")).unwrap();
        fs::create_dir_all(caller.join("classes")).unwrap();
        fs::write(
            v1.join("kr/re/kisti/idr/model/SubmissionView.java"),
            "package kr.re.kisti.idr.model; public class SubmissionView { }\n",
        )
        .unwrap();
        fs::write(
            v2.join("kr/re/kisti/idr/model/SubmissionView.java"),
            "package kr.re.kisti.idr.model; public class SubmissionView { public void setSecurity(String value) {} }\n",
        )
        .unwrap();
        fs::write(
            caller.join("kr/re/kisti/idr/aip/ModelWrapperService.java"),
            "package kr.re.kisti.idr.aip; import kr.re.kisti.idr.model.SubmissionView; public class ModelWrapperService { public static void main(String[] a) { new SubmissionView().setSecurity(\"ok\"); System.out.println(\"ok\"); } }\n",
        )
        .unwrap();
        for source_root in [&v1, &v2] {
            let status = Command::new("javac")
                .arg("-d")
                .arg(source_root.join("classes"))
                .arg(source_root.join("kr/re/kisti/idr/model/SubmissionView.java"))
                .status()
                .unwrap();
            assert!(status.success());
            let status = Command::new("jar")
                .args(["cf"])
                .arg(source_root.join("common.jar"))
                .arg("-C")
                .arg(source_root.join("classes"))
                .arg(".")
                .status()
                .unwrap();
            assert!(status.success());
        }
        let status = Command::new("javac")
            .arg("-cp")
            .arg(v2.join("common.jar"))
            .arg("-d")
            .arg(caller.join("classes"))
            .arg(caller.join("kr/re/kisti/idr/aip/ModelWrapperService.java"))
            .status()
            .unwrap();
        assert!(status.success());
        let runtime_jar = root.join("common-runtime.jar");
        fs::copy(v1.join("common.jar"), &runtime_jar).unwrap();
        let stale = Command::new("java")
            .arg("-cp")
            .arg(format!(
                "{}:{}",
                caller.join("classes").display(),
                runtime_jar.display()
            ))
            .arg("kr.re.kisti.idr.aip.ModelWrapperService")
            .output()
            .unwrap();
        assert!(!stale.status.success());
        assert!(String::from_utf8_lossy(&stale.stderr).contains("NoSuchMethodError"));

        fs::write(
            root.join("pom.xml"),
            "<project><modules><module>adm</module></modules></project>",
        )
        .unwrap();
        fs::create_dir(root.join("adm")).unwrap();
        fs::write(root.join("adm/pom.xml"), "<project/>").unwrap();
        executable_script(
            &root.join("mvnw"),
            &format!(
                "#!/bin/sh\ncp '{}' '{}'\n",
                v2.join("common.jar").display(),
                runtime_jar.display()
            ),
        );
        let mut spec = spring_spec(LaunchBuildTool::Maven);
        spec.module = Some("adm".into());
        let mut profile = profile(
            root,
            &format!(
                "java -cp {}:{} kr.re.kisti.idr.aip.ModelWrapperService",
                caller.join("classes").display(),
                runtime_jar.display()
            ),
            Some(spec),
        );
        profile.tasks[0].cwd = "adm".into();
        let mut cache = PrepareCache::default();
        prepare_task(
            &mut cache,
            &profile,
            &profile.tasks[0],
            &root.join("logs"),
            &root.join("logs/api.log"),
        )
        .unwrap();
        let fresh = Command::new("java")
            .arg("-cp")
            .arg(format!(
                "{}:{}",
                caller.join("classes").display(),
                runtime_jar.display()
            ))
            .arg("kr.re.kisti.idr.aip.ModelWrapperService")
            .output()
            .unwrap();
        assert!(
            fresh.status.success(),
            "{}",
            String::from_utf8_lossy(&fresh.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&fresh.stdout).trim(), "ok");
    }
}
