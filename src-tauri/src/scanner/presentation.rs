use super::spring::{spring_context_path, SpringSettings};
use crate::models::{Endpoint, ProjectInfo, ServiceSnapshot};
use sha1::{Digest, Sha1};
use sysinfo::{Pid, Process, System};

pub(crate) fn lowest_port(service: &ServiceSnapshot) -> u16 {
    service
        .endpoints
        .iter()
        .map(|endpoint| endpoint.port)
        .min()
        .unwrap_or(u16::MAX)
}

pub(crate) fn origin_for(process: Option<&Process>, system: &System) -> (String, Option<String>) {
    let mut current = process;
    for _ in 0..10 {
        let Some(item) = current else {
            break;
        };
        let name = item.name().to_string_lossy().to_lowercase();
        let command = item
            .cmd()
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let text = format!("{name} {command}");
        let agents = [
            ("claude", "Claude Code"),
            ("codex", "Codex"),
            ("aider", "Aider"),
            ("cursor-agent", "Cursor"),
            ("copilot-agent", "GitHub Copilot"),
        ];
        if let Some((_, label)) = agents.iter().find(|(needle, _)| text.contains(needle)) {
            return ("agent".into(), Some((*label).into()));
        }
        let ide = [
            ("visual studio code", "VS Code"),
            ("code helper", "VS Code"),
            ("cursor", "Cursor"),
            ("intellij", "IntelliJ IDEA"),
            ("pycharm", "PyCharm"),
            ("webstorm", "WebStorm"),
            ("android studio", "Android Studio"),
            ("zed", "Zed"),
        ];
        if let Some((_, label)) = ide.iter().find(|(needle, _)| text.contains(needle)) {
            return ("ide".into(), Some((*label).into()));
        }
        let terminals = [
            ("iterm", "iTerm2"),
            ("wezterm", "WezTerm"),
            ("alacritty", "Alacritty"),
            ("kitty", "Kitty"),
            ("ghostty", "Ghostty"),
            ("terminal.app", "Terminal"),
            ("gnome-terminal", "Terminal"),
            ("konsole", "Konsole"),
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

pub(crate) fn display_name(
    process_name: &str,
    project: Option<&ProjectInfo>,
    tech: &str,
) -> String {
    if let Some(project) = project {
        if tech == "generic" || project.name.to_lowercase().contains(tech) {
            return project.name.clone();
        }
        return format!("{} · {}", project.name, friendly_tech(tech));
    }
    let value = process_name.trim();
    if value.is_empty() {
        friendly_tech(tech).into()
    } else {
        value.into()
    }
}

fn friendly_tech(tech: &str) -> &str {
    match tech {
        "nodejs" => "Node.js",
        "nextjs" => "Next.js",
        "postgresql" => "PostgreSQL",
        "mongodb" => "MongoDB",
        "dotnet" => ".NET",
        "spring" => "Spring Boot",
        "generic" => "Service",
        value => value,
    }
}

pub(crate) fn browser_url(
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
        .or_else(|| {
            endpoints
                .iter()
                .find(|endpoint| endpoint.scope == "wildcard")
        })
        .or_else(|| endpoints.first())?;
    let scheme = if spring.is_some_and(|settings| settings.ssl_enabled)
        || matches!(endpoint.port, 443 | 8443 | 9443)
        || args.iter().any(|arg| arg.to_lowercase().contains("https"))
    {
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

pub(crate) fn redact_command_full(args: &[String], fallback: &str) -> String {
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
        if is_sensitive_option(argument) {
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
    result.join(" ")
}

pub(crate) fn truncate_command(value: &str) -> String {
    if value.chars().count() > 600 {
        format!("{}…", value.chars().take(599).collect::<String>())
    } else {
        value.to_string()
    }
}

fn is_sensitive_option(argument: &str) -> bool {
    if !argument.starts_with('-') {
        return false;
    }
    let option = argument
        .trim_start_matches('-')
        .split(['=', ':'])
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let parts = option
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    parts.iter().any(|part| {
        matches!(
            *part,
            "password" | "passwd" | "secret" | "token" | "authorization" | "credential"
        )
    }) || parts.windows(2).any(|pair| pair == ["api", "key"])
        || option.replace(['-', '_'], "").contains("apikey")
}

fn redact_url(value: &str) -> String {
    let Some(scheme_end) = value.find("://") else {
        return value.into();
    };
    let authority_start = scheme_end + 3;
    let Some(at) = value[authority_start..]
        .find('@')
        .map(|index| index + authority_start)
    else {
        return value.into();
    };
    format!("{}•••@{}", &value[..authority_start], &value[at + 1..])
}

pub(crate) fn service_id(pid: u32, start_time: u64, ports: &[u16]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{pid}:{start_time}:{ports:?}").as_bytes());
    format!("service-{:x}", hasher.finalize())
        .chars()
        .take(24)
        .collect()
}
