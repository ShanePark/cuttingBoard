export type ThemeMode = "dark" | "light" | "system";
export type ServiceCategory = "web" | "api" | "database" | "cache" | "proxy" | "runtime" | "other";
export type Relevance = "dev" | "container" | "noise";
export type LaunchState = "stopped" | "starting" | "running" | "stopping" | "failed" | "external";

export interface Endpoint {
  family: string;
  address: string;
  port: number;
  scope: string;
  protocol: "TCP";
}

export interface ProcessInfo {
  pid: number;
  parent_pid: number | null;
  name: string;
  executable: string | null;
  working_directory: string | null;
  command: string;
  launch_command?: string | null;
  create_time: number;
  uptime_seconds: number;
  cpu_percent: number | null;
  memory_bytes: number | null;
  uid: number | null;
}

export interface ProjectInfo {
  id: string;
  name: string;
  root_path: string;
  workspace_root_path: string;
  workspace_name: string;
  detection_source: string;
}

export interface ServiceSnapshot {
  id: string;
  display_name: string;
  tech: string;
  category: ServiceCategory;
  relevance: Relevance;
  endpoints: Endpoint[];
  process: ProcessInfo | null;
  project: ProjectInfo | null;
  status: "healthy" | "limited";
  warnings: string[];
  origin_kind: "terminal" | "agent" | "ide" | "system" | "unknown";
  origin_label: string | null;
  can_terminate: boolean;
  browser_url: string | null;
  active_profiles: string[];
}

export interface WorkspaceSnapshot {
  services: ServiceSnapshot[];
  scanned_at: number;
  scan_duration_ms: number;
  endpoint_count: number;
  errors: string[];
}

export interface ContainerInfo {
  id: string;
  name: string;
  image: string;
  state: string;
  status: string;
  ports: number[];
  compose_project: string | null;
  compose_service: string | null;
}

export interface ContainerListing {
  available: boolean;
  containers: ContainerInfo[];
  message: string | null;
}

export interface UiSettings {
  theme_mode: ThemeMode;
  scan_interval_ms: number;
  window_width: number;
  window_height: number;
  window_x: number | null;
  window_y: number | null;
}

export interface LaunchTask {
  name: string;
  cwd: string;
  command: string;
  expected_port: number | null;
}

export interface LaunchProfile {
  id: string;
  name: string;
  project_root: string;
  tasks: LaunchTask[];
}

export interface ManagedTaskSnapshot {
  profile_id: string;
  task_name: string;
  state: LaunchState;
  main_pid: number | null;
  started_at: number | null;
  message: string | null;
  log_tail: string;
}

export interface AppInfo {
  version: string;
  demo: boolean;
  settings_path: string;
  profiles_path: string;
}

export interface TerminationResult {
  success: boolean;
  message: string;
}
