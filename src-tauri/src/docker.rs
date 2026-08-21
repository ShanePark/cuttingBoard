use crate::models::{ContainerInfo, ContainerListing};
use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn list_containers(demo: bool) -> ContainerListing {
    if demo {
        return ContainerListing {
            available: true,
            containers: vec![
                ContainerInfo {
                    id: "4d2a92d5c814".into(),
                    name: "local-postgres".into(),
                    image: "postgres:16".into(),
                    state: "running".into(),
                    status: "Up 2 hours".into(),
                    ports: vec![5432],
                    compose_project: Some("local-stack".into()),
                    compose_service: Some("database".into()),
                    compose_working_dir: Some("/Users/shane/Developer/local-stack".into()),
                },
                ContainerInfo {
                    id: "e9396d773a34".into(),
                    name: "mailpit".into(),
                    image: "axllent/mailpit:latest".into(),
                    state: "running".into(),
                    status: "Up 2 hours".into(),
                    ports: vec![1025, 8025],
                    compose_project: Some("local-stack".into()),
                    compose_service: Some("mail".into()),
                    compose_working_dir: Some("/Users/shane/Developer/local-stack".into()),
                },
                ContainerInfo {
                    id: "91fd0fb2d381".into(),
                    name: "old-redis".into(),
                    image: "redis:7".into(),
                    state: "exited".into(),
                    status: "Exited (0) 3 days ago".into(),
                    ports: vec![],
                    compose_project: None,
                    compose_service: None,
                    compose_working_dir: None,
                },
            ],
            message: None,
        };
    }

    let format = concat!(
        "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t",
        "{{.Label \"com.docker.compose.project\"}}\t",
        "{{.Label \"com.docker.compose.service\"}}\t",
        "{{.Label \"com.docker.compose.project.working_dir\"}}"
    );
    let Some(executable) = resolve_docker_executable() else {
        return ContainerListing {
            available: false,
            containers: vec![],
            message: Some("Docker CLI was not found in PATH or standard macOS locations.".into()),
        };
    };
    let output = match Command::new(executable)
        .args(["ps", "-a", "--no-trunc", "--format", format])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            return ContainerListing {
                available: false,
                containers: vec![],
                message: Some(format!("Could not start Docker: {error}")),
            };
        }
    };
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return ContainerListing {
            available: false,
            containers: vec![],
            message: Some(if message.is_empty() {
                "Docker returned an error.".into()
            } else {
                message
            }),
        };
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut containers = text
        .lines()
        .filter_map(parse_container_line)
        .collect::<Vec<_>>();
    containers.sort_by(|left, right| {
        let left_running = left.state.eq_ignore_ascii_case("running");
        let right_running = right.state.eq_ignore_ascii_case("running");
        right_running
            .cmp(&left_running)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    ContainerListing {
        available: true,
        containers,
        message: None,
    }
}

fn parse_container_line(line: &str) -> Option<ContainerInfo> {
    let mut columns = line.split('\t');
    let id = columns.next()?.trim();
    let name = columns.next()?.trim();
    let image = columns.next()?.trim();
    let state = columns.next()?.trim();
    let status = columns.next()?.trim();
    let ports = columns.next().unwrap_or_default();
    let compose_project = non_empty(columns.next());
    let compose_service = non_empty(columns.next());
    let compose_working_dir = non_empty(columns.next()).map(|value| canonicalize_working_dir(&value));
    if id.is_empty() || name.is_empty() {
        return None;
    }
    Some(ContainerInfo {
        id: id.into(),
        name: name.into(),
        image: image.into(),
        state: state.to_lowercase(),
        status: status.into(),
        ports: parse_published_ports(ports),
        compose_project,
        compose_service,
        compose_working_dir,
    })
}

fn canonicalize_working_dir(value: &str) -> String {
    fs::canonicalize(value)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|_| value.to_string())
}

fn resolve_docker_executable() -> Option<PathBuf> {
    let path = env::var_os("PATH");
    let home = dirs::home_dir();
    docker_executable_candidates(path.as_deref(), home.as_deref())
        .into_iter()
        .find(|candidate| is_executable(candidate))
}

fn docker_executable_candidates(path: Option<&OsStr>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = path
        .into_iter()
        .flat_map(env::split_paths)
        .filter(|entry| !entry.as_os_str().is_empty())
        .map(|entry| entry.join("docker"))
        .collect::<Vec<_>>();

    candidates.extend([
        PathBuf::from("/opt/homebrew/bin/docker"),
        PathBuf::from("/usr/local/bin/docker"),
        PathBuf::from("/Applications/Docker.app/Contents/Resources/bin/docker"),
    ]);
    if let Some(home) = home {
        candidates.push(home.join("Applications/Docker.app/Contents/Resources/bin/docker"));
    }
    candidates
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|value| !value.is_empty()).map(str::to_string)
}

fn parse_published_ports(value: &str) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    for segment in value.split(',') {
        let left = segment.trim().split("->").next().unwrap_or_default();
        if !segment.contains("->") {
            continue;
        }
        let port_text = if let Some(end) = left.rfind(']') {
            left[end + 1..].trim_start_matches(':')
        } else {
            left.rsplit_once(':').map(|(_, port)| port).unwrap_or(left)
        };
        if let Ok(port) = port_text.parse::<u16>() {
            ports.insert(port);
        }
    }
    ports.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_published_ports_only() {
        assert_eq!(
            parse_published_ports("0.0.0.0:5432->5432/tcp, [::]:5432->5432/tcp, 6379/tcp"),
            vec![5432]
        );
    }

    #[test]
    fn parses_container_columns() {
        let item = parse_container_line("abc\tpostgres\tpostgres:16\trunning\tUp 1m\t0.0.0.0:5432->5432/tcp\tstack\tdb\t/work/stack").unwrap();
        assert_eq!(item.ports, vec![5432]);
        assert_eq!(item.compose_project.as_deref(), Some("stack"));
        assert_eq!(item.compose_working_dir.as_deref(), Some("/work/stack"));
    }

    #[test]
    fn canonicalizes_existing_working_directories_and_preserves_missing_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let existing = temporary.path().join("project");
        std::fs::create_dir(&existing).unwrap();
        let existing_text = existing.to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(&existing).unwrap().to_string_lossy().into_owned();

        assert_eq!(canonicalize_working_dir(&existing_text), canonical);

        let missing = temporary.path().join("missing").to_string_lossy().into_owned();
        assert_eq!(canonicalize_working_dir(&missing), missing);
    }

    #[test]
    fn docker_path_candidates_prefer_gui_process_path_before_fallbacks() {
        let path = env::join_paths([
            PathBuf::from("/tmp/custom-bin"),
            PathBuf::from("/tmp/another-bin"),
        ])
        .unwrap();
        let candidates = docker_executable_candidates(
            Some(path.as_os_str()),
            Some(Path::new("/Users/example")),
        );

        assert_eq!(candidates[0], PathBuf::from("/tmp/custom-bin/docker"));
        assert_eq!(candidates[1], PathBuf::from("/tmp/another-bin/docker"));
        assert_eq!(candidates[2], PathBuf::from("/opt/homebrew/bin/docker"));
    }
}
