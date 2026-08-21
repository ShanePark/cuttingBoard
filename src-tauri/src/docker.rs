use crate::models::{ContainerInfo, ContainerListing};
use std::{collections::BTreeSet, process::Command};

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
                },
            ],
            message: None,
        };
    }

    let format = concat!(
        "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.State}}\t{{.Status}}\t{{.Ports}}\t",
        "{{.Label \"com.docker.compose.project\"}}\t",
        "{{.Label \"com.docker.compose.service\"}}"
    );
    let output = match Command::new("docker")
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
    })
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
        let item = parse_container_line("abc\tpostgres\tpostgres:16\trunning\tUp 1m\t0.0.0.0:5432->5432/tcp\tstack\tdb").unwrap();
        assert_eq!(item.ports, vec![5432]);
        assert_eq!(item.compose_project.as_deref(), Some("stack"));
    }
}
