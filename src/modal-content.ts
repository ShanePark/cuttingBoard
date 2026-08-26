import { techIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import {
  currentUptime,
  formatBytes,
  formatUptimeCompact,
  relatedContainersForGroup,
  serviceTitle,
  techLabel,
  type ServiceGroup
} from "./presentation";
import { stateLabel } from "./launch-state";
import type {
  ContainerInfo,
  LaunchProfile,
  LaunchTask,
  ManagedTaskSnapshot,
  ServiceSnapshot
} from "./types";

const DONE_ACTION = `<div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`;

export function renderGroupDetails(group: ServiceGroup, containers: ContainerInfo[]): string {
  const serviceItems = group.services.map((service) => `<li><strong>${h(serviceTitle(service) || service.display_name)}</strong><span>${h(techLabel(service.tech))}</span></li>`).join("");
  const containerItems = relatedContainersForGroup(group.services, containers)
    .map((container) => `<li><strong>${h(container.name)}</strong><span>${h(container.status || container.state)}</span></li>`).join("");
  return `<div class="info-modal-copy"><p>Workspace details for this service group.</p>${group.path ? `<dl class="detail-grid"><dt>Project root</dt><dd class="mono">${h(group.path)}</dd></dl>` : ""}<h3 class="detail-heading">Services</h3><ul class="detail-list">${serviceItems || "<li><span>No services available</span></li>"}</ul>${containerItems ? `<h3 class="detail-heading">Docker</h3><ul class="detail-list">${containerItems}</ul>` : ""}</div>${DONE_ACTION}`;
}

export function renderContainerGroupDetails(containers: ContainerInfo[]): string {
  return `<div class="info-modal-copy"><p>Docker containers in this group.</p><ul class="detail-list">${containers.map((container) => `<li><strong>${h(container.name)}</strong><span>${h(container.status || container.state)}</span></li>`).join("") || "<li><span>No containers available</span></li>"}</ul></div>${DONE_ACTION}`;
}

export function renderProfileDetails(profile: LaunchProfile): string {
  const taskItems = profile.tasks.map((task) => `<li><strong>${h(task.name)}</strong><span class="profile-task-meta"><code class="mono">${h(task.command || "No command configured")}</code>${task.expected_port ? `<span>Port ${task.expected_port}</span>` : ""}</span></li>`).join("");
  return `<div class="info-modal-copy"><p>Saved commands and project context for this launch profile.</p><dl class="detail-grid"><dt>Project root</dt><dd class="mono">${h(profile.project_root)}</dd><dt>Tasks</dt><dd>${profile.tasks.length}</dd></dl><h3 class="detail-heading">Commands</h3><ul class="detail-list">${taskItems || "<li><span>No tasks configured</span></li>"}</ul></div>${DONE_ACTION}`;
}

export function renderTaskDetails(profile: LaunchProfile, task: LaunchTask, snapshot: ManagedTaskSnapshot | undefined, matchedService: ServiceSnapshot | null): string {
  const state = snapshot?.state ?? "stopped";
  const external = state === "external";
  const pid = external ? snapshot?.external_pid ?? snapshot?.main_pid ?? null : snapshot?.main_pid ?? null;
  const cwd = external ? snapshot?.external_working_directory || task.cwd : task.cwd;
  const uptime = snapshot?.started_at ? formatUptimeCompact(Date.now() / 1000 - snapshot.started_at) : "";
  const message = snapshot?.message?.trim() ?? "";
  return `
    <div class="detail-identity"><div class="detail-icon" aria-hidden="true">${matchedService ? techIcon(matchedService.tech, 56) : uiIcon("terminal", 34)}</div><div><strong>${h(task.name)}</strong><span>${h(profile.name)} · ${h(stateLabel(state))}</span></div></div>
    ${message ? `<div class="detail-warning">${h(message)}</div>` : ""}
    <dl class="detail-grid">
      <dt>State</dt><dd>${h(stateLabel(state))}</dd><dt>PID</dt><dd>${pid ?? "—"}</dd>
      <dt>Uptime</dt><dd>${h(uptime || "—")}</dd><dt>Expected port</dt><dd>${task.expected_port ?? "—"}</dd>
      <dt>Command</dt><dd class="mono">${h(task.command || "—")}</dd><dt>Working directory</dt><dd class="mono">${h(cwd || "—")}</dd>
      <dt>Project root</dt><dd class="mono">${h(profile.project_root)}</dd>
      ${external ? `<dt>External log</dt><dd class="mono">${h(snapshot?.external_log_path ?? "—")}</dd>` : ""}
      ${matchedService ? `<dt>Detected service</dt><dd>${h(serviceTitle(matchedService) || matchedService.display_name)}</dd><dt>Memory</dt><dd>${formatBytes(matchedService.process?.memory_bytes ?? null)}</dd>` : ""}
    </dl>
    ${DONE_ACTION}`;
}

export type InfoCopy = { title: string; message: string };

export function infoCopy(kind: string): InfoCopy {
  const copy: Record<string, InfoCopy> = {
    launch: { title: "Launch Profiles", message: "Group related commands into one place. Select a task to view its output, then use the play and stop icons to control it." },
    appearance: { title: "Appearance", message: "Choose whether Cutting Board follows the system theme or always uses a light or dark appearance." },
    scanning: { title: "Scanning", message: "This controls how often Cutting Board refreshes the list of running local services. A longer interval uses less CPU." },
    privacy: { title: "Privacy", message: "Settings are stored locally on this device. Cutting Board does not collect telemetry or start automatically at login." }
  };
  return copy[kind] ?? { title: "Information", message: "More information is available here." };
}

export function renderInfoDetails(message: string): string {
  return `<div class="info-modal-copy"><p>${h(message)}</p></div>${DONE_ACTION}`;
}

export function renderServiceDetails(service: ServiceSnapshot): string {
  const process = service.process;
  const activeProfiles = service.tech.trim().toLowerCase() === "spring"
    ? service.active_profiles.map((profile) => profile.trim()).filter(Boolean)
    : [];
  return `
    <div class="detail-identity"><div class="detail-icon" aria-hidden="true">${techIcon(service.tech, 56)}</div><div><strong>${h(serviceTitle(service))}</strong><span>${h(techLabel(service.tech))} · ${h(service.category)}</span></div></div>
    ${service.warnings.map((warning) => `<div class="detail-warning">${h(warning)}</div>`).join("")}
    <dl class="detail-grid">
      <dt>Status</dt><dd>${h(service.status)}</dd><dt>Origin</dt><dd>${h(service.origin_label ?? service.origin_kind)}</dd>
      ${activeProfiles.length ? `<dt>Active profiles</dt><dd class="detail-badges">${activeProfiles.map((profile) => `<span class="profile-badge">${h(profile)}</span>`).join("")}</dd>` : ""}
      <dt>Project</dt><dd>${h(service.project?.root_path ?? "—")}</dd><dt>PID</dt><dd>${process?.pid ?? "—"}</dd>
      <dt>Executable</dt><dd>${h(process?.executable ?? "—")}</dd><dt>Working directory</dt><dd>${h(process?.working_directory ?? "—")}</dd>
      <dt>Command</dt><dd class="mono">${h(process?.command ?? "—")}</dd><dt>CPU</dt><dd>${process?.cpu_percent === null || process?.cpu_percent === undefined ? "Calculating" : `${process.cpu_percent.toFixed(1)}%`}</dd>
      <dt>Memory</dt><dd>${formatBytes(process?.memory_bytes ?? null)}</dd><dt>Uptime</dt><dd>${h(formatUptimeCompact(currentUptime(service)) || "—")}</dd>
    </dl>
    <h3 class="detail-heading">Listening endpoints</h3><div class="endpoint-list">${service.endpoints.map((endpoint) => `<div><span class="port-chip">${endpoint.port}</span><code>${h(endpoint.address)} · ${h(endpoint.family)} · ${h(endpoint.scope)}</code></div>`).join("")}</div>`;
}

export function renderContainerDetails(container: ContainerInfo): string {
  return `<dl class="detail-grid"><dt>Container ID</dt><dd class="mono">${h(container.id)}</dd><dt>Image</dt><dd>${h(container.image)}</dd><dt>State</dt><dd>${h(container.state)}</dd><dt>Status</dt><dd>${h(container.status || "—")}</dd><dt>Compose project</dt><dd>${h(container.compose_project ?? "—")}</dd><dt>Compose service</dt><dd>${h(container.compose_service ?? "—")}</dd><dt>Compose working directory</dt><dd>${h(container.compose_working_dir ?? "—")}</dd><dt>Published ports</dt><dd>${container.ports.length ? container.ports.join(", ") : "—"}</dd></dl>${DONE_ACTION}`;
}
