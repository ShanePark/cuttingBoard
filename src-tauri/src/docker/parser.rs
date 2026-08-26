use crate::models::ContainerInfo;
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
        let item = parse_container_line(
            "abc\tpostgres\tpostgres:16\trunning\tUp 1m\t0.0.0.0:5432->5432/tcp\tstack\tdb\t/work/stack",
        )
        .unwrap();
        assert_eq!(item.ports, vec![5432]);
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
