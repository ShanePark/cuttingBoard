use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: Vec<u16>,
    #[serde(default)]
    pub port_mappings: Vec<PublishedPortMapping>,
    pub compose_project: Option<String>,
    pub compose_service: Option<String>,
    #[serde(default)]
    pub compose_working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PublishedPortMapping {
    pub host_ip: Option<String>,
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerListing {
    pub available: bool,
    pub containers: Vec<ContainerInfo>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRequest {
    pub container_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerActionResult {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerLogSnapshot {
    pub logs: String,
}
