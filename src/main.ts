import "./styles.css";
import { open as choosePath } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { techIcon, uiIcon } from "./icons";
import {
  browserLinkLabel,
  currentUptime,
  formatBytes,
  formatUptimeCompact,
  groupServices,
  imageTech,
  middleEllipsis,
  portBadgeLabels,
  shortenPath,
  uniquePorts
} from "./presentation";
import type {
  AppInfo,
  ContainerInfo,
  ContainerListing,
  LaunchProfile,
  LaunchState,
  LaunchTask,
  ManagedTaskSnapshot,
  ServiceSnapshot,
  ThemeMode,
  UiSettings,
  WorkspaceSnapshot
} from "./types";

type Tab = "services" | "docker" | "launch";
const SOURCE_URL = "https://github.com/ShanePark/cuttingBoard";

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("Missing application root");

root.innerHTML = `
  <div class="app-shell">
    <header class="toolbar">
      <nav class="tabs" aria-label="Workspace">
        <button class="tab is-active" type="button" data-tab="services">Services&nbsp;&nbsp;<span id="services-count">0</span></button>
        <button class="tab" type="button" data-tab="docker">Docker&nbsp;&nbsp;<span id="docker-count">0</span></button>
        <button class="tab" type="button" data-tab="launch">Launch Profiles&nbsp;&nbsp;<span id="launch-count">0</span></button>
      </nav>
      <button class="gear-button" type="button" data-action="settings" aria-label="Settings" title="Settings">${uiIcon("settings", 18)}</button>
    </header>
    <main id="workspace" class="workspace" aria-live="polite"></main>
    <footer id="status-footer" class="footer" hidden><span id="app-status"></span></footer>
  </div>
  <div id="modal-root"></div>
  <div id="toast-root" aria-live="assertive"></div>
`;

const workspaceElement = byId("workspace");
let activeTab: Tab = "services";
let appInfo: AppInfo | null = null;
let settings: UiSettings = {
  theme_mode: "dark",
  scan_interval_ms: 2000,
  window_width: 1080,
  window_height: 720,
  window_x: null,
  window_y: null
};
let workspace: WorkspaceSnapshot | null = null;
let containerListing: ContainerListing | null = null;
let profiles: LaunchProfile[] = [];
let taskSnapshots: ManagedTaskSnapshot[] = [];
let scanBusy = false;
let dockerBusy = false;
let closing = false;
let scanTimer: number | null = null;
let uptimeTimer: number | null = null;
let serviceSignature = "";
let dockerSignature = "";
let launchSignature = "";
let modalFocusReturn: HTMLElement | null = null;
const operations = new Set<string>();

root.addEventListener("click", (event) => void handleClick(event));
root.addEventListener("keydown", handleKeyboard);
document.addEventListener("keydown", (event) => {
  if (!document.querySelector(".modal-backdrop")) return;
  if (event.key === "Escape") closeModal();
  else if (event.key === "Tab") trapModalFocus(event);
});

void bootstrap();

async function bootstrap(): Promise<void> {
  try {
    const [info, loadedSettings, loadedProfiles, snapshots] = await Promise.all([
      api.appInfo(), api.loadSettings(), api.profiles(), api.taskSnapshots()
    ]);
    appInfo = info;
    settings = loadedSettings;
    profiles = loadedProfiles;
    taskSnapshots = snapshots;
    applyTheme(settings.theme_mode);
    renderHeaderCounts();
    await refreshWorkspace(true);
    installTimers();
    await installWindowLifecycle();
  } catch (error) {
    showFatal(error);
  }
}

function installTimers(): void {
  if (scanTimer !== null) window.clearInterval(scanTimer);
  scanTimer = window.setInterval(() => void refreshWorkspace(), Math.max(500, settings.scan_interval_ms));
  if (uptimeTimer !== null) window.clearInterval(uptimeTimer);
  uptimeTimer = window.setInterval(updateLiveUptimes, 1000);
}

async function installWindowLifecycle(): Promise<void> {
  const windowHandle = getCurrentWindow();
  await windowHandle.onCloseRequested(async (event) => {
    if (closing) return;
    event.preventDefault();
    closing = true;
    try {
      const [size, position] = await Promise.all([windowHandle.innerSize(), windowHandle.outerPosition()]);
      settings = await api.saveSettings({
        ...settings,
        window_width: Math.max(560, size.width),
        window_height: Math.max(420, size.height),
        window_x: position.x,
        window_y: position.y
      });
      await api.shutdown();
    } catch (error) {
      console.error(error);
    }
    await windowHandle.close();
  });
}

async function refreshWorkspace(force = false): Promise<void> {
  if (scanBusy) return;
  scanBusy = true;
  try {
    workspace = await api.scan();
    renderHeaderCounts();
    renderFooter();
    render(force);
    if (activeTab === "docker") await refreshContainers(force);
    if (activeTab === "launch") await refreshLaunch(force);
  } catch (error) {
    toast(messageOf(error), true);
  } finally {
    scanBusy = false;
  }
}

