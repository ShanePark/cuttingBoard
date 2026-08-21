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
        let executable = process
            .and_then(|item| item.exe())
            .map(|path| path.to_string_lossy().into_owned());
        let cwd = process
            .and_then(|item| item.cwd())
            .map(Path::to_path_buf)
            .or_else(|| inferred_existing_path(&args));
        let project = detect_project(cwd.as_deref(), &args);
        let searchable = format!(
            "{} {} {} {}",
            process_name,
            executable.as_deref().unwrap_or_default(),
            args.join(" "),
            project.as_ref().map(|item| item.name.as_str()).unwrap_or_default()
        )
        .to_lowercase();
        let ports = listeners
            .iter()
            .map(|record| record.endpoint.port)
            .collect::<Vec<_>>();
        let classification = classify(&searchable, &ports, project.as_ref());
        let uid = listeners.iter().find_map(|record| record.uid);

        if classification.relevance == "noise" || (uid.is_some() && current_uid.is_some() && uid != current_uid) {
            continue;
        }

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
        let browser_url = browser_url(&classification.category, &endpoints, &args);
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

fn classify(text: &str, ports: &[u16], project: Option<&ProjectInfo>) -> Classification {
    if contains_any(text, &["docker-proxy", "com.docker", "vpnkit", "containerd-shim", "podman machine"]) {
        return Classification { tech: "docker".into(), category: "runtime".into(), relevance: "container".into() };
    }
    if contains_any(text, &[
        "launchd", "systemd", "windowserver", "controlcenter", "coreservices", "sharingd",
        "rapportd", "identityservicesd", "airportd", "bluetoothd", "cupsd", "avahi-daemon",
        "gnome-shell", "kdeconnectd", "notificationcenter", "distnoted", "trustd", "opendirectoryd",
    ]) {
        return Classification { tech: "generic".into(), category: "other".into(), relevance: "noise".into() };
    }

    let mappings: &[(&[&str], &str, &str)] = &[
        (&["spring boot", "spring-boot", "bootrun"], "spring", "api"),
        (&["next dev", "next-server", "nextjs"], "nextjs", "web"),
        (&["vite", "vite.js"], "vite", "web"),
        (&["nuxt"], "nuxt", "web"),
        (&["angular", "ng serve"], "angular", "web"),
        (&["django", "manage.py runserver"], "django", "web"),
        (&["fastapi", "uvicorn"], "fastapi", "api"),
        (&["flask"], "flask", "web"),
        (&["rails", "puma"], "rails", "web"),
        (&["postgres"], "postgresql", "database"),
        (&["mariadb"], "mariadb", "database"),
        (&["mysqld", "mysql"], "mysql", "database"),
        (&["mongod", "mongodb"], "mongodb", "database"),
        (&["redis-server", "redis"], "redis", "cache"),
        (&["memcached"], "memcached", "cache"),
        (&["elasticsearch"], "elasticsearch", "database"),
        (&["rabbitmq"], "rabbitmq", "cache"),
        (&["nginx"], "nginx", "proxy"),
        (&["caddy"], "caddy", "proxy"),
        (&["deno"], "deno", "runtime"),
        (&["bun"], "bun", "runtime"),
        (&["node", "npm", "pnpm", "yarn"], "nodejs", "runtime"),
        (&["python", "poetry"], "python", "runtime"),
        (&["java", "gradle", "maven"], "java", "runtime"),
        (&["dotnet"], "dotnet", "runtime"),
        (&["cargo run", "target/debug", "target/release"], "rust", "runtime"),
        (&["go run", "/go/bin/"], "go", "runtime"),
        (&["php"], "php", "runtime"),
        (&["ruby"], "ruby", "runtime"),
    ];
    for (needles, tech, category) in mappings {
        if contains_any(text, needles) {
            return Classification { tech: (*tech).into(), category: (*category).into(), relevance: "dev".into() };
        }
    }

    let common_dev_port = ports.iter().any(|port| {
        matches!(
            port,
            3000 | 3001 | 4000 | 4200 | 5000 | 5173 | 5432 | 6379 | 8000 | 8001 | 8080
                | 8081 | 8443 | 8888 | 9000 | 9090 | 9200 | 27017
        ) || (10_000..=49_999).contains(port)
    });
    Classification {
        tech: "generic".into(),
        category: if common_dev_port { "web".into() } else { "other".into() },
        relevance: if project.is_some() || common_dev_port { "dev".into() } else { "noise".into() },
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
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
        if value.starts_with('/') {
            let path = PathBuf::from(value);
            if path.exists() {
                candidates.push(if path.is_file() { path.parent()?.to_path_buf() } else { path });
            }
        }
    }
    let mut seen = HashSet::new();
    for candidate in candidates {
        let canonical = candidate.canonicalize().unwrap_or(candidate);
        if !seen.insert(canonical.clone()) {
            continue;
        }
        if let Some((root, source)) = project_root(&canonical) {
            let name = project_name(&root, source).unwrap_or_else(|| {
                root.file_name().unwrap_or_default().to_string_lossy().into_owned()
            });
            let mut hasher = Sha1::new();
            hasher.update(root.to_string_lossy().as_bytes());
            let id = format!("project-{:x}", hasher.finalize());
            return Some(ProjectInfo {
                id: id.chars().take(24).collect(),
                name,
                root_path: root.to_string_lossy().into_owned(),
                detection_source: source.into(),
            });
        }
    }
    None
}

fn project_root(start: &Path) -> Option<(PathBuf, &'static str)> {
    const MARKERS: &[(&str, &str)] = &[
        (".git", "git"),
        ("package.json", "package.json"),
        ("pyproject.toml", "pyproject.toml"),
        ("Cargo.toml", "Cargo.toml"),
        ("go.mod", "go.mod"),
        ("pom.xml", "pom.xml"),
        ("build.gradle.kts", "build.gradle.kts"),
        ("build.gradle", "build.gradle"),
        ("docker-compose.yml", "docker-compose.yml"),
        ("compose.yml", "compose.yml"),
    ];
    let mut current = Some(start);
    for _ in 0..12 {
        let directory = current?;
        for (marker, source) in MARKERS {
            if directory.join(marker).exists() {
                return Some((directory.to_path_buf(), source));
            }
        }
        current = directory.parent();
    }
    None
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
        "pom.xml" => xml_tag(&fs::read_to_string(root.join(source)).ok()?, "artifactId").map(clean_project_name),
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

fn browser_url(category: &str, endpoints: &[Endpoint], args: &[String]) -> Option<String> {
    if !matches!(category, "web" | "api" | "proxy" | "runtime") {
        return None;
    }
    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.scope == "loopback")
        .or_else(|| endpoints.iter().find(|endpoint| endpoint.scope == "wildcard"))
        .or_else(|| endpoints.first())?;
    let scheme = if matches!(endpoint.port, 443 | 8443) || args.iter().any(|arg| arg.to_lowercase().contains("https")) {
        "https"
    } else {
        "http"
    };
    let host = match endpoint.scope.as_str() {
        "loopback" | "wildcard" => "localhost".into(),
        _ if endpoint.address.contains(':') => format!("[{}]", endpoint.address),
        _ => endpoint.address.clone(),
    };
    let context = spring_context_path(args).unwrap_or_default();
    Some(format!("{scheme}://{host}:{}{context}", endpoint.port))
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
}
