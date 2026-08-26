use crate::models::{ContainerInfo, ContainerListing};

pub(crate) fn container_listing() -> ContainerListing {
    ContainerListing {
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
    }
}

pub(crate) fn container_logs(container_id: &str) -> Option<&'static str> {
    match container_id {
        "4d2a92d5c814" => {
            Some("2026-01-01T00:00:00.000000000Z database system is ready to accept connections\n")
        }
        "e9396d773a34" => {
            Some("2026-01-01T00:00:00.000000000Z [INFO] mailpit listening on :8025\n")
        }
        "91fd0fb2d381" => Some("2025-12-29T00:00:00.000000000Z Ready to accept connections\n"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_deterministic_demo_logs_for_known_containers() {
        assert_eq!(
            container_logs("4d2a92d5c814").unwrap(),
            "2026-01-01T00:00:00.000000000Z database system is ready to accept connections\n"
        );
        assert_eq!(
            container_logs("e9396d773a34").unwrap(),
            "2026-01-01T00:00:00.000000000Z [INFO] mailpit listening on :8025\n"
        );
    }

    #[test]
    fn returns_empty_for_unknown_container_logs() {
        assert!(container_logs("0123456789ab").is_none());
    }
}