async function refreshContainers(force = false): Promise<void> {
  if (dockerBusy) return;
  dockerBusy = true;
  try {
    containerListing = await api.containers();
    renderHeaderCounts();
    if (activeTab === "docker") renderDocker(force);
  } catch (error) {
    containerListing = { available: false, containers: [], message: messageOf(error) };
    if (activeTab === "docker") renderDocker(true);
  } finally {
    dockerBusy = false;
  }
}

async function refreshLaunch(force = false): Promise<void> {
  try {
    [profiles, taskSnapshots] = await Promise.all([api.profiles(), api.taskSnapshots()]);
    renderHeaderCounts();
    if (activeTab === "launch") renderLaunch(force);
  } catch (error) {
    toast(messageOf(error), true);
  }
}

function render(force = false): void {
  if (activeTab === "services") renderServices(force);
  else if (activeTab === "docker") renderDocker(force);
  else renderLaunch(force);
}

function renderHeaderCounts(): void {
  const services = workspace?.services.filter((service) => service.relevance === "dev").length ?? 0;
  const fallbackContainers = workspace?.services.filter((service) => service.relevance === "container").length ?? 0;
  byId("services-count").textContent = String(services);
  byId("docker-count").textContent = String(containerListing?.available ? containerListing.containers.length : fallbackContainers);
  byId("launch-count").textContent = String(profiles.length);
}

function renderFooter(): void {
  if (!workspace) return;
  const status = workspace.errors[0] ?? (appInfo?.demo ? "Demonstration mode" : "");
  byId("app-status").textContent = status;
  byId("status-footer").toggleAttribute("hidden", !status);
}

function renderServices(force = false): void {
  const services = workspace?.services.filter((service) => service.relevance === "dev") ?? [];
  const signature = JSON.stringify(services.map((service) => [
    service.id, service.display_name, service.tech, uniquePorts(service), service.category, service.status,
    service.origin_kind, service.origin_label, service.can_terminate, service.browser_url,
    service.project?.id, service.project?.name, service.project?.root_path,
    service.project?.workspace_name, service.project?.workspace_root_path,
    service.process?.pid, service.process?.name,
    operations.has(`stop:${service.id}`)
  ]));
  if (!force && signature === serviceSignature && workspaceElement.dataset.view === "services") {
    updateLiveUptimes();
    return;
  }
  serviceSignature = signature;
  workspaceElement.dataset.view = "services";
  if (!workspace) {
    workspaceElement.innerHTML = loadingState("Finding services");
    return;
  }
  if (services.length === 0) {
    workspaceElement.innerHTML = emptyState("No development services are running", "Start a local server from a terminal, agent, or IDE.");
    return;
  }
  workspaceElement.innerHTML = `<div class="board">${groupServices(services).map((group) => `
    <section class="service-section" aria-labelledby="group-${h(encodeURIComponent(group.id))}">
      <header class="section-header">
        <span class="section-accent accent-${h(group.accent)}"></span>
        <h2 id="group-${h(encodeURIComponent(group.id))}">${h(group.name.toUpperCase())}</h2>
        ${group.path ? `<p title="${h(group.path)}">${h(shortenPath(group.path))}</p>` : ""}
      </header>
      <div class="section-rule"></div>
      <div class="tile-grid">${group.services.map(renderServiceTile).join("")}</div>
    </section>`).join("")}</div>`;
  updateLiveUptimes();
}

function renderServiceTile(service: ServiceSnapshot): string {
  const ports = uniquePorts(service);
  const labels = portBadgeLabels(ports);
  const busy = operations.has(`stop:${service.id}`);
  const uptime = formatUptimeCompact(currentUptime(service));
  const origin = ["agent", "ide"].includes(service.origin_kind) && service.origin_label;
  return `
    <article class="service-tile category-${service.category}${busy ? " is-busy" : ""}" aria-label="${h(service.display_name)} service">
      <button class="tile-details-button" type="button" data-tile-action data-action="service-details" data-service-id="${h(service.id)}" aria-label="View ${h(service.display_name)} details" title="View details"></button>
      <div class="icon-well" aria-hidden="true">${techIcon(service.tech, 44)}</div>
      <div class="tile-content">
        <h3 title="${h(service.display_name)}">${h(service.display_name)}</h3>
        <div class="status-line">
          <span class="uptime ${currentUptime(service) !== null && (currentUptime(service) ?? 999) < 300 ? "is-fresh" : ""}" data-uptime-id="${h(service.id)}">
            <span class="status-dot"></span><span data-uptime-text>${busy ? "Stopping…" : uptime ? `Running ${h(uptime)}` : ""}</span>
          </span>
          ${origin ? `<span class="origin-badge origin-${service.origin_kind}" title="Detected launch source: ${h(service.origin_label ?? service.origin_kind)}" aria-label="Detected launch source: ${h(service.origin_label ?? service.origin_kind)}"><span class="origin-dot"></span>${h(service.origin_label ?? "")}</span>` : ""}
        </div>
        <div class="port-row">
          ${ports.length ? `<span class="port-label">Ports</span>` : ""}
          ${labels.map((label) => `<span class="port-chip${label.startsWith("+") ? " port-overflow" : ""}" title="${h(portChipDescription(label, ports))}" aria-label="${h(portChipDescription(label, ports))}">${h(label)}</span>`).join("")}
          ${ports.length === 0 ? `<span class="no-port-label">No port information</span>` : ""}
        </div>
        ${service.browser_url ? `<button type="button" class="service-link" data-tile-action data-action="open-service" data-service-id="${h(service.id)}" aria-label="Open ${h(service.display_name)} in the browser" title="${h(service.browser_url)}">${h(browserLinkLabel(service.browser_url))}</button>` : ""}
        ${service.process ? `<span class="process-meta" title="Process: ${h(service.process.name)} · PID ${service.process.pid}" aria-label="Process ${h(service.process.name)}, PID ${service.process.pid}">PID ${service.process.pid}</span>` : ""}
      </div>
      <span class="details-hint">${uiIcon("details", 12)} Details</span>
      ${service.can_terminate ? `<button type="button" class="power-button" data-tile-action data-action="stop-service" data-service-id="${h(service.id)}" aria-label="${busy ? "Stopping" : "Stop"} ${h(service.display_name)}" title="${busy ? "Stopping process" : "Stop process"}" ${busy ? "disabled" : ""}><span class="power-glyph" aria-hidden="true"></span></button>` : ""}
    </article>`;
}

