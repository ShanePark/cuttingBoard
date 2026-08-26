use crate::models::ProjectInfo;
use sha1::{Digest, Sha1};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn project_has_spring_evidence(project: &ProjectInfo) -> bool {
    let root = Path::new(&project.root_path);
    for build_file in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        let path = root.join(build_file);
        let Ok(metadata) = path.metadata() else {
            continue;
        };
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

    super::spring::spring_config_locations(Some(root), Some(root))
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
            super::spring::read_spring_config(&path, &[])
                .keys()
                .any(|key| {
                    key.starts_with("spring.")
                        || key.starts_with("server.")
                        || key.starts_with("management.")
                })
        })
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

pub(crate) fn detect_project(cwd: Option<&Path>, args: &[String]) -> Option<ProjectInfo> {
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
                candidates.push(if path.is_file() {
                    path.parent()?.to_path_buf()
                } else {
                    path
                });
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
            let replace = best.as_ref().map_or(true, |(current, _)| {
                root.components().count() > current.components().count()
            });
            if replace {
                best = Some((root, source));
            }
        }
    }
    let (root, source) = best?;
    let name = project_name(&root, source).unwrap_or_else(|| {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
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

pub(crate) fn looks_like_absolute_path(value: &str) -> bool {
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

pub(crate) fn is_excluded_project_root(path: &Path) -> bool {
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
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join(source)).ok()?).ok()?;
            value.get("name")?.as_str().map(clean_project_name)
        }
        "Cargo.toml" | "pyproject.toml" | "build.gradle" | "build.gradle.kts" => {
            named_assignment(&fs::read_to_string(root.join(source)).ok()?)
        }
        "go.mod" => fs::read_to_string(root.join(source))
            .ok()?
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .map(|value| clean_project_name(value.rsplit('/').next().unwrap_or(value)))
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

pub(crate) fn inferred_existing_path(args: &[String]) -> Option<PathBuf> {
    args.iter().find_map(|argument| {
        let value = argument.trim_matches(['\'', '"']);
        let path = PathBuf::from(value);
        if !path.is_absolute() || !path.exists() {
            return None;
        }
        Some(if path.is_dir() {
            path
        } else {
            path.parent()?.to_path_buf()
        })
    })
}
