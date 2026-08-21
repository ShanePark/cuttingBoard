use crate::models::{
    now_epoch, Endpoint, ProcessInfo, ProjectInfo, ServiceIdentity, ServiceSnapshot,
    WorkspaceSnapshot,
};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Instant,
};
use sysinfo::{Pid, System};

#[derive(Debug, Clone)]
struct ListenerRecord {
    pid: u32,
    uid: Option<u32>,
    process_name: String,
    endpoint: Endpoint,
}

#[derive(Debug)]
struct Classification {
    tech: String,
    category: String,
    relevance: String,
}

#[derive(Debug)]
struct SpringSettings {
    port: Option<u16>,
    management_port: Option<u16>,
    livereload_port: u16,
    context_path: String,
    ssl_enabled: bool,
}

impl Default for SpringSettings {
    fn default() -> Self {
        Self {
            port: None,
            management_port: None,
            livereload_port: 35_729,
            context_path: String::new(),
            ssl_enabled: false,
        }
    }
}

pub fn scan_workspace(
    demo: bool,
) -> Result<(WorkspaceSnapshot, HashMap<String, ServiceIdentity>), String> {
    if demo {
        let snapshot = demo_snapshot();
        let index = snapshot
            .services
            .iter()
            .filter_map(|service| {
                service.process.as_ref().map(|process| {
                    (
                        service.id.clone(),
                        ServiceIdentity {
                            pid: process.pid,
                            start_time: process.create_time,
                            uid: process.uid,
                            display_name: service.display_name.clone(),
                        },
                    )
                })
            })
            .collect();
        return Ok((snapshot, index));
    }

    let started = Instant::now();
    let current_uid = current_uid();
    let mut errors = Vec::new();
    let records = match listeners_from_lsof() {
        Ok(records) => records,
        Err(lsof_error) => match listeners_from_ss(current_uid) {
            Ok(records) => {
                errors.push(format!("lsof unavailable; using ss: {lsof_error}"));
                records
            }
            Err(ss_error) => {
                return Err(format!(
                    "Could not inspect TCP listeners. lsof: {lsof_error}; ss: {ss_error}"
                ));
            }
        },
    };
    let endpoint_count = records.len();
    let mut grouped: BTreeMap<u32, Vec<ListenerRecord>> = BTreeMap::new();
    for record in records {
        grouped.entry(record.pid).or_default().push(record);
    }

    let system = System::new_all();
    let own_pid = std::process::id();
    let mut services = Vec::new();
    let mut index = HashMap::new();

    for (pid, mut listeners) in grouped {
        listeners.sort_by_key(|record| (record.endpoint.port, record.endpoint.address.clone()));
        listeners.dedup_by(|left, right| left.endpoint == right.endpoint);
        let process = system.process(Pid::from_u32(pid));
        let process_name = process
            .map(|item| item.name().to_string_lossy().into_owned())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| listeners[0].process_name.clone());
        let args = process
            .map(|item| {
                item.cmd()
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let environment = process
            .map(|item| {
                item.environ()
                    .iter()
                    .map(|value| value.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let executable = process
            .and_then(|item| item.exe())
            .map(|path| path.to_string_lossy().into_owned());
        let cwd = process
            .and_then(|item| item.cwd())
            .map(Path::to_path_buf)
            .or_else(|| inferred_existing_path(&args));
        let project = detect_project(cwd.as_deref(), &args);
        let observed_ports = listeners
            .iter()
            .map(|record| record.endpoint.port)
            .collect::<Vec<_>>();
        let classification = classify(
            &process_name,
            executable.as_deref(),
            &args,
            &observed_ports,
            project.as_ref(),
        );
        let uid = listeners.iter().find_map(|record| record.uid);

        if classification.relevance == "noise" || (uid.is_some() && current_uid.is_some() && uid != current_uid) {
            continue;
        }

        let spring_settings = if classification.tech == "spring" {
            let settings = resolve_spring_settings(
                &args,
                &environment,
                cwd.as_deref(),
                project.as_ref().map(|value| Path::new(&value.root_path)),
            );
            if !refine_spring_listeners(&mut listeners, &settings) {
                continue;
            }
            Some(settings)
        } else {
            None
        };
        let ports = listeners
            .iter()
            .map(|record| record.endpoint.port)
            .collect::<Vec<_>>();

        let start_time = process.map(|item| item.start_time()).unwrap_or(0);
        let id = service_id(pid, start_time, &ports);
        let command = redact_command(&args, &process_name);
        let process_info = process.map(|item| ProcessInfo {
            pid,
            parent_pid: item.parent().map(|value| value.as_u32()),
            name: process_name.clone(),
            executable: executable.clone(),
            working_directory: cwd.as_ref().map(|path| path.to_string_lossy().into_owned()),
            command,
            create_time: item.start_time(),
            uptime_seconds: item.run_time(),
            cpu_percent: Some(item.cpu_usage()),
            memory_bytes: Some(item.memory()),
            uid,
        });
        let endpoints = listeners
            .into_iter()
            .map(|record| record.endpoint)
            .collect::<Vec<_>>();
        let (origin_kind, origin_label) = origin_for(process, &system);
        let mut warnings = Vec::new();
        if process.is_none() {
            warnings.push("Process details were unavailable during this scan.".into());
        }
        if endpoints.iter().any(|endpoint| endpoint.scope == "wildcard") {
            warnings.push("This listener is reachable on every network interface.".into());
        }
        let can_terminate = process.is_some()
            && pid > 1
            && pid != own_pid
            && (uid.is_none() || current_uid.is_none() || uid == current_uid)
            && classification.relevance == "dev";
        let display_name = display_name(&process_name, project.as_ref(), &classification.tech);
        let browser_url = browser_url(
            &classification.category,
            &endpoints,
            &args,
            spring_settings.as_ref(),
        );
        let service = ServiceSnapshot {
            id: id.clone(),
            display_name: display_name.clone(),
            tech: classification.tech,
            category: classification.category,
            relevance: classification.relevance,
            endpoints,
            process: process_info,
            project,
            status: if warnings.is_empty() { "healthy".into() } else { "limited".into() },
            warnings,
            origin_kind,
            origin_label,
            can_terminate,
            browser_url,
        };
        if can_terminate {
            index.insert(
                id,
                ServiceIdentity {
                    pid,
                    start_time,
                    uid,
                    display_name,
                },
            );
        }
        services.push(service);
    }

    services.sort_by(|left, right| {
        let left_path = left.project.as_ref().map(|item| item.root_path.as_str()).unwrap_or("~");
        let right_path = right.project.as_ref().map(|item| item.root_path.as_str()).unwrap_or("~");
        left_path
            .cmp(right_path)
            .then_with(|| lowest_port(left).cmp(&lowest_port(right)))
            .then_with(|| left.display_name.to_lowercase().cmp(&right.display_name.to_lowercase()))
    });

    Ok((
        WorkspaceSnapshot {
            services,
            scanned_at: now_epoch(),
            scan_duration_ms: started.elapsed().as_millis(),
            endpoint_count,
            errors,
        },
        index,
    ))
}

fn lowest_port(service: &ServiceSnapshot) -> u16 {
    service.endpoints.iter().map(|endpoint| endpoint.port).min().unwrap_or(u16::MAX)
}

fn listeners_from_lsof() -> Result<Vec<ListenerRecord>, String> {
    let output = Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-FpcuPn"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() && output.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pid = None;
    let mut uid = None;
    let mut process_name = String::new();
    let mut protocol = String::new();
    let mut records = Vec::new();
    for line in text.lines() {
        let Some((field, value)) = line.split_at_checked(1) else { continue };
        match field {
            "p" => {
                pid = value.parse::<u32>().ok();
                uid = None;
                process_name.clear();
                protocol.clear();
            }
            "c" => process_name = value.to_string(),
            "u" => uid = value.parse::<u32>().ok(),
            "P" => protocol = value.to_string(),
            "n" if protocol.eq_ignore_ascii_case("TCP") || protocol.is_empty() => {
                if let (Some(pid), Some(endpoint)) = (pid, parse_endpoint(value)) {
                    records.push(ListenerRecord {
                        pid,
                        uid,
                        process_name: process_name.clone(),
                        endpoint,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(records)
}

fn listeners_from_ss(uid: Option<u32>) -> Result<Vec<ListenerRecord>, String> {
    let output = Command::new("ss")
        .args(["-H", "-ltnp"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut records = Vec::new();
    for line in text.lines() {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        if columns.len() < 4 {
            continue;
        }
        let Some(endpoint) = parse_endpoint(columns[3]) else { continue };
        let Some(pid) = parse_pid_from_ss(line) else { continue };
        let process_name = parse_name_from_ss(line).unwrap_or_else(|| "process".into());
        records.push(ListenerRecord { pid, uid, process_name, endpoint });
    }
    Ok(records)
}

fn parse_pid_from_ss(line: &str) -> Option<u32> {
    let start = line.find("pid=")? + 4;
    let digits = line[start..].chars().take_while(char::is_ascii_digit).collect::<String>();
    digits.parse().ok()
}

fn parse_name_from_ss(line: &str) -> Option<String> {
    let marker = "((\"";
    let start = line.find(marker)? + marker.len();
    let end = line[start..].find('"')? + start;
    Some(line[start..end].to_string())
}

fn parse_endpoint(value: &str) -> Option<Endpoint> {
    let value = value.trim().trim_start_matches("TCP ");
    let local = value.split("->").next()?.trim();
    let (address, port_text) = if let Some(rest) = local.strip_prefix('[') {
        let end = rest.find(']')?;
        let address = &rest[..end];
        let port = rest[end + 1..].strip_prefix(':')?;
        (address, port)
    } else {
        local.rsplit_once(':')?
    };
    let port = port_text.trim_matches('*').parse::<u16>().ok()?;
    let address = match address.trim() {
        "" | "*" => "0.0.0.0".to_string(),
        value => value.trim_matches(['[', ']']).to_string(),
    };
    let family = if address.contains(':') { "IPv6" } else { "IPv4" };
    let scope = endpoint_scope(&address);
    Some(Endpoint {
        family: family.into(),
        address,
        port,
        scope,
        protocol: "TCP".into(),
    })
}

fn endpoint_scope(address: &str) -> String {
    if matches!(address, "0.0.0.0" | "::" | "*") {
        return "wildcard".into();
    }
    match IpAddr::from_str(address) {
        Ok(ip) if ip.is_loopback() => "loopback".into(),
        Ok(ip) if ip.is_unspecified() => "wildcard".into(),
        Ok(_) => "lan".into(),
        Err(_) if address.eq_ignore_ascii_case("localhost") => "loopback".into(),
        Err(_) => "lan".into(),
    }
}

fn current_uid() -> Option<u32> {
    let output = Command::new("id").arg("-u").output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn classify(
    process_name: &str,
    executable: Option<&str>,
    args: &[String],
    ports: &[u16],
    project: Option<&ProjectInfo>,
) -> Classification {
    let identity = process_identity(process_name, executable, args);
    let command = args.join(" ").to_lowercase();
    let searchable = format!("{} {}", identity.join(" "), command);

    if contains_any(&searchable, &["docker-proxy", "com.docker", "vpnkit", "containerd-shim", "podman machine"]) {
        return Classification { tech: "docker".into(), category: "runtime".into(), relevance: "container".into() };
    }
    if is_desktop_or_system_noise(&identity, &command) || is_build_daemon(&identity, args) {
        return Classification { tech: "generic".into(), category: "other".into(), relevance: "noise".into() };
    }

    if contains_any(&searchable, &[
        "launchd", "systemd", "windowserver", "controlcenter", "coreservices", "sharingd",
        "rapportd", "identityservicesd", "airportd", "bluetoothd", "cupsd", "avahi-daemon",
        "gnome-shell", "kdeconnectd", "notificationcenter", "distnoted", "trustd", "opendirectoryd",
    ]) {
        return Classification { tech: "generic".into(), category: "other".into(), relevance: "noise".into() };
    }

    let daemon_mappings: &[(&[&str], &str, &str)] = &[
        (&["postgres", "postmaster"], "postgresql", "database"),
        (&["mariadbd"], "mariadb", "database"),
        (&["mysqld", "mysql"], "mysql", "database"),
        (&["mongod", "mongodb"], "mongodb", "database"),
        (&["redis-server", "redis"], "redis", "cache"),
        (&["memcached"], "memcached", "cache"),
        (&["elasticsearch"], "elasticsearch", "database"),
        (&["rabbitmq"], "rabbitmq", "cache"),
        (&["nginx"], "nginx", "proxy"),
        (&["caddy"], "caddy", "proxy"),
        (&["traefik"], "traefik", "proxy"),
        (&["ollama"], "ollama", "runtime"),
        (&["ssh", "sshd"], "ssh", "proxy"),
    ];
    for (needles, tech, category) in daemon_mappings {
        if identity.iter().any(|term| needles.iter().any(|needle| term == needle)) {
            return Classification { tech: (*tech).into(), category: (*category).into(), relevance: "dev".into() };
        }
    }

    if identity.iter().any(|term| matches!(term.as_str(), "java" | "java.exe"))
        && project.is_some_and(project_has_spring_evidence)
    {
        return Classification {
            tech: "spring".into(),
            category: "api".into(),
            relevance: "dev".into(),
        };
    }

    let mappings: &[(&[&str], &str, &str)] = &[
        (&["spring boot", "spring-boot", "bootrun"], "spring", "api"),
        (&["next dev", "next-server", "nextjs"], "nextjs", "web"),
        (&["vite", "vite.js"], "vite", "web"),
        (&["nuxt"], "nuxt", "web"),
        (&["astro"], "astro", "web"),
        (&["remix"], "remix", "web"),
        (&["webpack-dev-server", "react-scripts"], "nodejs", "web"),
        (&["storybook", "start-storybook"], "storybook", "web"),
        (&["svelte-kit", "sveltekit"], "svelte", "web"),
        (&["angular", "ng serve"], "angular", "web"),
        (&["django", "manage.py runserver"], "django", "web"),
        (&["fastapi", "uvicorn"], "fastapi", "api"),
        (&["gunicorn", "hypercorn", "daphne"], "python", "api"),
        (&["flask"], "flask", "web"),
        (&["rails", "puma"], "rails", "web"),
        (&["artisan"], "laravel", "api"),
        (&["jupyter-lab", "jupyter-notebook"], "jupyter", "runtime"),
        (&["quarkus"], "java", "api"),
        (&["micronaut"], "java", "api"),
        (&["catalina", "tomcat"], "tomcat", "api"),
        (&["solr"], "solr", "database"),
        (&["kafka"], "kafka", "runtime"),
        (&["deno"], "deno", "runtime"),
        (&["bun"], "bun", "runtime"),
        (&["cargo run", "target/debug", "target/release"], "rust", "runtime"),
        (&["go run", "/go/bin/"], "go", "runtime"),
    ];
    for (needles, tech, category) in mappings {
        if contains_any_term(&searchable, needles) {
            return Classification { tech: (*tech).into(), category: (*category).into(), relevance: "dev".into() };
        }
    }

    let common_dev_port = ports.iter().any(|port| {
        matches!(
            port,
            3000 | 3001 | 4000 | 4173 | 4200 | 4321 | 5000 | 5173 | 5432 | 6379 | 8000
                | 8001 | 8080 | 8081 | 8443 | 8888 | 9000 | 9090 | 9200 | 27017
        )
    });
    let tech = runtime_tech(&identity).unwrap_or("generic");
    let system_executable = executable.is_some_and(|value| {
        ["/usr/lib", "/usr/libexec", "/usr/share", "/usr/sbin", "/opt", "/snap",
            "/var/lib/flatpak", "/var/lib/snapd"]
            .iter()
            .any(|prefix| value.starts_with(prefix))
    });
    let is_dev = !system_executable && (project.is_some() || common_dev_port);
    Classification {
        tech: tech.into(),
        category: if common_dev_port { "web".into() } else { "other".into() },
        relevance: if is_dev { "dev".into() } else { "noise".into() },
    }
}

fn process_identity(process_name: &str, executable: Option<&str>, args: &[String]) -> Vec<String> {
    let mut identity = vec![process_name.to_lowercase()];
    if let Some(name) = executable.and_then(|value| Path::new(value).file_name()) {
        identity.push(name.to_string_lossy().to_lowercase());
    }
    for argument in args.iter().skip(1).filter(|value| !value.starts_with('-')).take(2) {
        if let Some(name) = Path::new(argument).file_name() {
            identity.push(name.to_string_lossy().to_lowercase());
        }
    }
    identity
}

fn runtime_tech(identity: &[String]) -> Option<&'static str> {
    for name in identity {
        let tech = match name.as_str() {
            "node" | "nodejs" => "nodejs",
            "python" | "python3" => "python",
            "java" | "java.exe" => "java",
            "dotnet" => "dotnet",
            "ruby" => "ruby",
            "php" => "php",
            "go" => "go",
            "cargo" => "rust",
            "deno" => "deno",
            "bun" => "bun",
            _ => continue,
        };
        return Some(tech);
    }
    None
}

fn is_desktop_or_system_noise(identity: &[String], command: &str) -> bool {
    const DESKTOP_APPS: &[&str] = &[
        "ulauncher", "kdeconnectd", "kdeconnect-indicator", "dropbox", "insync", "nextcloud",
        "megasync", "syncthing", "spotify", "steam", "steamwebhelper", "discord", "slack",
        "telegram-desktop", "signal-desktop", "element-desktop", "zoom", "teams", "skype",
        "github-desktop", "jetbrains-toolbox", "anydesk", "teamviewer", "rustdesk", "barrier",
        "synergy", "obs", "kdeinit5", "plasmashell", "gnome-shell", "evolution", "thunderbird",
        "firefox", "chrome", "chromium", "brave", "vivaldi", "opera", "transmission",
        "qbittorrent", "deluge", "vlc", "kodi", "warp", "1password", "bitwarden", "keepassxc",
    ];
    const IDE_MARKERS: &[&str] = &[
        "com.intellij.idea.main", "org.jetbrains.jps.cmdline.launcher", "kotlincompiledaemon",
        "daemonmavencli", "org.mvndaemon.mvnd.daemon.server",
    ];
    identity.iter().any(|name| DESKTOP_APPS.contains(&name.as_str()))
        || contains_any(command, IDE_MARKERS)
}

fn is_build_daemon(identity: &[String], args: &[String]) -> bool {
    if identity.iter().any(|name| matches!(name.as_str(), "mvnd" | "mvnd.exe")) {
        return true;
    }
    const MAIN_CLASSES: &[&str] = &[
        "org.gradle.launcher.daemon.bootstrap.gradledaemon",
        "org.gradle.process.internal.worker.gradleworkermain",
        "org.jetbrains.kotlin.daemon.kotlincompiledaemon",
        "org.apache.maven.cli.daemonmavencli",
        "org.mvndaemon.mvnd.daemon.server",
        "com.facebook.nailgun.ngserver",
        "com.martiansoftware.nailgun.ngserver",
    ];
    args.iter().any(|argument| {
        let lowered = argument.to_lowercase();
        if MAIN_CLASSES.contains(&lowered.as_str()) {
            return true;
        }
        lowered.split([':', ';']).any(|entry| {
            let name = entry.rsplit(['/', '\\']).next().unwrap_or(entry);
            name.ends_with(".jar")
                && (name.starts_with("gradle-daemon-main") || name.starts_with("gradle-worker"))
        })
    })
}

fn contains_any_term(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        text.match_indices(needle).any(|(start, matched)| {
            let before = text[..start].chars().next_back();
            let after = text[start + matched.len()..].chars().next();
            !before.is_some_and(|value| value.is_ascii_alphanumeric())
                && !after.is_some_and(|value| value.is_ascii_alphanumeric())
        })
    })
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn project_has_spring_evidence(project: &ProjectInfo) -> bool {
    let root = Path::new(&project.root_path);
    for build_file in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        let path = root.join(build_file);
        let Ok(metadata) = path.metadata() else { continue };
        if !metadata.is_file() || metadata.len() > 1_000_000 {
            continue;
        }
        if fs::read_to_string(path).ok().is_some_and(|text| {
            let text = text.to_lowercase();
            contains_any(&text, &["spring-boot", "org.springframework.boot"])
        }) {
            return true;
        }
    }

    spring_config_locations(Some(root), Some(root))
        .into_iter()
        .filter_map(|location| fs::read_dir(location).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let supported = matches!(
                path.extension().and_then(|value| value.to_str()),
                Some("properties" | "yml" | "yaml")
            );
            name.starts_with("application") && supported
        })
        .any(|path| {
            read_spring_config(&path, &[]).keys().any(|key| {
                key.starts_with("spring.")
                    || key.starts_with("server.")
                    || key.starts_with("management.")
            })
        })
}

fn detect_project(cwd: Option<&Path>, args: &[String]) -> Option<ProjectInfo> {
    let mut candidates = Vec::new();
    if let Some(cwd) = cwd {
        candidates.push(cwd.to_path_buf());
    }
    for argument in args {
        let value = argument
            .split_once('=')
            .map(|(_, right)| right)
            .unwrap_or(argument)
            .trim_matches(['\'', '"']);
        if looks_like_absolute_path(value) {
            let path = PathBuf::from(value);
            if path.exists() {
                candidates.push(if path.is_file() { path.parent()?.to_path_buf() } else { path });
            }
        }
    }
    let mut seen = HashSet::new();
    let mut best: Option<(PathBuf, &'static str)> = None;
    for candidate in candidates {
        let canonical = candidate.canonicalize().unwrap_or(candidate);
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if let Some((root, source)) = project_root(&canonical) {
            let replace = best
                .as_ref()
                .map_or(true, |(current, _)| {
                    root.components().count() > current.components().count()
                });
            if replace {
                best = Some((root, source));
            }
        }
    }
    let (root, source) = best?;
    let name = project_name(&root, source).unwrap_or_else(|| {
        root.file_name().unwrap_or_default().to_string_lossy().into_owned()
    });
    let workspace_root = workspace_root(&root).unwrap_or_else(|| root.clone());
    let workspace_name = workspace_root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut hasher = Sha1::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let id = format!("project-{:x}", hasher.finalize());
    Some(ProjectInfo {
        id: id.chars().take(24).collect(),
        name,
        root_path: root.to_string_lossy().into_owned(),
        detection_source: source.into(),
        workspace_root_path: workspace_root.to_string_lossy().into_owned(),
        workspace_name,
    })
}

fn looks_like_absolute_path(value: &str) -> bool {
    Path::new(value).is_absolute()
        || value.starts_with("\\\\")
        || value
            .as_bytes()
            .get(1..3)
            .is_some_and(|separator| separator == b":\\" || separator == b":/")
}

fn project_root(start: &Path) -> Option<(PathBuf, &'static str)> {
    const MARKERS: &[(&str, &str)] = &[
        ("pnpm-workspace.yaml", "pnpm-workspace.yaml"),
        ("package.json", "package.json"),
        ("pyproject.toml", "pyproject.toml"),
        ("Cargo.toml", "Cargo.toml"),
        ("go.mod", "go.mod"),
        ("pom.xml", "pom.xml"),
        ("build.gradle.kts", "build.gradle.kts"),
        ("build.gradle", "build.gradle"),
        ("settings.gradle.kts", "settings.gradle.kts"),
        ("settings.gradle", "settings.gradle"),
        ("docker-compose.yml", "docker-compose.yml"),
        ("docker-compose.yaml", "docker-compose.yaml"),
        ("compose.yml", "compose.yml"),
        ("compose.yaml", "compose.yaml"),
        (".git", "git"),
    ];
    let mut current = Some(start);
    for _ in 0..12 {
        let directory = current?;
        if is_excluded_project_root(directory) {
            break;
        }
        for (marker, source) in MARKERS {
            if directory.join(marker).exists() {
                return Some((directory.to_path_buf(), source));
            }
        }
        current = directory.parent();
    }
    None
}

fn workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    for _ in 0..12 {
        let directory = current?;
        if is_excluded_project_root(directory) {
            break;
        }
        if directory.join(".git").exists() {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

fn is_excluded_project_root(path: &Path) -> bool {
    const ROOTS: &[&str] = &["/", "/tmp", "/var/tmp", "/usr", "/opt", "/etc"];
    if ROOTS.iter().any(|root| path == Path::new(root)) {
        return true;
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.canonicalize().unwrap_or(home))
        .is_some_and(|home| path == home)
}

fn project_name(root: &Path, source: &str) -> Option<String> {
    match source {
        "package.json" => {
            let value: serde_json::Value = serde_json::from_slice(&fs::read(root.join(source)).ok()?).ok()?;
            value.get("name")?.as_str().map(clean_project_name)
        }
        "Cargo.toml" | "pyproject.toml" | "build.gradle" | "build.gradle.kts" => {
            named_assignment(&fs::read_to_string(root.join(source)).ok()?)
        }
        "go.mod" => fs::read_to_string(root.join(source)).ok()?.lines().find_map(|line| {
            line.trim().strip_prefix("module ").map(|value| clean_project_name(value.rsplit('/').next().unwrap_or(value)))
        }),
        "pom.xml" => xml_tag(&fs::read_to_string(root.join(source)).ok()?, "artifactId")
            .map(|value| clean_project_name(&value)),
        _ => None,
    }
}

fn named_assignment(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        let value = trimmed
            .strip_prefix("name =")
            .or_else(|| trimmed.strip_prefix("rootProject.name ="))?;
        Some(clean_project_name(value.trim().trim_matches(['\'', '"'])))
    })
}

fn xml_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim().to_string())
}

fn clean_project_name(value: &str) -> String {
    value.trim().trim_start_matches('@').replace('/', " · ")
}

fn inferred_existing_path(args: &[String]) -> Option<PathBuf> {
    args.iter().find_map(|argument| {
        let value = argument.trim_matches(['\'', '"']);
        let path = PathBuf::from(value);
        if !path.is_absolute() || !path.exists() {
            return None;
        }
        Some(if path.is_dir() { path } else { path.parent()?.to_path_buf() })
    })
}

fn origin_for(process: Option<&sysinfo::Process>, system: &System) -> (String, Option<String>) {
    let mut current = process;
    for _ in 0..10 {
        let Some(item) = current else { break };
        let name = item.name().to_string_lossy().to_lowercase();
        let command = item
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let text = format!("{name} {command}");
        if contains_any(&text, &["codex", "claude", "aider", "cursor-agent", "copilot-agent"]) {
            let label = if text.contains("claude") { "Claude Code" } else if text.contains("aider") { "Aider" } else { "Agent" };
            return ("agent".into(), Some(label.into()));
        }
        let ide = [
            ("visual studio code", "VS Code"), ("code helper", "VS Code"), ("cursor", "Cursor"),
            ("intellij", "IntelliJ IDEA"), ("pycharm", "PyCharm"), ("webstorm", "WebStorm"),
            ("android studio", "Android Studio"), ("zed", "Zed"),
        ];
        if let Some((_, label)) = ide.iter().find(|(needle, _)| text.contains(needle)) {
            return ("ide".into(), Some((*label).into()));
        }
        let terminals = [
            ("iterm", "iTerm2"), ("wezterm", "WezTerm"), ("alacritty", "Alacritty"),
            ("kitty", "Kitty"), ("ghostty", "Ghostty"), ("terminal.app", "Terminal"),
            ("gnome-terminal", "Terminal"), ("konsole", "Konsole"),
        ];
        if let Some((_, label)) = terminals.iter().find(|(needle, _)| text.contains(needle)) {
            return ("terminal".into(), Some((*label).into()));
        }
        current = item.parent().and_then(|pid| system.process(pid));
    }
    if process.and_then(|item| item.parent()).map(Pid::as_u32) == Some(1) {
        ("system".into(), Some("System".into()))
    } else {
        ("unknown".into(), None)
    }
}

fn display_name(process_name: &str, project: Option<&ProjectInfo>, tech: &str) -> String {
    if let Some(project) = project {
        if tech == "generic" || project.name.to_lowercase().contains(tech) {
            return project.name.clone();
        }
        return format!("{} · {}", project.name, friendly_tech(tech));
    }
    let value = process_name.trim();
    if value.is_empty() { friendly_tech(tech).into() } else { value.into() }
}

fn friendly_tech(tech: &str) -> &str {
    match tech {
        "nodejs" => "Node.js", "nextjs" => "Next.js", "postgresql" => "PostgreSQL",
        "mongodb" => "MongoDB", "dotnet" => ".NET", "spring" => "Spring Boot",
        "generic" => "Service", value => value,
    }
}

fn browser_url(
    category: &str,
    endpoints: &[Endpoint],
    args: &[String],
    spring: Option<&SpringSettings>,
) -> Option<String> {
    if !matches!(category, "web" | "api" | "proxy" | "runtime") {
        return None;
    }
    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.scope == "loopback")
        .or_else(|| endpoints.iter().find(|endpoint| endpoint.scope == "wildcard"))
        .or_else(|| endpoints.first())?;
    let scheme = if spring.is_some_and(|settings| settings.ssl_enabled)
        || matches!(endpoint.port, 443 | 8443 | 9443)
        || args.iter().any(|arg| arg.to_lowercase().contains("https")) {
        "https"
    } else {
        "http"
    };
    let host = match endpoint.scope.as_str() {
        "loopback" | "wildcard" => "localhost".into(),
        _ if endpoint.address.contains(':') => format!("[{}]", endpoint.address),
        _ => endpoint.address.clone(),
    };
    let context = spring
        .map(|settings| settings.context_path.clone())
        .or_else(|| spring_context_path(args))
        .unwrap_or_default();
    Some(format!("{scheme}://{host}:{}{context}", endpoint.port))
}

fn refine_spring_listeners(
    listeners: &mut Vec<ListenerRecord>,
    settings: &SpringSettings,
) -> bool {
    if let Some(port) = settings.port {
        if listeners.iter().any(|record| record.endpoint.port == port) {
            listeners.retain(|record| record.endpoint.port == port);
            return true;
        }
    }

    let management_port = settings.management_port.filter(|port| Some(*port) != settings.port);
    let livereload_port = (Some(settings.livereload_port) != settings.port)
        .then_some(settings.livereload_port);
    let observed = listeners.clone();
    listeners.retain(|record| {
        Some(record.endpoint.port) != livereload_port
            && Some(record.endpoint.port) != management_port
    });
    if listeners.is_empty() {
        *listeners = observed;
    }
    true
}

fn resolve_spring_settings(
    args: &[String],
    environment: &[String],
    cwd: Option<&Path>,
    project_root: Option<&Path>,
) -> SpringSettings {
    let environment = environment
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect::<HashMap<_, _>>();
    let command_properties = spring_command_properties(args);
    let mut environment_properties = HashMap::new();
    const ENVIRONMENT_KEYS: &[(&str, &str)] = &[
        ("SERVER_PORT", "server.port"),
        ("SERVER_SERVLET_CONTEXT_PATH", "server.servlet.context-path"),
        ("SPRING_PROFILES_ACTIVE", "spring.profiles.active"),
        ("SPRING_DEVTOOLS_LIVERELOAD_PORT", "spring.devtools.livereload.port"),
        ("MANAGEMENT_SERVER_PORT", "management.server.port"),
        ("SERVER_SSL_ENABLED", "server.ssl.enabled"),
    ];
    for (environment_key, property_key) in ENVIRONMENT_KEYS {
        if let Some(value) = environment.get(*environment_key) {
            environment_properties.insert((*property_key).to_string(), value.clone());
        }
    }

    let locations = spring_config_locations(cwd, project_root);
    let base_properties = load_spring_properties(&locations, &[]);
    let active_profiles = command_properties
        .get("spring.profiles.active")
        .or_else(|| environment_properties.get("spring.profiles.active"))
        .or_else(|| base_properties.get("spring.profiles.active"))
        .map(|value| spring_profiles(value))
        .unwrap_or_default();
    let mut properties = load_spring_properties(&locations, &active_profiles);
    properties.extend(environment_properties);
    properties.extend(command_properties);
    for value in properties.values_mut() {
        *value = resolve_spring_placeholder(value, &environment);
    }

    let mut settings = SpringSettings::default();
    settings.port = properties.get("server.port").and_then(|value| parse_port(value));
    settings.management_port = properties
        .get("management.server.port")
        .and_then(|value| parse_port(value));
    if let Some(port) = properties
        .get("spring.devtools.livereload.port")
        .and_then(|value| parse_port(value))
    {
        settings.livereload_port = port;
    }
    settings.ssl_enabled = properties
        .get("server.ssl.enabled")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    settings.context_path = normalize_context_path(
        properties
            .get("server.servlet.context-path")
            .or_else(|| properties.get("server.context-path"))
            .map(String::as_str)
            .unwrap_or_default(),
    );
    settings
}

fn spring_command_properties(args: &[String]) -> HashMap<String, String> {
    let mut tokens = Vec::new();
    for argument in args {
        tokens.push(argument.as_str());
        if let Some(nested) = argument.strip_prefix("--args=") {
            tokens.extend(nested.split_whitespace());
        }
    }
    let mut properties = HashMap::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        let option = token
            .strip_prefix("--")
            .or_else(|| token.strip_prefix("-D"));
        if let Some(option) = option {
            if let Some((name, value)) = option.split_once('=') {
                properties.insert(name.to_string(), value.trim_matches(['\'', '"']).to_string());
            } else if index + 1 < tokens.len() && !tokens[index + 1].starts_with('-') {
                properties.insert(option.to_string(), tokens[index + 1].to_string());
                index += 1;
            }
        }
        index += 1;
    }
    properties
}

fn spring_config_locations(cwd: Option<&Path>, project_root: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [project_root, cwd].into_iter().flatten() {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let mut locations = Vec::new();
    for root in roots {
        for location in [root.join("src/main/resources"), root.clone(), root.join("config")] {
            if !locations.contains(&location) {
                locations.push(location);
            }
        }
    }
    locations
}

fn load_spring_properties(
    locations: &[PathBuf],
    active_profiles: &[String],
) -> HashMap<String, String> {
    let mut properties = HashMap::new();
    for location in locations {
        for suffix in ["properties", "yml", "yaml"] {
            properties.extend(read_spring_config(
                &location.join(format!("application.{suffix}")),
                active_profiles,
            ));
        }
        for profile in active_profiles {
            for suffix in ["properties", "yml", "yaml"] {
                properties.extend(read_spring_config(
                    &location.join(format!("application-{profile}.{suffix}")),
                    active_profiles,
                ));
            }
        }
    }
    properties
}

fn read_spring_config(path: &Path, active_profiles: &[String]) -> HashMap<String, String> {
    let Ok(metadata) = path.metadata() else { return HashMap::new() };
    if !metadata.is_file() || metadata.len() > 1_000_000 {
        return HashMap::new();
    }
    let Ok(text) = fs::read_to_string(path) else { return HashMap::new() };
    if path.extension().is_some_and(|value| value == "properties") {
        parse_spring_properties(&text)
    } else {
        parse_spring_yaml(&text, active_profiles)
    }
}

fn parse_spring_properties(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', '!']) {
                return None;
            }
            let separator = line.find(['=', ':']).or_else(|| line.find(char::is_whitespace))?;
            let name = line[..separator].trim();
            let value = line[separator + 1..].trim();
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

fn parse_spring_yaml(text: &str, active_profiles: &[String]) -> HashMap<String, String> {
    let mut properties = HashMap::new();
    for document in spring_yaml_documents(text) {
        let document = flatten_spring_yaml(&document);
        let selector = document
            .get("spring.config.activate.on-profile")
            .or_else(|| document.get("spring.profiles"));
        if selector.is_some_and(|value| {
            !spring_profiles(value)
                .iter()
                .any(|profile| active_profiles.contains(profile))
        }) {
            continue;
        }
        properties.extend(document);
    }
    properties
}

fn spring_yaml_documents(text: &str) -> Vec<String> {
    let mut documents = vec![String::new()];
    for line in text.lines() {
        if line.trim_start().starts_with("---") {
            documents.push(String::new());
        } else if let Some(document) = documents.last_mut() {
            document.push_str(line);
            document.push('\n');
        }
    }
    documents
}

fn flatten_spring_yaml(text: &str) -> HashMap<String, String> {
    let mut properties = HashMap::new();
    let mut parents: Vec<(usize, String)> = Vec::new();
    for raw_line in text.lines() {
        let stripped = raw_line.trim_start_matches(' ');
        if stripped.is_empty() || stripped.starts_with('#') || !stripped.contains(':') {
            continue;
        }
        let indent = raw_line.len() - stripped.len();
        let Some((key, raw_value)) = stripped.split_once(':') else { continue };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        while parents.last().is_some_and(|(parent_indent, _)| *parent_indent >= indent) {
            parents.pop();
        }
        let raw_value = raw_value.trim();
        if raw_value.is_empty() {
            parents.push((indent, key.to_string()));
            continue;
        }
        let value = raw_value
            .split(" #")
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches(['\'', '"']);
        let name = parents
            .iter()
            .map(|(_, parent)| parent.as_str())
            .chain(std::iter::once(key))
            .collect::<Vec<_>>()
            .join(".");
        properties.insert(name, value.to_string());
    }
    properties
}

fn spring_profiles(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|profile| profile.trim().trim_matches(['\'', '"']).to_string())
        .filter(|profile| !profile.is_empty())
        .collect()
}

fn parse_port(value: &str) -> Option<u16> {
    value.trim().parse::<u16>().ok().filter(|port| *port > 0)
}

fn resolve_spring_placeholder(value: &str, environment: &HashMap<String, String>) -> String {
    let mut resolved = value.to_string();
    for _ in 0..8 {
        let Some(start) = resolved.find("${") else { break };
        let Some(relative_end) = resolved[start + 2..].find('}') else { break };
        let end = start + 2 + relative_end;
        let expression = &resolved[start + 2..end];
        let (name, default) = expression.split_once(':').unwrap_or((expression, ""));
        let replacement = environment.get(name).map(String::as_str).unwrap_or(default);
        resolved = format!("{}{}{}", &resolved[..start], replacement, &resolved[end + 1..]);
    }
    resolved
}

fn normalize_context_path(value: &str) -> String {
    let path = value.trim();
    if path.is_empty() || path == "/" {
        String::new()
    } else {
        format!("/{}/", path.trim_matches('/'))
    }
}

fn spring_context_path(args: &[String]) -> Option<String> {
    const KEYS: &[&str] = &[
        "--server.servlet.context-path=", "-Dserver.servlet.context-path=",
        "--server.context-path=", "SERVER_SERVLET_CONTEXT_PATH=",
    ];
    for argument in args {
        for key in KEYS {
            if let Some(value) = argument.strip_prefix(key) {
                let value = value.trim();
                if value.is_empty() || value == "/" {
                    return Some(String::new());
                }
                return Some(format!("/{}", value.trim_matches('/')));
            }
        }
    }
    None
}

fn redact_command(args: &[String], fallback: &str) -> String {
    if args.is_empty() {
        return fallback.to_string();
    }
    let mut result = Vec::new();
    let mut redact_next = false;
    for argument in args {
        if redact_next {
            result.push("•••".into());
            redact_next = false;
            continue;
        }
        let lower = argument.to_lowercase();
        let secret = ["password", "passwd", "secret", "token", "api-key", "apikey", "authorization", "credential"]
            .iter()
            .any(|needle| lower.contains(needle));
        if secret {
            if let Some((left, _)) = argument.split_once('=') {
                result.push(format!("{left}=•••"));
            } else {
                result.push(argument.clone());
                redact_next = true;
            }
        } else if argument.contains("://") && argument.contains('@') {
            result.push(redact_url(argument));
        } else {
            result.push(argument.clone());
        }
    }
    let joined = result.join(" ");
    if joined.chars().count() > 600 {
        format!("{}…", joined.chars().take(599).collect::<String>())
    } else {
        joined
    }
}

fn redact_url(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else { return value.into() };
    let authority_start = scheme_end + 3;
    let Some(at) = value[authority_start..].find('@').map(|index| index + authority_start) else { return value.into() };
    format!("{}•••@{}", &value[..authority_start], &value[at + 1..])
}

fn service_id(pid: u32, start_time: u64, ports: &[u16]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{pid}:{start_time}:{ports:?}").as_bytes());
    format!("service-{:x}", hasher.finalize()).chars().take(24).collect()
}

pub fn demo_snapshot() -> WorkspaceSnapshot {
    let now = now_epoch();
    let project = |id: &str, name: &str, path: &str| ProjectInfo {
        id: id.into(), name: name.into(), root_path: path.into(), detection_source: "package.json".into(),
        workspace_root_path: path.into(), workspace_name: name.into(),
    };
    let service = |id: &str, name: &str, tech: &str, category: &str, port: u16, path: &str, age: u64, origin_kind: &str, origin: &str| ServiceSnapshot {
        id: id.into(), display_name: name.into(), tech: tech.into(), category: category.into(), relevance: "dev".into(),
        endpoints: vec![Endpoint { family: "IPv4".into(), address: "127.0.0.1".into(), port, scope: "loopback".into(), protocol: "TCP".into() }],
        process: Some(ProcessInfo { pid: 20_000 + port as u32, parent_pid: Some(1000), name: tech.into(), executable: Some(format!("/usr/local/bin/{tech}")), working_directory: Some(path.into()), command: format!("{tech} run --port {port}"), create_time: now.saturating_sub(age), uptime_seconds: age, cpu_percent: Some(1.4), memory_bytes: Some(146_800_640), uid: current_uid() }),
        project: Some(project(&format!("project-{id}"), path.rsplit('/').next().unwrap_or(name), path)), status: "healthy".into(), warnings: vec![], origin_kind: origin_kind.into(), origin_label: Some(origin.into()), can_terminate: false,
        browser_url: Some(format!("http://localhost:{port}")),
    };
    let services = vec![
        service("demo-api", "cutting-board-api · Spring Boot", "spring", "api", 8080, "/Users/shane/Developer/cutting-board-api", 94, "ide", "IntelliJ IDEA"),
        service("demo-web", "cutting-board-web · Vite", "vite", "web", 5173, "/Users/shane/Developer/cutting-board-web", 3725, "agent", "Agent"),
        service("demo-db", "local-postgres", "postgresql", "database", 5432, "/Users/shane/Developer/local-stack", 86_400, "terminal", "iTerm2"),
        service("demo-cache", "local-redis", "redis", "cache", 6379, "/Users/shane/Developer/local-stack", 710, "terminal", "iTerm2"),
    ];
    WorkspaceSnapshot { endpoint_count: services.len(), services, scanned_at: now, scan_duration_ms: 18, errors: vec![] }
}

#[cfg(test)]
mod tests {
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
        let args = vec!["server".into(), "--token=abc".into(), "--password".into(), "value".into()];
        let value = redact_command(&args, "server");
        assert!(!value.contains("abc"));
        assert!(!value.contains("value"));
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
        let java = classify("java", Some("/usr/bin/java"), &["java".into()], &[33_357], None);
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
        let settings = resolve_spring_settings(&[], &[], Some(temporary.path()), Some(temporary.path()));
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
        assert_eq!(endpoints.iter().map(|endpoint| endpoint.port).collect::<Vec<_>>(), vec![48_080]);
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
    fn spring_yaml_only_applies_the_active_profile_document() {
        let properties = parse_spring_yaml(
            "server:\n  port: 8080\n---\nspring:\n  config:\n    activate:\n      on-profile: local\nserver:\n  port: 48080\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nserver:\n  port: 8443\n",
            &["local".into()],
        );
        assert_eq!(properties.get("server.port").map(String::as_str), Some("48080"));
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
        fs::write(inner.join("Cargo.toml"), "[package]\nname = \"inner\"\n").unwrap();

        let project = detect_project(Some(&inner.join("src")), &[]).unwrap();

        assert_eq!(Path::new(&project.workspace_root_path), inner);
        assert_eq!(project.workspace_name, "inner");
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

        let project = detect_project(
            Some(&workspace),
            &[classes.to_string_lossy().into_owned()],
        )
        .unwrap();

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
            &["ssh".into(), "-L".into(), "48983:localhost:8983".into(), "oasis".into()],
            &[48_983],
            Some(&project()),
        );
        assert_eq!(result.relevance, "dev");
        assert_eq!(result.tech, "ssh");
        assert_eq!(result.category, "proxy");
    }
}