function renderDocker(force = false): void {
  const fallback = workspace?.services.filter((service) => service.relevance === "container") ?? [];
  const signature = JSON.stringify([containerListing, fallback.map((service) => [service.id, uniquePorts(service)])]);
  if (!force && signature === dockerSignature && workspaceElement.dataset.view === "docker") return;
  dockerSignature = signature;
  workspaceElement.dataset.view = "docker";
  if (!containerListing) {
    workspaceElement.innerHTML = loadingState("Reading Docker containers");
    void refreshContainers();
    return;
  }
  if (!containerListing.available) {
    if (fallback.length) {
      workspaceElement.innerHTML = `<div class="inline-notice"><strong>Docker could not be queried.</strong><span>${h(containerListing.message ?? "Docker is unavailable.")}</span></div><div class="board"><section class="service-section"><header class="section-header"><span class="section-accent accent-container"></span><h2>CONTAINER LISTENERS</h2></header><div class="section-rule"></div><div class="tile-grid">${fallback.map(renderServiceTile).join("")}</div></section></div>`;
      return;
    }
    workspaceElement.innerHTML = emptyState("Docker is unavailable", containerListing.message ?? "The Docker CLI could not be queried.");
    return;
  }
  if (containerListing.containers.length === 0) {
    workspaceElement.innerHTML = emptyState("No containers found", containerListing.message ?? "Docker returned an empty list.");
    return;
  }
  const groups = groupContainers(containerListing.containers);
  workspaceElement.innerHTML = `<div class="board">${groups.map((group) => `
    <section class="service-section">
      <header class="section-header"><span class="section-accent accent-container"></span><h2>${h(group.name.toUpperCase())}</h2></header>
      <div class="section-rule"></div><div class="tile-grid">${group.containers.map(renderContainerTile).join("")}</div>
    </section>`).join("")}</div>`;
}

function groupContainers(containers: ContainerInfo[]): Array<{ name: string; containers: ContainerInfo[] }> {
  const map = new Map<string, ContainerInfo[]>();
  for (const container of containers) {
    const name = container.compose_project ?? "Standalone containers";
    const group = map.get(name) ?? [];
    group.push(container);
    map.set(name, group);
  }
  return [...map].map(([name, items]) => ({
    name,
    containers: items.sort((a, b) => Number(b.state === "running") - Number(a.state === "running") || a.name.localeCompare(b.name))
  })).sort((a, b) => a.name === "Standalone containers" ? 1 : b.name === "Standalone containers" ? -1 : a.name.localeCompare(b.name));
}

function renderContainerTile(container: ContainerInfo): string {
  const labels = portBadgeLabels(container.ports);
  const running = container.state === "running";
  const stateText = `${running ? "Running" : "Stopped"}${container.status ? ` · ${ellipsis(container.status, 22)}` : ""}`;
  return `
    <article class="service-tile container-tile ${running ? "is-running" : "is-stopped"}" aria-label="${h(container.name)} container">
      <button class="tile-details-button" type="button" data-tile-action data-action="container-details" data-container-id="${h(container.id)}" aria-label="View ${h(container.name)} details" title="View details"></button>
      <div class="icon-well" aria-hidden="true">${techIcon(imageTech(container.image), 44)}</div>
      <div class="tile-content">
        <h3>${h(container.name)}</h3>
        <div class="status-line"><span class="container-state state-${h(container.state)}"><span class="status-dot"></span>${h(stateText)}</span></div>
        <div class="port-row">${container.ports.length ? `<span class="port-label">Ports</span>` : ""}${labels.map((label) => `<span class="port-chip${label.startsWith("+") ? " port-overflow" : ""}" title="${h(portChipDescription(label, container.ports))}" aria-label="${h(portChipDescription(label, container.ports))}">${h(label)}</span>`).join("")}${container.ports.length ? "" : `<span class="no-port-label">No published ports</span>`}</div>
        <span class="process-meta" title="Container image: ${h(container.image)}">${h(ellipsis(container.image, 22))}</span>
      </div><span class="details-hint">${uiIcon("details", 12)} Details</span>
    </article>`;
}

