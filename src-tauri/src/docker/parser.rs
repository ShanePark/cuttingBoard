use crate::models::{ContainerInfo, PublishedPortMapping};
use std::{collections::BTreeSet, fs};

pub(crate) fn parse_containers(text: &str) -> Vec<ContainerInfo> {
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
    containers
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
    let compose_working_dir =
        non_empty(columns.next()).map(|value| canonicalize_working_dir(&value));

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
        port_mappings: parse_published_port_mappings(ports),
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

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_published_ports(value: &str) -> Vec<u16> {
    let mut ports = BTreeSet::new();
    for mapping in parse_published_port_mappings(value) {
        ports.insert(mapping.host_port);
    }
    ports.into_iter().collect()
}

fn parse_published_port_mappings(value: &str) -> Vec<PublishedPortMapping> {
    let mut mappings = BTreeSet::new();
    for segment in value.split(',') {
        let Some((binding, target)) = segment.trim().split_once("->") else {
            continue;
        };
        let (container_port, protocol) = target
            .split_once('/')
            .map(|(port, protocol)| (port.trim(), protocol.trim().to_ascii_lowercase()))
            .unwrap_or((target.trim(), "tcp".into()));
        let Ok(container_port) = container_port.parse::<u16>() else {
            continue;
        };
        let Some((host_ip, host_port)) = parse_host_binding(binding.trim()) else {
            continue;
        };
        mappings.insert(PublishedPortMapping {
            host_ip,
            host_port,
            container_port,
            protocol,
        });
    }
    mappings.into_iter().collect()
}

fn parse_host_binding(value: &str) -> Option<(Option<String>, u16)> {
    let (host_ip, port) = if let Some(value) = value.strip_prefix('[') {
        let (host_ip, rest) = value.split_once(']')?;
        (Some(host_ip.to_string()), rest.strip_prefix(':')?)
    } else if let Some((host_ip, port)) = value.rsplit_once(':') {
        ((!host_ip.is_empty()).then(|| host_ip.to_string()), port)
    } else {
        (None, value)
    };
    port.parse::<u16>().ok().map(|port| (host_ip, port))
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
    fn parses_host_and_container_port_mappings_with_protocol_and_bind_address() {
        assert_eq!(
            parse_published_port_mappings(
                "0.0.0.0:18080->80/tcp, [::]:18443->443/tcp, 127.0.0.1:1025->1025/tcp, 0.0.0.0:9999->53/udp"
            ),
            vec![
                PublishedPortMapping {
                    host_ip: Some("0.0.0.0".into()),
                    host_port: 9999,
                    container_port: 53,
                    protocol: "udp".into(),
                },
                PublishedPortMapping {
                    host_ip: Some("0.0.0.0".into()),
                    host_port: 18080,
                    container_port: 80,
                    protocol: "tcp".into(),
                },
                PublishedPortMapping {
                    host_ip: Some("127.0.0.1".into()),
                    host_port: 1025,
                    container_port: 1025,
                    protocol: "tcp".into(),
                },
                PublishedPortMapping {
                    host_ip: Some("::".into()),
                    host_port: 18443,
                    container_port: 443,
                    protocol: "tcp".into(),
                },
            ]
        );
    }

    #[test]
    fn parses_dockers_unbracketed_ipv6_wildcard_binding() {
        assert_eq!(
            parse_published_port_mappings(":::18443->443/tcp"),
            vec![PublishedPortMapping {
                host_ip: Some("::".into()),
                host_port: 18443,
                container_port: 443,
                protocol: "tcp".into(),
            }]
        );
    }

    #[test]
    fn parses_container_columns() {
        let item = parse_container_line(
            "abc\tpostgres\tpostgres:16\trunning\tUp 1m\t0.0.0.0:5432->5432/tcp\tstack\tdb\t/work/stack",
        )
        .unwrap();
        assert_eq!(item.ports, vec![5432]);
        assert_eq!(
            item.port_mappings,
            vec![PublishedPortMapping {
                host_ip: Some("0.0.0.0".into()),
                host_port: 5432,
                container_port: 5432,
                protocol: "tcp".into(),
            }]
        );
        assert_eq!(item.compose_project.as_deref(), Some("stack"));
        assert_eq!(item.compose_working_dir.as_deref(), Some("/work/stack"));
    }

    #[test]
    fn sorts_running_containers_before_stopped_containers_by_name() {
        let containers = parse_containers(
            "a\tzulu\timage\texited\tExited\t\n\
             b\talpha\timage\trunning\tUp\t\n\
             c\tbeta\timage\trunning\tUp\t",
        );
        assert_eq!(
            containers
                .iter()
                .map(|container| container.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "zulu"]
        );
    }

    #[test]
    fn canonicalizes_existing_working_directories_and_preserves_missing_paths() {
        let temporary = tempfile::tempdir().unwrap();
        let existing = temporary.path().join("project");
        std::fs::create_dir(&existing).unwrap();
        let existing_text = existing.to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(&existing)
            .unwrap()
            .to_string_lossy()
            .into_owned();

        assert_eq!(canonicalize_working_dir(&existing_text), canonical);

        let missing = temporary
            .path()
            .join("missing")
            .to_string_lossy()
            .into_owned();
        assert_eq!(canonicalize_working_dir(&missing), missing);
    }
}
