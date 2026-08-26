use super::listeners::ListenerRecord;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(crate) struct SpringSettings {
    pub(crate) port: Option<u16>,
    pub(crate) management_port: Option<u16>,
    pub(crate) livereload_port: u16,
    pub(crate) context_path: String,
    pub(crate) ssl_enabled: bool,
    pub(crate) active_profiles: Vec<String>,
}

impl Default for SpringSettings {
    fn default() -> Self {
        Self {
            port: None,
            management_port: None,
            livereload_port: 35_729,
            context_path: String::new(),
            ssl_enabled: false,
            active_profiles: Vec::new(),
        }
    }
}

pub(crate) fn refine_spring_listeners(
    listeners: &mut Vec<ListenerRecord>,
    settings: &SpringSettings,
) -> bool {
    if let Some(port) = settings.port {
        if listeners.iter().any(|record| record.endpoint.port == port) {
            listeners.retain(|record| record.endpoint.port == port);
            return true;
        }
    }

    let management_port = settings
        .management_port
        .filter(|port| Some(*port) != settings.port);
    let livereload_port =
        (Some(settings.livereload_port) != settings.port).then_some(settings.livereload_port);
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

pub(crate) fn resolve_spring_settings(
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
        (
            "SPRING_DEVTOOLS_LIVERELOAD_PORT",
            "spring.devtools.livereload.port",
        ),
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
        .map(|value| spring_profiles(&resolve_spring_placeholder(value, &environment)))
        .unwrap_or_default();
    let mut properties = load_spring_properties(&locations, &active_profiles);
    properties.extend(environment_properties);
    properties.extend(command_properties);
    for value in properties.values_mut() {
        *value = resolve_spring_placeholder(value, &environment);
    }

    let defaults = SpringSettings::default();
    let port = properties
        .get("server.port")
        .and_then(|value| parse_port(value));
    let management_port = properties
        .get("management.server.port")
        .and_then(|value| parse_port(value));
    let livereload_port = properties
        .get("spring.devtools.livereload.port")
        .and_then(|value| parse_port(value))
        .unwrap_or(defaults.livereload_port);
    let ssl_enabled = properties
        .get("server.ssl.enabled")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    let context_path = normalize_context_path(
        properties
            .get("server.servlet.context-path")
            .or_else(|| properties.get("server.context-path"))
            .map(String::as_str)
            .unwrap_or_default(),
    );
    SpringSettings {
        port,
        management_port,
        livereload_port,
        context_path,
        ssl_enabled,
        active_profiles,
    }
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
                properties.insert(
                    name.to_string(),
                    value.trim_matches(['\'', '"']).to_string(),
                );
            } else if index + 1 < tokens.len() && !tokens[index + 1].starts_with('-') {
                properties.insert(option.to_string(), tokens[index + 1].to_string());
                index += 1;
            }
        }
        index += 1;
    }
    properties
}

pub(crate) fn spring_config_locations(
    cwd: Option<&Path>,
    project_root: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [project_root, cwd].into_iter().flatten() {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let mut locations = Vec::new();
    for root in roots {
        for location in [
            root.join("src/main/resources"),
            root.clone(),
            root.join("config"),
        ] {
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

pub(crate) fn read_spring_config(
    path: &Path,
    active_profiles: &[String],
) -> HashMap<String, String> {
    let Ok(metadata) = path.metadata() else {
        return HashMap::new();
    };
    if !metadata.is_file() || metadata.len() > 1_000_000 {
        return HashMap::new();
    }
    let Ok(text) = fs::read_to_string(path) else {
        return HashMap::new();
    };
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
            let separator = line
                .find(['=', ':'])
                .or_else(|| line.find(char::is_whitespace))?;
            let name = line[..separator].trim();
            let value = line[separator + 1..].trim();
            (!name.is_empty()).then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

pub(crate) fn parse_spring_yaml(text: &str, active_profiles: &[String]) -> HashMap<String, String> {
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
        let Some((key, raw_value)) = stripped.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        while parents
            .last()
            .is_some_and(|(parent_indent, _)| *parent_indent >= indent)
        {
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
        let Some(start) = resolved.find("${") else {
            break;
        };
        let Some(relative_end) = resolved[start + 2..].find('}') else {
            break;
        };
        let end = start + 2 + relative_end;
        let expression = &resolved[start + 2..end];
        let (name, default) = expression.split_once(':').unwrap_or((expression, ""));
        let replacement = environment.get(name).map(String::as_str).unwrap_or(default);
        resolved = format!(
            "{}{}{}",
            &resolved[..start],
            replacement,
            &resolved[end + 1..]
        );
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

pub(crate) fn spring_context_path(args: &[String]) -> Option<String> {
    const KEYS: &[&str] = &[
        "--server.servlet.context-path=",
        "-Dserver.servlet.context-path=",
        "--server.context-path=",
        "SERVER_SERVLET_CONTEXT_PATH=",
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