function renderLaunch(force = false): void {
  const signature = JSON.stringify([profiles, taskSnapshots, appInfo?.demo, [...operations]]);
  if (!force && signature === launchSignature && workspaceElement.dataset.view === "launch") return;
  launchSignature = signature;
  workspaceElement.dataset.view = "launch";
  const header = `<div class="launch-heading"><div><h1>Launch Profiles</h1><p>Run backend, frontend, and auto-build tasks together.</p></div><button class="primary-button icon-button-label" type="button" data-action="add-profile" ${appInfo?.demo ? "disabled" : ""}>${uiIcon("plus", 14)} Add</button></div>`;
  if (profiles.length === 0) {
    workspaceElement.innerHTML = `${header}<div class="launch-empty"><h2>No launch profiles yet</h2><p>Add a project and run commands to start and stop them together without an IDE.</p><button class="secondary-button" type="button" data-action="add-profile" ${appInfo?.demo ? "disabled" : ""}>Add First Profile</button></div>`;
    return;
  }
  workspaceElement.innerHTML = `${header}<div class="launch-list">${profiles.map(renderProfile).join("")}</div>`;
}

function renderProfile(profile: LaunchProfile): string {
  const snapshots = profile.tasks.map((task) => snapshotFor(profile.id, task.name));
  const canStop = snapshots.some((snapshot) => snapshot && ["starting", "running", "stopping"].includes(snapshot.state));
  const canStart = profile.tasks.some((task) => !["starting", "running", "stopping", "external"].includes(snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  const primary = canStart
    ? `<button class="primary-button icon-button-label" type="button" data-action="start-profile" data-profile-id="${h(profile.id)}">${uiIcon("play", 14)} ${canStop ? "Start Remaining" : "Start All"}</button>`
    : canStop ? `<button class="secondary-button warning-action icon-button-label" type="button" data-action="stop-profile" data-profile-id="${h(profile.id)}">${uiIcon("stop", 14)} Stop All</button>` : "";
  return `<section class="profile-card"><div class="profile-content">
    <header class="profile-header"><div class="profile-title-line"><h2>${h(profile.name)}</h2><span class="task-count">${profile.tasks.length} ${profile.tasks.length === 1 ? "Task" : "Tasks"}</span></div><p class="profile-path">${h(middleEllipsis(profile.project_root, 120))}</p></header>
    <div class="profile-actions-row"><div>${primary}</div><div class="profile-secondary-actions"><button class="quiet-button" type="button" data-action="edit-profile" data-profile-id="${h(profile.id)}" ${appInfo?.demo || canStop ? "disabled" : ""}>Edit</button><button class="quiet-button danger-button" type="button" data-action="delete-profile" data-profile-id="${h(profile.id)}" ${appInfo?.demo || canStop ? "disabled" : ""}>Delete</button></div></div>
    <div class="profile-separator"></div><div class="task-list">${profile.tasks.map((task) => renderTask(profile, task)).join("")}</div>
  </div></section>`;
}

function renderTask(profile: LaunchProfile, task: LaunchTask): string {
  const snapshot = snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  const active = ["starting", "running", "stopping"].includes(state);
  const external = state === "external";
  const busy = operations.has(`task:${profile.id}:${task.name}`);
  const meta = snapshot?.message || `${middleEllipsis(task.cwd, 54)}  ·  ${middleEllipsis(task.command, 100)}`;
  return `<article class="task-row state-${state}"><div class="task-copy">
    <h3>${h(middleEllipsis(task.name, 72))}</h3>
    <div class="task-status-line"><span class="task-state state-${state}"><span class="task-state-dot" aria-hidden="true"></span>${h(stateLabel(state))}</span>${task.expected_port ? `<span class="task-port">localhost:${task.expected_port}</span>` : ""}</div>
    <p class="task-message">${h(meta)}</p>
    <div class="task-actions-row"><div>${active ? `<button class="quiet-button warning-action icon-button-label" type="button" data-action="stop-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" ${busy ? "disabled" : ""}>${uiIcon("stop", 13)} Stop</button>` : !external ? `<button class="quiet-button start-action icon-button-label" type="button" data-action="start-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" ${busy ? "disabled" : ""}>${uiIcon("play", 13)} Start</button>` : ""}</div><button class="quiet-button logs-action icon-button-label" type="button" data-action="show-logs" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}">${uiIcon("log", 13)} Logs</button></div>
  </div></article>`;
}

async function handleClick(event: Event): Promise<void> {
  const eventElement = event.target instanceof Element ? event.target : null;
  if (eventElement instanceof HTMLElement && eventElement.matches(".modal-backdrop[data-dismiss-on-backdrop='true']")) {
    closeModal();
    return;
  }
  const target = eventElement?.closest<HTMLElement>("[data-action], [data-tab]") ?? null;
  if (!target) return;
  event.stopPropagation();
  const tab = target.dataset.tab as Tab | undefined;
  if (tab) {
    activeTab = tab;
    document.querySelectorAll<HTMLElement>("[data-tab]").forEach((item) => item.classList.toggle("is-active", item.dataset.tab === tab));
    render(true);
    if (tab === "docker") await refreshContainers(true);
    if (tab === "launch") await refreshLaunch(true);
    return;
  }
  const action = target.dataset.action;
  if (!action) return;
  try {
    if (action === "service-details") showServiceDetails(findService(required(target.dataset.serviceId)));
    else if (action === "open-service") await openService(required(target.dataset.serviceId));
    else if (action === "stop-service") await stopService(required(target.dataset.serviceId));
    else if (action === "container-details") showContainerDetails(findContainer(required(target.dataset.containerId)));
    else if (action === "settings") showSettings();
    else if (action === "close-modal") closeModal();
    else if (action === "save-settings") await saveSettingsFromModal();
    else if (action === "open-source") await openUrl(SOURCE_URL);
    else if (action === "add-profile") showProfileEditor(null);
    else if (action === "edit-profile") showProfileEditor(findProfile(required(target.dataset.profileId)));
    else if (action === "save-profile") await saveProfileFromModal(target.dataset.profileId ?? null);
    else if (action === "delete-profile") await deleteProfile(required(target.dataset.profileId));
    else if (action === "start-profile") await startProfile(required(target.dataset.profileId));
    else if (action === "stop-profile") await stopProfile(required(target.dataset.profileId));
    else if (action === "start-task") await runTask(required(target.dataset.profileId), required(target.dataset.taskName), true);
    else if (action === "stop-task") await runTask(required(target.dataset.profileId), required(target.dataset.taskName), false);
    else if (action === "show-logs") showLogs(required(target.dataset.profileId), required(target.dataset.taskName));
    else if (action === "choose-root") await chooseProfileRoot();
    else if (action === "add-task-row") addTaskRow();
    else if (action === "remove-task-row") target.closest(".task-editor")?.remove();
  } catch (error) {
    toast(messageOf(error), true);
  }
}

function handleKeyboard(event: KeyboardEvent): void {
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
  const current = event.target instanceof HTMLElement ? event.target.closest<HTMLButtonElement>("[data-tile-action]") : null;
  const card = current?.closest<HTMLElement>(".service-tile");
  if (!current || !card) return;
  const actions = [...card.querySelectorAll<HTMLButtonElement>("[data-tile-action]:not([disabled])")];
  if (actions.length < 2) return;
  event.preventDefault();
  const index = Math.max(0, actions.indexOf(current));
  const step = event.key === "ArrowRight" ? 1 : -1;
  actions[(index + step + actions.length) % actions.length]?.focus();
}

async function openService(id: string): Promise<void> {
  const service = findService(id);
  if (!service.browser_url) throw new Error("This service has no browser destination.");
  await openUrl(service.browser_url);
}

async function stopService(id: string): Promise<void> {
  const service = findService(id);
  if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
  if (!window.confirm(`Stop ${service.display_name}?\n\nPID ${service.process?.pid ?? "unknown"}`)) return;
  const key = `stop:${id}`;
  operations.add(key);
  renderServices(true);
  try {
    const result = await api.terminate(id);
    toast(result.message, !result.success);
    await refreshWorkspace(true);
  } finally {
    operations.delete(key);
    renderServices(true);
  }
}

async function startProfile(id: string): Promise<void> {
  const profile = findProfile(id);
  for (const task of profile.tasks) {
    const state = snapshotFor(id, task.name)?.state ?? "stopped";
    if (!["starting", "running", "stopping", "external"].includes(state)) await runTask(id, task.name, true, false);
  }
  await refreshLaunch(true);
}

async function stopProfile(id: string): Promise<void> {
  taskSnapshots = mergeSnapshots(taskSnapshots, await api.stopProfile(id));
  await refreshLaunch(true);
}

async function runTask(profileId: string, taskName: string, start: boolean, refresh = true): Promise<void> {
  const key = `task:${profileId}:${taskName}`;
  if (operations.has(key)) return;
  operations.add(key);
  renderLaunch(true);
  try {
    const snapshot = start ? await api.startTask(profileId, taskName) : await api.stopTask(profileId, taskName);
    taskSnapshots = mergeSnapshots(taskSnapshots, [snapshot]);
    if (snapshot.message) toast(snapshot.message, snapshot.state === "failed");
  } finally {
    operations.delete(key);
    if (refresh) await refreshLaunch(true);
  }
}

function mergeSnapshots(current: ManagedTaskSnapshot[], next: ManagedTaskSnapshot[]): ManagedTaskSnapshot[] {
  const map = new Map(current.map((snapshot) => [`${snapshot.profile_id}\0${snapshot.task_name}`, snapshot]));
  for (const snapshot of next) map.set(`${snapshot.profile_id}\0${snapshot.task_name}`, snapshot);
  return [...map.values()];
}

function showServiceDetails(service: ServiceSnapshot): void {
  const process = service.process;
  openModal(service.display_name, `
    <div class="detail-identity"><div class="detail-icon" aria-hidden="true">${techIcon(service.tech, 56)}</div><div><strong>${h(service.display_name)}</strong><span>${h(service.tech)} · ${h(service.category)}</span></div></div>
    ${service.warnings.map((warning) => `<div class="detail-warning">${h(warning)}</div>`).join("")}
    <dl class="detail-grid">
      <dt>Status</dt><dd>${h(service.status)}</dd><dt>Origin</dt><dd>${h(service.origin_label ?? service.origin_kind)}</dd>
      <dt>Project</dt><dd>${h(service.project?.root_path ?? "—")}</dd><dt>PID</dt><dd>${process?.pid ?? "—"}</dd>
      <dt>Executable</dt><dd>${h(process?.executable ?? "—")}</dd><dt>Working directory</dt><dd>${h(process?.working_directory ?? "—")}</dd>
      <dt>Command</dt><dd class="mono">${h(process?.command ?? "—")}</dd><dt>CPU</dt><dd>${process?.cpu_percent === null || process?.cpu_percent === undefined ? "Calculating" : `${process.cpu_percent.toFixed(1)}%`}</dd>
      <dt>Memory</dt><dd>${formatBytes(process?.memory_bytes ?? null)}</dd><dt>Uptime</dt><dd>${h(formatUptimeCompact(currentUptime(service)) || "—")}</dd>
    </dl>
    <h3 class="detail-heading">Listening endpoints</h3><div class="endpoint-list">${service.endpoints.map((endpoint) => `<div><span class="port-chip">${endpoint.port}</span><code>${h(endpoint.address)} · ${h(endpoint.family)} · ${h(endpoint.scope)}</code></div>`).join("")}</div>
    <div class="modal-actions">${service.browser_url ? `<button class="secondary-button" type="button" data-action="open-service" data-service-id="${h(service.id)}">Open</button>` : ""}<button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showContainerDetails(container: ContainerInfo): void {
  openModal(container.name, `<dl class="detail-grid"><dt>Container ID</dt><dd class="mono">${h(container.id)}</dd><dt>Image</dt><dd>${h(container.image)}</dd><dt>State</dt><dd>${h(container.state)}</dd><dt>Status</dt><dd>${h(container.status || "—")}</dd><dt>Compose project</dt><dd>${h(container.compose_project ?? "—")}</dd><dt>Compose service</dt><dd>${h(container.compose_service ?? "—")}</dd><dt>Published ports</dt><dd>${container.ports.length ? container.ports.join(", ") : "—"}</dd></dl><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showSettings(): void {
  const intervalChoices = [...new Set([1000, 2000, 5000, 10000, 30000, settings.scan_interval_ms])].sort((a, b) => a - b);
  openModal("Settings", `<form id="settings-form" class="settings-form" onsubmit="return false">
    <section class="settings-section" aria-labelledby="appearance-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("theme", 18)}</span><div><h3 id="appearance-heading">Appearance</h3><p>Choose how Cutting Board looks on this device.</p></div></div>
      <fieldset class="choice-fieldset"><legend class="sr-only">Theme</legend><div class="theme-options">
        ${themeChoice("light", "Light", "Bright and clear")}${themeChoice("dark", "Dark", "Easy on the eyes")}${themeChoice("system", "System", "Follow your device")}
      </div></fieldset>
    </section>
    <section class="settings-section" aria-labelledby="scanning-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("scan", 18)}</span><div><h3 id="scanning-heading">Scanning</h3><p>How often running local services are refreshed.</p></div></div>
      <fieldset class="choice-fieldset"><legend class="sr-only">Scan interval</legend><div class="interval-options">
        ${intervalChoices.map((value) => `<label class="interval-choice"><input type="radio" name="scan_interval_ms" value="${value}" ${settings.scan_interval_ms === value ? "checked" : ""}><span>${h(formatInterval(value))}</span></label>`).join("")}
      </div></fieldset>
      <p class="settings-help">Short intervals update quickly but use slightly more CPU. Two seconds is a good default.</p>
    </section>
    <button class="source-link" type="button" data-action="open-source" aria-label="View Cutting Board source code on GitHub">${uiIcon("github", 20)}<span><strong>Open source on GitHub</strong><small>github.com/ShanePark/cuttingBoard</small></span>${uiIcon("external", 16)}</button>
    <p class="form-note settings-privacy">Settings stay on this device. Cutting Board does not collect telemetry or start at login.</p>
    <div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button" type="button" data-action="save-settings">Save</button></div>
  </form>`);
}

async function saveSettingsFromModal(): Promise<void> {
  const form = document.querySelector<HTMLFormElement>("#settings-form");
  if (!form) return;
  const data = new FormData(form);
  const theme = String(data.get("theme_mode")) as ThemeMode;
  const interval = Number(data.get("scan_interval_ms"));
  settings = await api.saveSettings({ ...settings, theme_mode: theme, scan_interval_ms: Math.min(60000, Math.max(500, interval || 2000)) });
  applyTheme(settings.theme_mode);
  installTimers();
  closeModal();
  render(true);
  toast("Settings saved.");
}

function showProfileEditor(profile: LaunchProfile | null): void {
  const tasks = profile?.tasks.length ? profile.tasks : [{ name: "Backend", cwd: ".", command: "", expected_port: null }];
  openModal(profile ? "Edit Launch Profile" : "Add Launch Profile", `<form id="profile-form" class="form-stack" onsubmit="return false">
    <label>Profile name<input name="name" required maxlength="80" value="${h(profile?.name ?? "")}"></label>
    <label>Project root<div class="field-with-button"><input id="project-root" name="project_root" required value="${h(profile?.project_root ?? "")}"><button class="secondary-button" type="button" data-action="choose-root">Choose</button></div></label>
    <div class="task-editor-heading"><strong>Tasks</strong><button class="quiet-button icon-button-label" type="button" data-action="add-task-row">${uiIcon("plus", 13)} Add task</button></div>
    <div id="task-editors">${tasks.map(renderTaskEditor).join("")}</div>
    <div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button" type="button" data-action="save-profile" ${profile ? `data-profile-id="${h(profile.id)}"` : ""}>Save</button></div>
  </form>`, { dismissOnBackdrop: false });
}

function renderTaskEditor(task: LaunchTask): string {
  return `<fieldset class="task-editor"><button class="remove-task" type="button" data-action="remove-task-row" aria-label="Remove task">${uiIcon("close", 15)}</button><label>Name<input data-task-field="name" required value="${h(task.name)}"></label><label>Working directory<input data-task-field="cwd" required value="${h(task.cwd)}"></label><label>Command<input data-task-field="command" required value="${h(task.command)}"></label><label>Expected port (optional)<input data-task-field="expected_port" type="number" min="1" max="65535" value="${task.expected_port ?? ""}"></label></fieldset>`;
}

function addTaskRow(): void {
  document.querySelector("#task-editors")?.insertAdjacentHTML("beforeend", renderTaskEditor({ name: "", cwd: ".", command: "", expected_port: null }));
}

async function chooseProfileRoot(): Promise<void> {
  const result = await choosePath({ directory: true, multiple: false, title: "Choose project root" });
  const path = Array.isArray(result) ? result[0] : result;
  if (path) {
    const input = document.querySelector<HTMLInputElement>("#project-root");
    if (input) input.value = path;
  }
}

async function saveProfileFromModal(id: string | null): Promise<void> {
  const form = document.querySelector<HTMLFormElement>("#profile-form");
  if (!form) return;
  const data = new FormData(form);
  const tasks = [...form.querySelectorAll<HTMLElement>(".task-editor")].map((row) => {
    const value = (name: string): string => row.querySelector<HTMLInputElement>(`[data-task-field='${name}']`)?.value.trim() ?? "";
    const portValue = value("expected_port");
    return { name: value("name"), cwd: value("cwd"), command: value("command"), expected_port: portValue ? Number(portValue) : null } satisfies LaunchTask;
  });
  const profile: LaunchProfile = {
    id: id ?? crypto.randomUUID().replaceAll("-", ""),
    name: String(data.get("name") ?? "").trim(),
    project_root: String(data.get("project_root") ?? "").trim(),
    tasks
  };
  if (!profile.name || !profile.project_root || !tasks.length || tasks.some((task) => !task.name || !task.cwd || !task.command)) throw new Error("Complete every profile and task field.");
  profiles = await api.saveProfile(profile);
  closeModal();
  renderHeaderCounts();
  renderLaunch(true);
  toast("Launch profile saved.");
}

async function deleteProfile(id: string): Promise<void> {
  const profile = findProfile(id);
  if (!window.confirm(`Delete ${profile.name}?\n\nThis removes its saved commands.`)) return;
  profiles = await api.deleteProfile(id);
  renderHeaderCounts();
  renderLaunch(true);
  toast("Launch profile deleted.");
}

function showLogs(profileId: string, taskName: string): void {
  const snapshot = snapshotFor(profileId, taskName);
  openModal(`${findProfile(profileId).name} · ${taskName}`, `<pre class="log-view">${h(snapshot?.log_tail || "No output has been captured yet.")}</pre><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function openModal(title: string, body: string, options: { dismissOnBackdrop?: boolean } = {}): void {
  const dismissOnBackdrop = options.dismissOnBackdrop ?? true;
  const activeElement = document.activeElement;
  modalFocusReturn = activeElement instanceof HTMLElement && activeElement !== document.body ? activeElement : null;
  byId("modal-root").innerHTML = `<div class="modal-backdrop" data-dismiss-on-backdrop="${dismissOnBackdrop}" role="presentation"><section class="modal" tabindex="-1" role="dialog" aria-modal="true" aria-labelledby="modal-title"><header class="modal-header"><h2 id="modal-title">${h(title)}</h2><button type="button" class="modal-close" data-action="close-modal" aria-label="Close">${uiIcon("close", 18)}</button></header><div class="modal-body">${body}</div></section></div>`;
  document.querySelector<HTMLElement>(".modal button, .modal input, .modal select")?.focus();
  const appShell = document.querySelector<HTMLElement>(".app-shell");
  appShell?.setAttribute("inert", "");
  appShell?.setAttribute("aria-hidden", "true");
}

function closeModal(): void {
  if (!document.querySelector(".modal-backdrop")) return;
  byId("modal-root").replaceChildren();
  const appShell = document.querySelector<HTMLElement>(".app-shell");
  appShell?.removeAttribute("inert");
  appShell?.removeAttribute("aria-hidden");
  const focusReturn = modalFocusReturn;
  modalFocusReturn = null;
  if (focusReturn?.isConnected) focusReturn.focus();
}

function trapModalFocus(event: KeyboardEvent): void {
  const modal = document.querySelector<HTMLElement>(".modal");
  if (!modal) return;
  const focusable = [...modal.querySelectorAll<HTMLElement>(
    'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
  )].filter((element) => !element.hasAttribute("hidden") && element.getAttribute("aria-hidden") !== "true");
  if (focusable.length === 0) {
    event.preventDefault();
    modal.focus();
    return;
  }
  const first = focusable[0]!;
  const last = focusable[focusable.length - 1]!;
  if (!modal.contains(document.activeElement) || document.activeElement === modal) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}

function updateLiveUptimes(): void {
  if (!workspace) return;
  const services = new Map(workspace.services.map((service) => [service.id, service]));
  document.querySelectorAll<HTMLElement>("[data-uptime-id]").forEach((element) => {
    const service = services.get(element.dataset.uptimeId ?? "");
    if (!service) return;
    const busy = operations.has(`stop:${service.id}`);
    const uptime = currentUptime(service);
    const text = element.querySelector<HTMLElement>("[data-uptime-text]");
    if (text) text.textContent = busy ? "Stopping…" : uptime === null ? "" : `Running ${formatUptimeCompact(uptime)}`;
    element.classList.toggle("is-fresh", uptime !== null && uptime < 300);
  });
}

function applyTheme(mode: ThemeMode): void {
  document.documentElement.dataset.theme = mode;
  document.documentElement.style.colorScheme = mode === "system" ? "dark light" : mode;
}

function snapshotFor(profileId: string, taskName: string): ManagedTaskSnapshot | undefined {
  return taskSnapshots.find((snapshot) => snapshot.profile_id === profileId && snapshot.task_name === taskName);
}

function findService(id: string): ServiceSnapshot {
  const value = workspace?.services.find((service) => service.id === id);
  if (!value) throw new Error("The service is no longer available.");
  return value;
}

function findContainer(id: string): ContainerInfo {
  const value = containerListing?.containers.find((container) => container.id === id);
  if (!value) throw new Error("The container is no longer available.");
  return value;
}

function findProfile(id: string): LaunchProfile {
  const value = profiles.find((profile) => profile.id === id);
  if (!value) throw new Error("The launch profile no longer exists.");
  return value;
}

function stateLabel(state: LaunchState): string {
  return ({ stopped: "Stopped", starting: "Starting", running: "Running", stopping: "Stopping", failed: "Failed", external: "Running externally" })[state];
}

function themeChoice(value: ThemeMode, label: string, description: string): string {
  return `<label class="theme-choice"><input type="radio" name="theme_mode" value="${value}" ${settings.theme_mode === value ? "checked" : ""}><span class="theme-preview theme-preview-${value}" aria-hidden="true"><i></i><i></i><i></i></span><span class="theme-copy"><strong>${label}</strong><small>${description}</small></span><span class="choice-check">${uiIcon("check", 14)}</span></label>`;
}

function formatInterval(milliseconds: number): string {
  const seconds = milliseconds / 1000;
  return `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)} sec`;
}

function portChipDescription(label: string, ports: number[]): string {
  if (!label.startsWith("+")) return `Listening port ${label}`;
  return `${ports.length - 1} additional listening ports: ${ports.slice(1).join(", ")}`;
}

function loadingState(message: string): string { return `<div class="empty-state"><span class="spinner"></span><p>${h(message)}</p></div>`; }
function emptyState(title: string, message: string): string { return `<div class="empty-state"><h2>${h(title)}</h2><p>${h(message)}</p><button class="secondary-button" type="button" onclick="location.reload()">Refresh</button></div>`; }
function byId(id: string): HTMLElement { const value = document.getElementById(id); if (!value) throw new Error(`Missing #${id}`); return value; }
function required(value: string | undefined): string { if (!value) throw new Error("Missing action identifier."); return value; }
function ellipsis(value: string, limit: number): string { return value.length <= limit ? value : `${value.slice(0, limit - 1)}…`; }
function messageOf(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function h(value: unknown): string { return String(value ?? "").replace(/[&<>'"]/g, (character) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" })[character] ?? character); }

function toast(message: string, error = false): void {
  const host = byId("toast-root");
  host.innerHTML = `<div class="toast${error ? " is-error" : ""}">${h(message)}</div>`;
  window.setTimeout(() => host.replaceChildren(), 3500);
}

function showFatal(error: unknown): void {
  workspaceElement.innerHTML = `<div class="empty-state"><h2>Cutting Board could not start</h2><p>${h(messageOf(error))}</p></div>`;
}
