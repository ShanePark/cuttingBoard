import { invoke } from "@tauri-apps/api/core";
import type {
  AppInfo,
  ContainerActionResult,
  ContainerLogSnapshot,
  ContainerListing,
  LaunchProfile,
  ManagedTaskSnapshot,
  ServiceLogSnapshot,
  TerminationResult,
  UpdateCheckResult,
  UiSettings,
  WorkspaceSnapshot
} from "./types";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  scan: () => invoke<WorkspaceSnapshot>("scan_workspace"),
  containers: () => invoke<ContainerListing>("list_containers"),
  containerLogs: (containerId: string) =>
    invoke<ContainerLogSnapshot>("container_logs", { request: { container_id: containerId } }),
  serviceLogs: (serviceId: string) =>
    invoke<ServiceLogSnapshot>("service_logs", { request: { service_id: serviceId } }),
  startContainer: (containerId: string) =>
    invoke<ContainerActionResult>("start_container", { request: { container_id: containerId } }),
  stopContainer: (containerId: string) =>
    invoke<ContainerActionResult>("stop_container", { request: { container_id: containerId } }),
  loadSettings: () => invoke<UiSettings>("load_settings"),
  saveSettings: (settings: UiSettings) => invoke<UiSettings>("save_settings", { settings }),
  terminate: (serviceId: string) => invoke<TerminationResult>("terminate_service", { request: { service_id: serviceId } }),
  restartService: (serviceId: string) => invoke<void>("restart_service", { request: { service_id: serviceId } }),
  profiles: () => invoke<LaunchProfile[]>("load_profiles"),
  saveProfile: (profile: LaunchProfile) => invoke<LaunchProfile[]>("save_profile", { profile }),
  deleteProfile: (profileId: string) => invoke<LaunchProfile[]>("delete_profile", { profileId }),
  taskSnapshots: () => invoke<ManagedTaskSnapshot[]>("task_snapshots"),
  taskLogTail: (profileId: string, taskName: string) =>
    invoke<string>("task_log_tail", { request: { profile_id: profileId, task_name: taskName } }),
  startTask: (profileId: string, taskName: string) =>
    invoke<ManagedTaskSnapshot>("start_task", { request: { profile_id: profileId, task_name: taskName } }),
  stopTask: (profileId: string, taskName: string) =>
    invoke<ManagedTaskSnapshot>("stop_task", { request: { profile_id: profileId, task_name: taskName } }),
  restartTask: (profileId: string, taskName: string) =>
    invoke<ManagedTaskSnapshot>("restart_task", { request: { profile_id: profileId, task_name: taskName } }),
  stopProfile: (profileId: string) => invoke<ManagedTaskSnapshot[]>("stop_profile", { profileId }),
  checkForUpdate: () => invoke<UpdateCheckResult>("check_for_update"),
  updateAndRestart: () => invoke<void>("update_and_restart"),
  shutdown: () => invoke<void>("shutdown")
};
