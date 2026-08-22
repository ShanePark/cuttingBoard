import "./styles.css";
import { open as choosePath } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import { techIcon, uiIcon } from "./icons";
import type { UiIconName } from "./icons";
import {
  FRESH_UPTIME_SECONDS,
  browserLinkLabel,
  currentUptime,
  formatBytes,
  formatUptimeCompact,
  groupServices,
  imageTech,
  launchTasksEquivalent,
  middleEllipsis,
  portBadgeLabels,
  relatedContainersForGroup,
  serviceTitle,
  shortenPath,
  techLabel,
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
const MIN_TILE_WIDTH = 300;

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("Missing application root");

root.innerHTML = `
  <div class="app-shell">
    <header class="toolbar">
      <nav class="tabs" aria-label="Workspace">
        <button class="tab is-active" type="button" data-tab="services">${uiIcon("power", 14)}<span>Services</span><span class="tab-count" id="services-count">0</span></button>
        <button class="tab" type="button" data-tab="docker">${uiIcon("docker", 14)}<span>Docker</span><span class="tab-count" id="docker-count">0</span></button>
        <button class="tab" type="button" data-tab="launch">${uiIcon("play", 14)}<span>Launch Profiles</span><span class="tab-count" id="launch-count">0</span></button>
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
  window_y: null,
  window_geometry_logical: false
};
let workspace: WorkspaceSnapshot | null = null;
let containerListing: ContainerListing | null = null;
let profiles: LaunchProfile[] = [];
let taskSnapshots: ManagedTaskSnapshot[] = [];
let scanBusy = false;
let dockerBusy = false;
let scanTimer: number | null = null;
let uptimeTimer: number | null = null;
let serviceSignature = "";
let dockerSignature = "";
let launchSignature = "";
let modalFocusReturn: HTMLElement | null = null;
let pendingServiceStopId: string | null = null;
let pendingGroupSaveId: string | null = null;
let pendingGroupStopId: string | null = null;
let pendingProfileDeleteId: string | null = null;
const operations = new Set<string>();
type LaunchTaskRef = { profile: LaunchProfile; task: LaunchTask };
let selectedTaskKey: string | null = null;
let consoleWrap = false;
let consoleFollow = true;
let consoleScrollTop = 0;
let consoleScrollTaskKey: string | null = null;
let restoringConsoleScroll = false;
type DockerLogState = {
  containerId: string | null;
  logs: string;
  loading: boolean;
  error: string | null;
};
let selectedContainerId: string | null = null;
let dockerLogState: DockerLogState = { containerId: null, logs: "", loading: false, error: null };
let dockerLogRequestId = 0;
let dockerConsoleScrollTop = 0;
let dockerConsoleScrollContainerId: string | null = null;
let restoringDockerConsoleScroll = false;

root.addEventListener("click", (event) => void handleClick(event));
root.addEventListener("keydown", handleKeyboard);
root.addEventListener("scroll", handleConsoleScroll, true);
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
    installBoardObserver();
  } catch (error) {
    showFatal(error);
  }
}

function installBoardObserver(): void {
  new ResizeObserver(() => applyBoardLayout()).observe(workspaceElement);
}

// Groups share rows: the board is one column grid and every section spans as many
// columns as it has cards, so a one-card group and a two-card group sit side by side.
function applyBoardLayout(): void {
  const board = workspaceElement.querySelector<HTMLElement>(".board");
  if (!board) return;
  const styles = window.getComputedStyle(board);
  const gap = Number.parseFloat(styles.columnGap) || 0;
  const width = board.clientWidth - Number.parseFloat(styles.paddingLeft) - Number.parseFloat(styles.paddingRight);
  const columns = Math.max(1, Math.floor((width + gap) / (MIN_TILE_WIDTH + gap)));
  board.style.setProperty("--board-columns", String(columns));
  for (const section of board.querySelectorAll<HTMLElement>(".service-section")) {
    const tiles = Number(section.dataset.tiles) || 1;
    section.style.setProperty("--section-span", String(Math.min(Math.max(1, tiles), columns)));
  }
}

function installTimers(): void {
  if (scanTimer !== null) window.clearInterval(scanTimer);
  scanTimer = window.setInterval(() => void refreshWorkspace(), Math.max(500, settings.scan_interval_ms));
  if (uptimeTimer !== null) window.clearInterval(uptimeTimer);
  uptimeTimer = window.setInterval(updateLiveMetrics, 1000);
}

async function refreshWorkspace(force = false): Promise<void> {
  if (scanBusy) return;
  scanBusy = true;
  try {
    workspace = await api.scan();
    renderHeaderCounts();
    renderFooter();
    render(force);
    if (activeTab === "services" || activeTab === "docker") await refreshContainers(force);
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
    syncSelectedContainer();
    renderHeaderCounts();
    if (activeTab === "docker") await refreshSelectedContainerLogs();
    if (activeTab === "docker") renderDocker(force);
    if (activeTab === "services") renderServices(force);
  } catch (error) {
    containerListing = { available: false, containers: [], message: messageOf(error) };
    clearDockerSelection();
    if (activeTab === "docker") renderDocker(true);
    if (activeTab === "services") renderServices(true);
  } finally {
    dockerBusy = false;
  }
}

function syncSelectedContainer(): void {
  if (!selectedContainerId) return;
  const stillAvailable = containerListing?.available && containerListing.containers.some((container) => container.id === selectedContainerId);
  if (!stillAvailable) clearDockerSelection();
}

function clearDockerSelection(): void {
  selectedContainerId = null;
  dockerLogRequestId += 1;
  dockerLogState = { containerId: null, logs: "", loading: false, error: null };
  dockerConsoleScrollContainerId = null;
  dockerConsoleScrollTop = 0;
}

async function refreshSelectedContainerLogs(): Promise<void> {
  if (!selectedContainerId || !containerListing?.available) return;
  if (dockerLogState.containerId === selectedContainerId && dockerLogState.loading) return;
  await loadContainerLogs(selectedContainerId);
}

async function loadContainerLogs(containerId: string, showLoading = false): Promise<void> {
  if (selectedContainerId !== containerId) return;
  if (!showLoading && dockerLogState.containerId === containerId && dockerLogState.loading) return;
  const requestId = ++dockerLogRequestId;
  if (showLoading) {
    dockerLogState = { containerId, logs: "", loading: true, error: null };
    if (activeTab === "docker") renderDocker(true);
  }
  try {
    const result = await api.containerLogs(containerId);
    if (requestId !== dockerLogRequestId || selectedContainerId !== containerId) return;
    dockerLogState = { containerId, logs: result.logs ?? "", loading: false, error: null };
    if (activeTab === "docker") renderDocker(true);
  } catch (error) {
    if (requestId !== dockerLogRequestId || selectedContainerId !== containerId) return;
    dockerLogState = { ...dockerLogState, containerId, loading: false, error: messageOf(error) };
    if (activeTab === "docker") renderDocker(true);
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
  const containers = containerListing?.available ? containerListing.containers : [];
  const groups = groupServices(services).map((group) => ({
    ...group,
    containers: relatedContainersForGroup(group.services, containers)
  }));
  const signature = JSON.stringify([
    services.map((service) => [
      service.id, service.display_name, service.tech, uniquePorts(service), service.category, service.status,
      service.origin_kind, service.origin_label, service.can_terminate, service.browser_url,
      service.project?.id, service.project?.name, service.project?.root_path,
      service.project?.workspace_name, service.project?.workspace_root_path,
      service.process?.pid, service.process?.name,
      service.active_profiles,
      operations.has(`stop:${service.id}`)
    ]),
    groups.map((group) => [
      group.id,
      group.containers.map((container) => [
        container.id, container.name, container.image, container.state, container.status,
        container.ports, container.compose_project, container.compose_service, container.compose_working_dir
      ])
    ])
  ]);
  if (!force && signature === serviceSignature && workspaceElement.dataset.view === "services") {
    updateLiveMetrics();
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
  workspaceElement.innerHTML = `<div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.services.length + group.containers.length}" aria-labelledby="group-${h(encodeURIComponent(group.id))}">
      <header class="section-header">
        <span class="section-accent accent-${h(group.accent)}"></span>
        <h2 id="group-${h(encodeURIComponent(group.id))}">${h(group.name.toUpperCase())}</h2>
        ${group.path ? `<p title="${h(group.path)}">${h(shortenPath(group.path))}</p>` : ""}
        <span class="section-count" aria-label="${group.services.length} services${group.containers.length ? ` and ${group.containers.length} Docker containers` : ""}">${group.services.length + group.containers.length}</span>
        ${renderGroupActions(group)}
      </header>
      <div class="tile-grid">${group.services.map((service) => renderServiceTile(service)).join("")}${group.containers.map((container) => renderContainerTile(container)).join("")}</div>
    </section>`).join("")}</div>`;
  applyBoardLayout();
  updateLiveMetrics();
}

function renderGroupTitle(title: string, itemCount: number, action: string, attributes: string, displayTitle = title, actionTitle = "View group details"): string {
  const escapedTitle = h(title);
  return itemCount > 1
    ? `<button class="group-title-button" type="button" data-action="${h(action)}" ${attributes} aria-label="View ${escapedTitle} details" title="${h(actionTitle)}">${h(displayTitle)}</button>`
    : h(displayTitle);
}

function renderServiceGroupTiles(group: ReturnType<typeof groupServices>[number] & { containers: ContainerInfo[] }): string {
  const total = group.services.length + group.containers.length;
  const ordinalTotal = total > 1 ? total : undefined;
  return `${group.services.map((service, index) => renderServiceTile(service, ordinalTotal ? index + 1 : undefined, ordinalTotal)).join("")}${group.containers.map((container, index) => renderContainerTile(container, ordinalTotal ? group.services.length + index + 1 : undefined, ordinalTotal, false)).join("")}`;
}

function renderGroupActions(group: ReturnType<typeof groupServices>[number]): string {
  const saveBusy = operations.has(`group-save:${group.id}`);
  const stopBusy = operations.has(`group-stop:${group.id}`);
  const profilePresent = profileForGroup(group) !== undefined;
  const saveAction = profilePresent
    ? `<span class="group-profile-saved" title="Launch profile saved" aria-label="Launch profile saved">${uiIcon("check", 14)}</span>`
    : `<button class="section-action save-group-action icon-button-label" type="button" data-action="save-service-group" data-group-id="${h(group.id)}" title="Save launch profile" aria-label="${saveBusy ? "Saving" : "Save"} launch profile for ${h(group.name)}" ${saveBusy ? "disabled" : ""}>${uiIcon("folder", 13)}<span>${saveBusy ? "Saving…" : "Save"}</span></button>`;
  const terminableCount = group.services.filter((service) => service.can_terminate).length;
  const stopDisabled = !terminableCount || stopBusy;
  return `<div class="section-actions">${saveAction}<button class="section-action stop-group-action icon-button-label" type="button" data-action="stop-service-group" data-group-id="${h(group.id)}" title="${terminableCount ? "Stop all stoppable services" : "No services can be stopped safely"}" aria-label="${stopBusy ? "Stopping" : terminableCount ? "Stop all" : "No stoppable"} services in ${h(group.name)}" ${stopDisabled ? "disabled" : ""}>${uiIcon("stop", 13)}<span>${stopBusy ? "Stopping…" : "Stop All"}</span></button></div>`;
}

function generatedTasksForGroup(services: ServiceSnapshot[]): LaunchTask[] {
  const usedNames = new Set<string>();
  return services.map((service) => {
    const baseName = serviceTitle(service) || service.display_name;
    let name = baseName;
    let suffix = 2;
    while (usedNames.has(name.toLowerCase())) name = `${baseName} ${suffix++}`;
    usedNames.add(name.toLowerCase());
    return {
      name,
      cwd: service.process?.working_directory ?? "",
      command: service.process?.launch_command ?? service.process?.command ?? "",
      expected_port: uniquePorts(service)[0] ?? null
    };
  });
}

function groupForId(id: string): ReturnType<typeof groupServices>[number] {
  const group = groupServices(workspace?.services.filter((service) => service.relevance === "dev") ?? []).find((item) => item.id === id);
  if (!group) throw new Error("The workspace group is no longer available.");
  return group;
}

function profileForGroup(group: ReturnType<typeof groupServices>[number]): LaunchProfile | undefined {
  if (!group.path) return undefined;
  return profiles.find((profile) => profile.project_root === group.path && launchTasksEquivalent(profile.tasks, generatedTasksForGroup(group.services)));
}

function validateGroupProfile(group: ReturnType<typeof groupServices>[number]): LaunchTask[] {
  if (!group.path) throw new Error("This workspace group has no project root and cannot be saved.");
  const incomplete = group.services.filter((service) => !service.process?.command.trim() || !service.process?.working_directory?.trim());
  if (incomplete.length) throw new Error(`Cannot save ${group.name}: ${incomplete.map((service) => serviceTitle(service) || service.display_name).join(", ")} ${incomplete.length === 1 ? "is missing" : "are missing"} a process command or working directory.`);
  return generatedTasksForGroup(group.services);
}

function renderServiceTile(service: ServiceSnapshot, ordinal?: number, ordinalTotal?: number): string {
  const busy = operations.has(`stop:${service.id}`);
  const uptime = currentUptime(service);
  const pip = busy ? "busy" : uptime === null ? "idle" : service.status === "limited" ? "limited" : "running";
  const profileBadges = springProfileBadges(service);
  return `
    <article class="service-tile category-${service.category}${busy ? " is-busy" : ""}" data-metrics-id="${h(service.id)}" aria-label="${h(service.display_name)} service">
      <button class="tile-details-button" type="button" data-tile-action data-action="service-details" data-service-id="${h(service.id)}" aria-label="View ${h(service.display_name)} details" title="View details"></button>
      <div class="tile-top">
        ${renderTileOrdinal(ordinal, ordinalTotal)}
        <span class="icon-well" aria-hidden="true">${techIcon(service.tech, 44)}<span class="status-pip state-${pip}"></span></span>
        ${renderTileHeading(serviceTitle(service), techLabel(service.tech), `${originBadge(service.origin_kind, service.origin_label)}${profileBadges}`)}
        <span class="details-hint" aria-hidden="true">${uiIcon("details", 14)}</span>
        ${service.can_terminate ? `<button type="button" class="stop-button" data-tile-action data-action="stop-service" data-service-id="${h(service.id)}" aria-label="${busy ? "Stopping" : "Stop"} ${h(service.display_name)}" title="${busy ? "Stopping process" : "Stop process"}" ${busy ? "disabled" : ""}><span class="stop-glyph" aria-hidden="true"></span></button>` : ""}
      </div>
      <div class="tile-metrics">
        <span class="metric metric-uptime${uptime !== null && uptime < FRESH_UPTIME_SECONDS ? " is-fresh" : ""}" data-metric="uptime">${uiIcon("clock", 13)}<span class="sr-only">Uptime </span><span data-metric-text>${h(uptimeText(service, busy))}</span></span>
        <span class="metric metric-memory" data-metric="memory">${uiIcon("memory", 13)}<span class="sr-only">Memory </span><span data-metric-text>${h(formatBytes(service.process?.memory_bytes ?? null))}</span></span>
      </div>
      ${renderTileFoot(uniquePorts(service), "No port information", service.browser_url
        ? `<button type="button" class="service-link" data-tile-action data-action="open-service" data-service-id="${h(service.id)}" aria-label="Open ${h(service.display_name)} in the browser" title="${h(service.browser_url)}"><span>${h(browserLinkLabel(service.browser_url))}</span>${uiIcon("external", 12)}</button>`
        : "")}
    </article>`;
}

function renderTileOrdinal(ordinal?: number, total?: number): string {
  if (!ordinal || !total || total < 2) return "";
  return `<span class="tile-ordinal" title="Item ${ordinal} of ${total}" aria-label="Item ${ordinal} of ${total}">${ordinal}</span>`;
}

// The subtitle carries the technology once: skip the label when the title already says it.
function renderTileHeading(title: string, label: string, badge: string): string {
  const tech = label.toLowerCase() === title.trim().toLowerCase() ? "" : `<span class="tech-label">${h(label)}</span>`;
  return `<div class="tile-heading">
        <h3 class="tile-name" title="${h(title)}">${h(title)}</h3>
        ${tech || badge ? `<p class="tile-subtitle">${tech}${badge}</p>` : ""}
      </div>`;
}

function originBadge(kind: string, label: string | null): string {
  if (!label || kind === "system" || kind === "unknown") return "";
  return `<span class="origin-badge origin-${h(kind)}" title="Started from ${h(label)}" aria-label="Started from ${h(label)}">${uiIcon(originIcon(kind, label), 12)}${h(label)}</span>`;
}

function springProfileBadges(service: ServiceSnapshot): string {
  if (service.tech.trim().toLowerCase() !== "spring") return "";
  return service.active_profiles
    .map((profile) => profile.trim())
    .filter(Boolean)
    .map((profile) => `<span class="profile-badge" title="Active Spring profile: ${h(profile)}" aria-label="Active Spring profile: ${h(profile)}">${h(profile)}</span>`)
    .join("");
}

function originIcon(kind: string, label: string): UiIconName {
  const value = label.toLowerCase();
  if (value.includes("claude")) return "claude";
  if (value.includes("copilot")) return "copilot";
  if (value.includes("cursor")) return "cursor";
  if (["intellij", "pycharm", "webstorm", "rider", "goland", "jetbrains"].some((name) => value.includes(name))) return "jetbrains";
  if (kind === "agent") return "bot";
  return kind === "ide" ? "ide" : "terminal";
}

function renderTileFoot(ports: number[], emptyLabel: string, trailing: string): string {
  const labels = portBadgeLabels(ports);
  return `<div class="tile-foot">
        <div class="port-row">
          ${labels.map((label) => `<span class="port-chip${label.startsWith("+") ? " port-overflow" : ""}" title="${h(portChipDescription(label, ports))}" aria-label="${h(portChipDescription(label, ports))}">${h(label)}</span>`).join("")}
          ${ports.length === 0 ? `<span class="no-port-label">${h(emptyLabel)}</span>` : ""}
        </div>
        ${trailing}
      </div>`;
}

function uptimeText(service: ServiceSnapshot, busy: boolean): string {
  if (busy) return "Stopping…";
  const uptime = currentUptime(service);
  return uptime === null ? "—" : formatUptimeCompact(uptime);
}

function renderDocker(force = false): void {
  captureDockerConsoleState();
  const fallback = workspace?.services.filter((service) => service.relevance === "container") ?? [];
  const containerActions = containerListing?.containers.map((container) => [container.id, containerOperationBusy(container.id)]) ?? [];
  const signature = JSON.stringify([
    containerListing,
    containerActions,
    selectedContainerId,
    dockerLogState,
    fallback.map((service) => [service.id, uniquePorts(service)])
  ]);
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
      workspaceElement.innerHTML = `<div class="inline-notice"><strong>Docker could not be queried.</strong><span>${h(containerListing.message ?? "Docker is unavailable.")}</span></div><div class="board"><section class="service-section" data-tiles="${fallback.length}"><header class="section-header"><span class="section-accent accent-container"></span><h2>CONTAINER LISTENERS</h2><span class="section-count">${fallback.length}</span></header><div class="tile-grid">${fallback.map(renderServiceTile).join("")}</div></section></div>`;
      applyBoardLayout();
      restoreDockerConsoleScroll();
      return;
    }
    workspaceElement.innerHTML = `<div class="docker-view">${emptyState("Docker is unavailable", containerListing.message ?? "The Docker CLI could not be queried.")}${renderDockerConsole()}</div>`;
    restoreDockerConsoleScroll();
    return;
  }
  if (containerListing.containers.length === 0) {
    workspaceElement.innerHTML = `<div class="docker-view">${emptyState("No containers found", containerListing.message ?? "Docker returned an empty list.")}${renderDockerConsole()}</div>`;
    restoreDockerConsoleScroll();
    return;
  }
  const groups = groupContainers(containerListing.containers);
  workspaceElement.innerHTML = `<div class="docker-view"><div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.containers.length}" aria-labelledby="container-group-${h(encodeURIComponent(group.name))}">
      <header class="section-header"><span class="section-accent accent-container"></span><h2 id="container-group-${h(encodeURIComponent(group.name))}">${renderGroupTitle(group.name, group.containers.length, "container-group-details", `data-group-name="${h(group.name)}"`, group.name.toUpperCase())}</h2></header>
      <div class="tile-grid">${group.containers.map((container, index) => renderContainerTile(container, index + 1, group.containers.length, true)).join("")}</div>
    </section>`).join("")}</div>${renderDockerConsole()}</div>`;
  applyBoardLayout();
  restoreDockerConsoleScroll();
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

function renderContainerTile(container: ContainerInfo, ordinal?: number, ordinalTotal?: number, showActions = false): string {
  const running = container.state === "running";
  const busy = showActions && containerOperationBusy(container.id);
  const selected = showActions && selectedContainerId === container.id;
  const action = running ? "stop-container" : "start-container";
  const actionLabel = busy ? (running ? "Stopping" : "Starting") : running ? "Stop" : "Start";
  const stateText = busy ? `${actionLabel}…` : container.status || (running ? "Running" : "Stopped");
  const actionButton = showActions
    ? running
      ? `<button type="button" class="stop-button container-action" data-tile-action data-action="${action}" data-container-id="${h(container.id)}" aria-label="${actionLabel} ${h(container.name)}" title="${actionLabel} container" ${busy ? "disabled aria-busy=\"true\"" : ""}><span class="stop-glyph" aria-hidden="true"></span></button>`
      : `<button type="button" class="start-action icon-only-button container-action" data-tile-action data-action="${action}" data-container-id="${h(container.id)}" aria-label="${actionLabel} ${h(container.name)}" title="${actionLabel} container" ${busy ? "disabled aria-busy=\"true\"" : ""}>${uiIcon(busy ? "refresh" : "play", 16)}</button>`
    : "";
  const detailsButton = showActions
    ? `<button type="button" class="info-button icon-only-button container-details-button" data-tile-action data-action="container-details" data-container-id="${h(container.id)}" aria-label="View ${h(container.name)} details" title="View container details">${uiIcon("info", 15)}</button>`
    : `<button class="tile-details-button" type="button" data-tile-action data-action="container-details" data-container-id="${h(container.id)}" aria-label="View ${h(container.name)} details" title="View details"></button>`;
  const selectionAttributes = showActions
    ? ` data-action="select-container" data-container-id="${h(container.id)}" tabindex="0" role="button" aria-pressed="${selected ? "true" : "false"}"`
    : "";
  return `
    <article class="service-tile container-tile ${running ? "is-running" : "is-stopped"}${selected ? " is-selected" : ""}${busy ? " is-busy" : ""}" aria-label="${h(container.name)} container"${selectionAttributes}${busy ? " aria-busy=\"true\"" : ""}>
      ${showActions ? "" : detailsButton}
      <div class="tile-top">
        ${renderTileOrdinal(ordinal, ordinalTotal)}
        <span class="icon-well" aria-hidden="true">${techIcon(imageTech(container.image), 44)}<span class="status-pip state-${busy ? "busy" : running ? "running" : "idle"}"></span></span>
        ${renderTileHeading(container.name, "", "")}
        ${showActions ? detailsButton : ""}
        ${actionButton}
      </div>
      <div class="tile-metrics">
        <span class="metric metric-state ${busy ? "is-busy" : running ? "is-running" : "is-stopped"}" title="Container state">${uiIcon("docker", 13)}<span class="sr-only">State </span>${h(ellipsis(stateText, 30))}</span>
      </div>
      ${renderTileFoot(container.ports, "No published ports", `<span class="image-label" title="Container image: ${h(container.image)}">${h(ellipsis(container.image, 24))}</span>`)}
    </article>`;
}

function containerOperationBusy(id: string): boolean {
  return operations.has(`container:${id}`);
}

function renderDockerConsole(): string {
  const container = selectedContainerId
    ? containerListing?.containers.find((item) => item.id === selectedContainerId) ?? null
    : null;
  const stateClass = container ? containerStateClass(container) : "idle";
  const statusText = container ? containerStateText(container) : "No container selected";
  const title = container?.name ?? "Container console";
  const subtitle = container ? container.image : "Select a container to view its recent logs";
  const logMeta = !container
    ? ""
    : dockerLogState.loading
      ? `<span class="console-meta-item" title="Loading recent logs">${uiIcon("refresh", 12)}<span>Loading logs</span></span>`
      : dockerLogState.error
        ? `<span class="console-meta-item console-meta-error" title="${h(dockerLogState.error)}">${uiIcon("warning", 12)}<span>Logs unavailable</span></span>`
        : `<span class="console-meta-item" title="The backend returns the most recent 200 lines">${uiIcon("log", 12)}<span>Last 200 lines</span></span>`;
  const context = container
    ? `<details class="console-context-details"><summary aria-label="Show container details" title="Show container details">${uiIcon("info", 14)}<span class="sr-only">Container details</span></summary><div class="console-context" aria-label="Container context"><div class="console-context-item"><span>CONTAINER ID</span><code title="${h(container.id)}">${h(container.id)}</code></div><div class="console-context-item"><span>IMAGE</span><code title="${h(container.image)}">${h(middleEllipsis(container.image, 240))}</code></div><div class="console-context-item"><span>COMPOSE SERVICE</span><code title="${h(container.compose_service ?? "Standalone container")}">${h(container.compose_service ?? "Standalone container")}</code></div><div class="console-context-item"><span>COMPOSE PROJECT</span><code title="${h(container.compose_project ?? "—")}">${h(container.compose_project ?? "—")}</code></div></div></details>`
    : "";
  const disabled = container ? "" : " disabled";
  const outputLabel = container ? `Logs for ${h(container.name)}` : "Docker container logs";
  return `<section class="launch-console docker-console${container ? ` state-${stateClass}` : " launch-console-empty"}" aria-labelledby="docker-console-title" data-console-container-id="${h(container?.id ?? "")}">
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${stateClass}" aria-hidden="true">${uiIcon("docker", 16)}</span><div><h2 id="docker-console-title">${h(title)}</h2><p>${h(subtitle)}</p></div></div>
      <div class="console-meta" aria-label="Container status"><span class="console-state state-${stateClass}"><span class="task-state-dot" aria-hidden="true"></span>${h(statusText)}</span>${container ? `<span class="console-meta-item" title="Container ID"><span class="sr-only">Container ID </span>${uiIcon("docker", 12)}<code>${h(middleEllipsis(container.id, 16))}</code></span>` : ""}${logMeta}</div>
    </header>
    ${context}
    <div class="console-toolbar"><span class="console-toolbar-label" title="Recent logs">${uiIcon("log", 14)}<span>Recent logs</span></span><div class="console-tools"><button class="console-tool icon-only-button${consoleWrap ? " is-active" : ""}" type="button" data-action="toggle-log-wrap" aria-pressed="${consoleWrap ? "true" : "false"}" aria-label="${consoleWrap ? "Disable line wrapping" : "Wrap long lines"}" title="${consoleWrap ? "Disable line wrapping" : "Wrap long lines"}"${disabled}>${uiIcon("chevronDown", 13)}</button><button class="console-tool icon-only-button${consoleFollow ? " is-active" : ""}" type="button" data-action="toggle-log-follow" aria-pressed="${consoleFollow ? "true" : "false"}" aria-label="${consoleFollow ? "Pause following new output" : "Follow new output"}" title="${consoleFollow ? "Pause following new output" : "Follow new output"}"${disabled}>${uiIcon(consoleFollow ? "refresh" : "play", 13)}<span class="sr-only" data-console-follow-label>${consoleFollow ? "Following" : "Follow output"}</span></button></div></div>
    <div class="console-output${consoleWrap ? " is-wrapped" : ""} docker-console-output" tabindex="0" role="log" aria-live="polite" aria-label="${outputLabel}">${renderDockerLogOutput(container)}</div>
  </section>`;
}

function renderDockerLogOutput(container: ContainerInfo | null): string {
  if (!container) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("docker", 18)}</span><strong>No container selected</strong><span>Select a Docker container card to view its recent logs.</span></div>`;
  }
  const log = dockerLogState.containerId === container.id ? dockerLogState.logs : "";
  if (dockerLogState.loading) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("refresh", 18)}</span><strong>Loading container logs</strong><span>Fetching the most recent output from Docker…</span></div>`;
  }
  if (dockerLogState.error) {
    const alert = `<div class="console-alert">${uiIcon("warning", 14)}<span>${h(dockerLogState.error)}</span></div>`;
    if (log.trim()) return `${alert}<pre class="console-log">${h(log)}</pre>`;
    return `${alert}<div class="console-message is-failed"><span class="console-message-icon">${uiIcon("warning", 18)}</span><strong>Container logs are unavailable</strong><span>Docker could not return output for this container.</span></div>`;
  }
  if (log.trim()) return `<pre class="console-log">${h(log)}</pre>`;
  return `<div class="console-message"><span class="console-message-icon">${uiIcon("log", 18)}</span><strong>No logs available</strong><span>This container has not produced any output yet.</span></div>`;
}

function containerStateClass(container: ContainerInfo): "running" | "starting" | "stopped" {
  const state = container.state.trim().toLowerCase();
  if (state === "running") return "running";
  if (["created", "restarting", "paused"].includes(state)) return "starting";
  return "stopped";
}

function containerStateText(container: ContainerInfo): string {
  return container.status.trim() || (container.state.trim() ? container.state : "Unknown");
}

function renderLaunch(force = false): void {
  captureConsoleState();
  const signature = JSON.stringify([profiles, taskSnapshots, appInfo?.demo, [...operations]]);
  if (!force && signature === launchSignature && workspaceElement.dataset.view === "launch") return;
  launchSignature = signature;
  workspaceElement.dataset.view = "launch";
  const header = `<div class="launch-heading"><div><h1>Launch Profiles</h1><p>Run backend, frontend, and auto-build tasks together.</p></div><button class="primary-button icon-button-label" type="button" data-action="add-profile" ${appInfo?.demo ? "disabled" : ""}>${uiIcon("plus", 14)} Add</button></div>`;
  if (profiles.length === 0) {
    selectedTaskKey = null;
    workspaceElement.innerHTML = `${header}<div class="launch-empty"><h2>No launch profiles yet</h2><p>Add a project and run commands to start and stop them together without an IDE.</p><button class="secondary-button" type="button" data-action="add-profile" ${appInfo?.demo ? "disabled" : ""}>Add First Profile</button></div>`;
    return;
  }
  const selected = ensureSelectedTask();
  workspaceElement.innerHTML = `<div class="launch-view">${header}<div class="launch-list">${profiles.map(renderProfile).join("")}</div>${renderLaunchConsole(selected)}</div>`;
  restoreConsoleScroll();
}

function renderProfile(profile: LaunchProfile): string {
  const snapshots = profile.tasks.map((task) => snapshotFor(profile.id, task.name));
  const canStop = snapshots.some((snapshot) => snapshot && ["starting", "running", "stopping"].includes(snapshot.state));
  const canStart = profile.tasks.some((task) => !["starting", "running", "stopping", "external"].includes(snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  const activeCount = snapshots.filter((snapshot) => snapshot && ["starting", "running", "stopping"].includes(snapshot.state)).length;
  const externalCount = snapshots.filter((snapshot) => snapshot?.state === "external").length;
  const failedCount = snapshots.filter((snapshot) => snapshot?.state === "failed").length;
  const profileStatus = failedCount > 0 ? `${failedCount} failed` : activeCount > 0 ? `${activeCount} active` : externalCount > 0 ? `${externalCount} external` : "Ready";
  const profileStatusClass = failedCount > 0 ? "is-failed" : activeCount > 0 || externalCount > 0 ? "is-active" : "is-idle";
  const primary = canStart
    ? `<button class="primary-button icon-button-label" type="button" data-action="start-profile" data-profile-id="${h(profile.id)}">${uiIcon("play", 14)} ${canStop ? "Start Remaining" : "Start All"}</button>`
    : canStop ? `<button class="secondary-button warning-action icon-button-label" type="button" data-action="stop-profile" data-profile-id="${h(profile.id)}">${uiIcon("stop", 14)} Stop All</button>` : "";
  return `<section class="launch-profile service-section" aria-labelledby="launch-profile-${h(profile.id)}">
    <header class="section-header launch-profile-header">
      <span class="section-accent accent-runtime" aria-hidden="true"></span>
      <div class="launch-profile-heading"><h2 id="launch-profile-${h(profile.id)}">${h(profile.name)}</h2><p title="${h(profile.project_root)}">${h(shortenPath(profile.project_root))}</p></div>
      <span class="launch-profile-status ${profileStatusClass}" aria-label="Profile status: ${h(profileStatus)}">${h(profileStatus)}</span>
      <span class="section-count" aria-label="${profile.tasks.length} ${profile.tasks.length === 1 ? "task" : "tasks"}">${profile.tasks.length}</span>
      <div class="section-actions launch-profile-actions">
        ${primary}
        <button class="section-action" type="button" data-action="edit-profile" data-profile-id="${h(profile.id)}" ${appInfo?.demo || canStop ? "disabled" : ""}>Edit</button>
        <button class="section-action danger-button" type="button" data-action="delete-profile" data-profile-id="${h(profile.id)}" ${appInfo?.demo || canStop ? "disabled" : ""}>Delete</button>
      </div>
    </header>
    <div class="task-list" role="list" aria-label="Tasks in ${h(profile.name)}">${profile.tasks.map((task) => renderTask(profile, task)).join("")}</div>
  </section>`;
}

function renderTask(profile: LaunchProfile, task: LaunchTask): string {
  const snapshot = snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  const active = ["starting", "running", "stopping"].includes(state);
  const external = state === "external";
  const busy = operations.has(`task:${profile.id}:${task.name}`);
  const selected = selectedTaskKey === launchTaskKey(profile.id, task.name);
  return `<article class="task-row state-${state}${selected ? " is-selected" : ""}" data-action="select-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" tabindex="0" role="listitem" aria-current="${selected ? "true" : "false"}" aria-label="${h(`${profile.name} · ${task.name}, ${stateLabel(state)}${task.expected_port ? `, port ${task.expected_port}` : ""}`)}"><div class="task-copy">
    <div class="task-heading-line"><h3>${h(middleEllipsis(task.name, 72))}</h3><span class="task-state state-${state}"><span class="task-state-dot" aria-hidden="true"></span>${h(stateLabel(state))}</span></div>
    <div class="task-actions-row"><div class="task-port-group">${task.expected_port ? `<span class="task-port">localhost:${task.expected_port}</span>` : ""}</div><div>${active ? `<button class="quiet-button warning-action icon-button-label" type="button" data-action="stop-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" ${busy ? "disabled" : ""}>${uiIcon("stop", 13)} Stop</button>` : !external ? `<button class="quiet-button start-action icon-button-label" type="button" data-action="start-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" ${busy ? "disabled" : ""}>${uiIcon("play", 13)} Start</button>` : ""}</div></div>
  </div></article>`;
}

function launchTaskKey(profileId: string, taskName: string): string {
  return `${profileId}\0${taskName}`;
}

// Keep the NUL-delimited key for in-memory identity, but never put it in HTML:
// HTML parsers normalize NUL characters in attribute values.
function launchTaskDomKey(profileId: string, taskName: string): string {
  return `${encodeURIComponent(profileId)}:${encodeURIComponent(taskName)}`;
}

function launchTaskRefs(): LaunchTaskRef[] {
  return profiles.flatMap((profile) => profile.tasks.map((task) => ({ profile, task })));
}

function launchTaskRefForKey(key: string | null): LaunchTaskRef | null {
  if (!key) return null;
  return launchTaskRefs().find(({ profile, task }) => launchTaskKey(profile.id, task.name) === key) ?? null;
}

function selectedTaskDomKey(): string | null {
  const ref = launchTaskRefForKey(selectedTaskKey);
  return ref ? launchTaskDomKey(ref.profile.id, ref.task.name) : null;
}

function ensureSelectedTask(): LaunchTaskRef | null {
  const existing = launchTaskRefForKey(selectedTaskKey);
  if (existing) return existing;
  const refs = launchTaskRefs();
  const active = refs
    .map((ref, index) => ({ ref, index, state: snapshotFor(ref.profile.id, ref.task.name)?.state ?? "stopped" as LaunchState }))
    .filter(({ state }) => ["running", "starting", "stopping", "external"].includes(state))
    .sort((a, b) => launchStatePriority(a.state) - launchStatePriority(b.state) || a.index - b.index)[0]?.ref;
  const next = active ?? refs[0] ?? null;
  selectedTaskKey = next ? launchTaskKey(next.profile.id, next.task.name) : null;
  return next;
}

function launchStatePriority(state: LaunchState): number {
  return ({ running: 0, starting: 1, stopping: 2, external: 3, failed: 4, stopped: 5 })[state];
}

function renderLaunchConsole(ref: LaunchTaskRef | null): string {
  if (!ref) {
    return `<section class="launch-console launch-console-empty" aria-labelledby="launch-console-title"><header class="console-header"><div class="console-title"><span class="console-icon" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="launch-console-title">Task console</h2><p>No task selected</p></div></div></header><div class="console-output console-empty-output" role="status"><div class="console-message"><strong>No tasks available</strong><span>Add at least one task to this launch profile.</span></div></div></section>`;
  }
  const { profile, task } = ref;
  const snapshot = snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  const pid = state === "external" ? snapshot?.external_pid ?? snapshot?.main_pid ?? null : snapshot?.main_pid ?? null;
  const port = task.expected_port;
  const command = task.command || "No command configured";
  const cwd = state === "external" ? snapshot?.external_working_directory || task.cwd : task.cwd;
  const externalLogPath = state === "external" ? snapshot?.external_log_path : null;
  return `<section class="launch-console state-${state}" aria-labelledby="launch-console-title" data-console-task-key="${h(launchTaskDomKey(profile.id, task.name))}">
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${state}" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="launch-console-title">${h(task.name)}</h2><p>${h(profile.name)}</p></div></div>
      <div class="console-meta" aria-label="Task status"><span class="console-state state-${state}"><span class="task-state-dot" aria-hidden="true"></span>${h(stateLabel(state))}</span>${pid !== null ? `<span class="console-meta-item"><span>PID</span><code>${pid}</code></span>` : ""}${port ? `<span class="console-meta-item"><span>PORT</span><code>localhost:${port}</code></span>` : ""}${externalLogPath ? `<span class="console-meta-item console-meta-source" title="External log source: ${h(externalLogPath)}"><span>LOG</span><code>${h(middleEllipsis(externalLogPath, 52))}</code></span>` : ""}</div>
    </header>
    <details class="console-context-details"><summary>${uiIcon("details", 13)} Task details</summary><div class="console-context" aria-label="Task context"><div class="console-context-item"><span>COMMAND</span><code title="${h(task.command)}">${h(middleEllipsis(command, 240))}</code></div><div class="console-context-item"><span>WORKING DIRECTORY</span><code title="${h(cwd)}">${h(middleEllipsis(cwd, 240))}</code></div></div></details>
    <div class="console-toolbar"><span class="console-toolbar-label">${uiIcon("log", 13)} Output</span><div class="console-tools"><button class="console-tool${consoleWrap ? " is-active" : ""}" type="button" data-action="toggle-log-wrap" aria-pressed="${consoleWrap ? "true" : "false"}" title="${consoleWrap ? "Disable line wrapping" : "Wrap long lines"}">${uiIcon("chevronDown", 12)}<span>Wrap</span></button><button class="console-tool${consoleFollow ? " is-active" : ""}" type="button" data-action="toggle-log-follow" aria-pressed="${consoleFollow ? "true" : "false"}" title="${consoleFollow ? "Pause following new output" : "Follow new output"}">${uiIcon(consoleFollow ? "refresh" : "play", 12)}<span data-console-follow-label>${consoleFollow ? "Following" : "Follow output"}</span></button></div></div>
    <div class="console-output${consoleWrap ? " is-wrapped" : ""}" tabindex="0" role="log" aria-live="polite" aria-label="Output for ${h(task.name)}">${renderConsoleOutput(snapshot, state)}</div>
  </section>`;
}

function renderConsoleOutput(snapshot: ManagedTaskSnapshot | undefined, state: LaunchState): string {
  const log = snapshot?.log_tail ?? "";
  if (state === "external") {
    if (log.length > 0) return `<pre class="console-log">${h(log)}</pre>`;
    const message = snapshot?.external_log_path
      ? "No output is available from the configured external log source yet."
      : "External process output is not captured.";
    return `<div class="console-message is-external"><span class="console-message-icon">${uiIcon("terminal", 18)}</span><strong>${snapshot?.external_log_path ? "Waiting for external output" : "Output unavailable"}</strong><span>${message}</span></div>`;
  }
  const message = snapshot?.message?.trim() ?? "";
  const notice = state === "failed"
    ? `<div class="console-alert">${uiIcon("warning", 14)}<span>${h(message || "The task exited before completing successfully.")}</span></div>`
    : "";
  if (log.length > 0) return `${notice}<pre class="console-log">${h(log)}</pre>`;
  if (state === "failed") {
    return `<div class="console-message is-failed"><span class="console-message-icon">${uiIcon("warning", 18)}</span><strong>Task failed before producing output</strong><span>${h(message || "The task exited before completing successfully.")}</span></div>`;
  }
  const copy = state === "starting"
    ? "Waiting for the task to produce output…"
    : state === "running"
      ? "No output has been captured yet."
      : state === "stopping"
        ? "Stopping task…"
        : "No output has been captured for this task.";
  return `${notice}<div class="console-message"><span class="console-message-icon">${uiIcon("log", 18)}</span><strong>${h(copy)}</strong><span>${state === "stopped" ? "Start the task to stream its output here." : "New output will appear here automatically."}</span></div>`;
}

function captureConsoleState(): void {
  const output = document.querySelector<HTMLElement>(".launch-console:not(.docker-console) .console-output");
  if (!output) return;
  const taskKey = output.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
  if (taskKey !== selectedTaskDomKey()) return;
  consoleScrollTaskKey = selectedTaskKey;
  consoleScrollTop = output.scrollTop;
}

function restoreConsoleScroll(): void {
  const output = document.querySelector<HTMLElement>(".launch-console:not(.docker-console) .console-output");
  if (!output) return;
  restoringConsoleScroll = true;
  const savedScrollTop = consoleScrollTaskKey === selectedTaskKey ? consoleScrollTop : 0;
  output.scrollTop = consoleFollow ? output.scrollHeight : Math.min(savedScrollTop, output.scrollHeight);
  consoleScrollTaskKey = selectedTaskKey;
  consoleScrollTop = output.scrollTop;
  window.setTimeout(() => { restoringConsoleScroll = false; }, 0);
}

function captureDockerConsoleState(): void {
  const output = document.querySelector<HTMLElement>(".docker-console .console-output");
  if (!output) return;
  const containerId = output.closest<HTMLElement>(".docker-console")?.dataset.consoleContainerId || null;
  if (containerId !== selectedContainerId) return;
  dockerConsoleScrollContainerId = selectedContainerId;
  dockerConsoleScrollTop = output.scrollTop;
}

function restoreDockerConsoleScroll(): void {
  const output = document.querySelector<HTMLElement>(".docker-console .console-output");
  if (!output) return;
  restoringDockerConsoleScroll = true;
  const savedScrollTop = dockerConsoleScrollContainerId === selectedContainerId ? dockerConsoleScrollTop : 0;
  output.scrollTop = consoleFollow ? output.scrollHeight : Math.min(savedScrollTop, output.scrollHeight);
  dockerConsoleScrollContainerId = selectedContainerId;
  dockerConsoleScrollTop = output.scrollTop;
  window.setTimeout(() => { restoringDockerConsoleScroll = false; }, 0);
}

function handleConsoleScroll(event: Event): void {
  const target = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-output") : null;
  if (!target) return;
  const dockerConsole = target.closest<HTMLElement>(".docker-console");
  if (dockerConsole) {
    const containerId = dockerConsole.dataset.consoleContainerId || null;
    if (containerId !== selectedContainerId) return;
    dockerConsoleScrollContainerId = selectedContainerId;
    dockerConsoleScrollTop = target.scrollTop;
    if (restoringDockerConsoleScroll) return;
    const distanceFromEnd = target.scrollHeight - target.clientHeight - target.scrollTop;
    consoleFollow = distanceFromEnd <= 18;
    updateConsoleControls();
    return;
  }
  const taskKey = target.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
  if (taskKey !== selectedTaskDomKey()) return;
  consoleScrollTaskKey = selectedTaskKey;
  consoleScrollTop = target.scrollTop;
  if (restoringConsoleScroll) return;
  const distanceFromEnd = target.scrollHeight - target.clientHeight - target.scrollTop;
  consoleFollow = distanceFromEnd <= 18;
  updateConsoleControls();
}

function updateConsoleControls(): void {
  const button = document.querySelector<HTMLButtonElement>("[data-action='toggle-log-follow']");
  if (!button) return;
  button.classList.toggle("is-active", consoleFollow);
  button.setAttribute("aria-pressed", String(consoleFollow));
  button.title = consoleFollow ? "Pause following new output" : "Follow new output";
  const icon = button.querySelector<HTMLElement>(".ui-icon");
  if (icon) icon.outerHTML = uiIcon(consoleFollow ? "refresh" : "play", 13);
  const label = button.querySelector<HTMLElement>("[data-console-follow-label]");
  if (label) label.textContent = consoleFollow ? "Following" : "Follow output";
}

function selectTask(profileId: string, taskName: string, focus = false): void {
  const ref = launchTaskRefs().find(({ profile, task }) => profile.id === profileId && task.name === taskName);
  if (!ref) throw new Error("The launch task no longer exists.");
  setSelectedTask(profileId, taskName);
  renderLaunch(true);
  if (focus) focusTaskRow(profileId, taskName);
}

function setSelectedTask(profileId: string, taskName: string): void {
  const key = launchTaskKey(profileId, taskName);
  if (selectedTaskKey === key) return;
  selectedTaskKey = key;
  consoleScrollTaskKey = key;
  consoleScrollTop = 0;
}

function focusTaskRow(profileId: string, taskName: string): void {
  const row = [...document.querySelectorAll<HTMLElement>(".task-row")].find((item) => item.dataset.profileId === profileId && item.dataset.taskName === taskName);
  row?.focus();
}

function selectContainer(id: string, focus = false): void {
  findContainer(id);
  selectedContainerId = id;
  dockerConsoleScrollContainerId = id;
  dockerConsoleScrollTop = 0;
  dockerLogState = { containerId: id, logs: "", loading: true, error: null };
  renderDocker(true);
  void loadContainerLogs(id, true);
  if (focus) focusContainerCard(id);
}

function focusContainerCard(id: string): void {
  const card = [...document.querySelectorAll<HTMLElement>(".container-tile[data-action='select-container']")]
    .find((item) => item.dataset.containerId === id);
  card?.focus();
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
    if (tab === "services" || tab === "docker") await refreshContainers(true);
    if (tab === "launch") await refreshLaunch(true);
    return;
  }
  const action = target.dataset.action;
  if (!action) return;
  try {
    if (action === "service-details") showServiceDetails(findService(required(target.dataset.serviceId)));
    else if (action === "open-service") await openService(required(target.dataset.serviceId));
    else if (action === "stop-service") await requestStopService(required(target.dataset.serviceId));
    else if (action === "confirm-stop-service") await confirmStopService(required(target.dataset.serviceId));
    else if (action === "save-service-group") requestSaveGroup(required(target.dataset.groupId));
    else if (action === "confirm-save-service-group") await confirmSaveGroup(required(target.dataset.groupId));
    else if (action === "stop-service-group") requestStopGroup(required(target.dataset.groupId));
    else if (action === "confirm-stop-service-group") await confirmStopGroup(required(target.dataset.groupId));
    else if (action === "group-details") showGroupDetails(groupForId(required(target.dataset.groupId)));
    else if (action === "container-group-details") showContainerGroupDetails(required(target.dataset.groupName));
    else if (action === "select-container") selectContainer(required(target.dataset.containerId), false);
    else if (action === "start-container") await operateContainer(required(target.dataset.containerId), true);
    else if (action === "stop-container") await operateContainer(required(target.dataset.containerId), false);
    else if (action === "container-details") showContainerDetails(findContainer(required(target.dataset.containerId)));
    else if (action === "settings") showSettings();
    else if (action === "close-modal") closeModal();
    else if (action === "save-settings") await saveSettingsFromModal();
    else if (action === "open-source") await openUrl(SOURCE_URL);
    else if (action === "add-profile") showProfileEditor(null);
    else if (action === "edit-profile") showProfileEditor(findProfile(required(target.dataset.profileId)));
    else if (action === "save-profile") await saveProfileFromModal(target.dataset.profileId ?? null);
    else if (action === "delete-profile") requestDeleteProfile(required(target.dataset.profileId));
    else if (action === "confirm-delete-profile") await confirmDeleteProfile(required(target.dataset.profileId));
    else if (action === "start-profile") await startProfile(required(target.dataset.profileId));
    else if (action === "stop-profile") await stopProfile(required(target.dataset.profileId));
    else if (action === "select-task") selectTask(required(target.dataset.profileId), required(target.dataset.taskName), true);
    else if (action === "start-task") await runTask(required(target.dataset.profileId), required(target.dataset.taskName), true);
    else if (action === "stop-task") await runTask(required(target.dataset.profileId), required(target.dataset.taskName), false);
    else if (action === "toggle-log-wrap") {
      consoleWrap = !consoleWrap;
      if (activeTab === "docker") renderDocker(true);
      else if (activeTab === "launch") renderLaunch(true);
    }
    else if (action === "toggle-log-follow") {
      consoleFollow = !consoleFollow;
      if (activeTab === "docker") renderDocker(true);
      else if (activeTab === "launch") renderLaunch(true);
    }
    else if (action === "choose-root") await chooseProfileRoot();
    else if (action === "add-task-row") addTaskRow();
    else if (action === "remove-task-row") target.closest(".task-editor")?.remove();
  } catch (error) {
    toast(messageOf(error), true);
  }
}

function handleKeyboard(event: KeyboardEvent): void {
  const taskRow = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>(".task-row") : null;
  if (taskRow && event.target === taskRow) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectTask(required(taskRow.dataset.profileId), required(taskRow.dataset.taskName), true);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const rows = [...document.querySelectorAll<HTMLElement>(".task-row")];
      const index = rows.indexOf(taskRow);
      const step = event.key === "ArrowDown" ? 1 : -1;
      rows[(index + step + rows.length) % rows.length]?.focus();
      event.preventDefault();
      return;
    }
  }
  const containerTile = event.target instanceof HTMLElement
    ? event.target.closest<HTMLElement>(".container-tile[data-action='select-container']")
    : null;
  if (containerTile && event.target === containerTile && (event.key === "Enter" || event.key === " ")) {
    event.preventDefault();
    selectContainer(required(containerTile.dataset.containerId), true);
    return;
  }
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

function requestStopService(id: string): void {
  if (pendingServiceStopId !== null || operations.has(`stop:${id}`)) return;
  const service = findService(id);
  if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
  pendingServiceStopId = id;
  openModal("Stop service?", `<p class="confirm-copy">Stop <strong>${h(service.display_name)}</strong>? This will terminate process PID <span class="mono">${service.process?.pid ?? "unknown"}</span>.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service" data-service-id="${h(id)}">${uiIcon("stop", 13)} Stop</button></div>`, { dismissOnBackdrop: false });
}

function requestSaveGroup(id: string): void {
  if (pendingGroupSaveId !== null || operations.has(`group-save:${id}`)) return;
  const group = groupForId(id);
  const tasks = validateGroupProfile(group);
  if (profileForGroup(group)) return;
  pendingGroupSaveId = id;
  openModal("Save launch profile?", `<p class="confirm-copy">Save <strong>${h(group.name)}</strong> as a launch profile with ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}?</p><p class="form-note mono">${h(group.path)}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button icon-button-label" type="button" data-action="confirm-save-service-group" data-group-id="${h(id)}">${uiIcon("folder", 13)} Save Profile</button></div>`, { dismissOnBackdrop: false });
}

async function confirmSaveGroup(id: string): Promise<void> {
  if (pendingGroupSaveId !== id) return;
  closeModal();
  await saveGroup(id);
}

async function saveGroup(id: string): Promise<void> {
  const group = groupForId(id);
  const key = `group-save:${id}`;
  if (operations.has(key)) return;
  const tasks = validateGroupProfile(group);
  if (profileForGroup(group)) return;
  operations.add(key);
  renderServices(true);
  try {
    profiles = await api.saveProfile({
      id: crypto.randomUUID().replaceAll("-", ""),
      name: group.name,
      project_root: group.path!,
      tasks
    });
    renderHeaderCounts();
    toast(`Launch profile saved for ${group.name}.`);
  } finally {
    operations.delete(key);
    renderServices(true);
  }
}

function requestStopGroup(id: string): void {
  if (pendingGroupStopId !== null || operations.has(`group-stop:${id}`)) return;
  const group = groupForId(id);
  const candidates = group.services.filter((service) => service.can_terminate);
  if (!candidates.length) throw new Error("No services in this group can be stopped safely.");
  pendingGroupStopId = id;
  openModal("Stop all services?", `<p class="confirm-copy">Stop <strong>${candidates.length} stoppable ${candidates.length === 1 ? "service" : "services"}</strong> in ${h(group.name)}? Services that cannot be terminated safely will remain untouched.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service-group" data-group-id="${h(id)}">${uiIcon("stop", 13)} Stop All</button></div>`, { dismissOnBackdrop: false });
}

async function confirmStopGroup(id: string): Promise<void> {
  if (pendingGroupStopId !== id) return;
  closeModal();
  await stopGroup(id);
}

async function stopGroup(id: string): Promise<void> {
  const group = groupForId(id);
  const candidates = group.services.filter((service) => service.can_terminate);
  if (!candidates.length) throw new Error("No services in this group can be stopped safely.");
  const key = `group-stop:${id}`;
  if (operations.has(key)) return;
  const pending = candidates.filter((service) => !operations.has(`stop:${service.id}`));
  if (!pending.length) {
    toast(`All stoppable services in ${group.name} are already stopping.`);
    return;
  }
  pending.forEach((service) => operations.add(`stop:${service.id}`));
  operations.add(key);
  renderServices(true);
  try {
    const results = await Promise.allSettled(pending.map((service) => api.terminate(service.id)));
    const successes = results.filter((result) => result.status === "fulfilled" && result.value.success).length;
    const failures = results.length - successes;
    toast(`${successes} of ${results.length} services in ${group.name} stopped${failures ? `; ${failures} could not be stopped.` : "."}`, failures > 0);
    await refreshWorkspace(true);
  } finally {
    pending.forEach((service) => operations.delete(`stop:${service.id}`));
    operations.delete(key);
    renderServices(true);
  }
}

function requestDeleteProfile(id: string): void {
  if (pendingProfileDeleteId !== null) return;
  const profile = findProfile(id);
  pendingProfileDeleteId = id;
  openModal("Delete launch profile?", `<p class="confirm-copy">Delete <strong>${h(profile.name)}</strong>? This removes its saved commands.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button" type="button" data-action="confirm-delete-profile" data-profile-id="${h(id)}">Delete</button></div>`, { dismissOnBackdrop: false });
}

async function confirmDeleteProfile(id: string): Promise<void> {
  if (pendingProfileDeleteId !== id) return;
  closeModal();
  await deleteProfile(id);
}

async function confirmStopService(id: string): Promise<void> {
  if (pendingServiceStopId !== id) return;
  closeModal();
  await stopService(id);
}

async function stopService(id: string): Promise<void> {
  const service = findService(id);
  if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
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

async function operateContainer(id: string, start: boolean): Promise<void> {
  findContainer(id);
  const key = `container:${id}`;
  if (operations.has(key)) return;
  operations.add(key);
  if (activeTab === "docker") renderDocker(true);
  try {
    const result = start ? await api.startContainer(id) : await api.stopContainer(id);
    toast(result.message, !result.success);
  } catch (error) {
    toast(messageOf(error), true);
  } finally {
    try {
      await refreshContainers(true);
    } finally {
      operations.delete(key);
      if (activeTab === "docker") renderDocker(true);
    }
  }
}

async function startProfile(id: string): Promise<void> {
  const profile = findProfile(id);
  const firstStartable = profile.tasks.find((task) => !["starting", "running", "stopping", "external"].includes(snapshotFor(id, task.name)?.state ?? "stopped"));
  if (firstStartable) setSelectedTask(id, firstStartable.name);
  for (const task of profile.tasks) {
    const state = snapshotFor(id, task.name)?.state ?? "stopped";
    if (!["starting", "running", "stopping", "external"].includes(state)) await runTask(id, task.name, true, false, false);
  }
  await refreshLaunch(true);
}

async function stopProfile(id: string): Promise<void> {
  taskSnapshots = mergeSnapshots(taskSnapshots, await api.stopProfile(id));
  await refreshLaunch(true);
}

async function runTask(profileId: string, taskName: string, start: boolean, refresh = true, select = true): Promise<void> {
  const key = `task:${profileId}:${taskName}`;
  if (operations.has(key)) return;
  if (select) setSelectedTask(profileId, taskName);
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

function showGroupDetails(group: ReturnType<typeof groupServices>[number]): void {
  const serviceItems = group.services.map((service) => `<li><strong>${h(serviceTitle(service) || service.display_name)}</strong><span>${h(techLabel(service.tech))}</span></li>`).join("");
  const containerItems = relatedContainersForGroup(group.services, containerListing?.available ? containerListing.containers : [])
    .map((container) => `<li><strong>${h(container.name)}</strong><span>${h(container.status || container.state)}</span></li>`).join("");
  openModal(group.name, `<div class="info-modal-copy"><p>Workspace details for this service group.</p>${group.path ? `<dl class="detail-grid"><dt>Project root</dt><dd class="mono">${h(group.path)}</dd></dl>` : ""}<h3 class="detail-heading">Services</h3><ul class="detail-list">${serviceItems || "<li><span>No services available</span></li>"}</ul>${containerItems ? `<h3 class="detail-heading">Docker</h3><ul class="detail-list">${containerItems}</ul>` : ""}</div><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showContainerGroupDetails(groupName: string): void {
  const containers = containerListing?.containers.filter((container) => (container.compose_project ?? "Standalone containers") === groupName) ?? [];
  openModal(groupName, `<div class="info-modal-copy"><p>Docker containers in this group.</p><ul class="detail-list">${containers.map((container) => `<li><strong>${h(container.name)}</strong><span>${h(container.status || container.state)}</span></li>`).join("") || "<li><span>No containers available</span></li>"}</ul></div><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showProfileDetails(profile: LaunchProfile): void {
  const taskItems = profile.tasks.map((task) => `<li><strong>${h(task.name)}</strong><span class="profile-task-meta"><code class="mono">${h(task.command || "No command configured")}</code>${task.expected_port ? `<span>Port ${task.expected_port}</span>` : ""}</span></li>`).join("");
  openModal(profile.name, `<div class="info-modal-copy"><p>Saved commands and project context for this launch profile.</p><dl class="detail-grid"><dt>Project root</dt><dd class="mono">${h(profile.project_root)}</dd><dt>Tasks</dt><dd>${profile.tasks.length}</dd></dl><h3 class="detail-heading">Commands</h3><ul class="detail-list">${taskItems || "<li><span>No tasks configured</span></li>"}</ul></div><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showInfo(kind: string): void {
  const copy: Record<string, { title: string; message: string }> = {
    launch: { title: "Launch Profiles", message: "Group related commands into one place. Select a task to view its output, then use the play and stop icons to control it." },
    appearance: { title: "Appearance", message: "Choose whether Cutting Board follows the system theme or always uses a light or dark appearance." },
    scanning: { title: "Scanning", message: "This controls how often Cutting Board refreshes the list of running local services. A longer interval uses less CPU." },
    privacy: { title: "Privacy", message: "Settings are stored locally on this device. Cutting Board does not collect telemetry or start automatically at login." }
  };
  const selected = copy[kind] ?? { title: "Information", message: "More information is available here." };
  openModal(selected.title, `<div class="info-modal-copy"><p>${h(selected.message)}</p></div><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function toggleSettingsInfo(button: HTMLElement): void {
  const targetId = required(button.dataset.infoTarget);
  const info = document.getElementById(targetId);
  if (!info) throw new Error("The settings explanation is unavailable.");
  const expanded = info.hidden;
  info.hidden = !expanded;
  button.setAttribute("aria-expanded", String(expanded));
}

function showServiceDetails(service: ServiceSnapshot): void {
  const process = service.process;
  const activeProfiles = service.tech.trim().toLowerCase() === "spring"
    ? service.active_profiles.map((profile) => profile.trim()).filter(Boolean)
    : [];
  openModal(service.display_name, `
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
    <h3 class="detail-heading">Listening endpoints</h3><div class="endpoint-list">${service.endpoints.map((endpoint) => `<div><span class="port-chip">${endpoint.port}</span><code>${h(endpoint.address)} · ${h(endpoint.family)} · ${h(endpoint.scope)}</code></div>`).join("")}</div>`);
}

function showContainerDetails(container: ContainerInfo): void {
  openModal(container.name, `<dl class="detail-grid"><dt>Container ID</dt><dd class="mono">${h(container.id)}</dd><dt>Image</dt><dd>${h(container.image)}</dd><dt>State</dt><dd>${h(container.state)}</dd><dt>Status</dt><dd>${h(container.status || "—")}</dd><dt>Compose project</dt><dd>${h(container.compose_project ?? "—")}</dd><dt>Compose service</dt><dd>${h(container.compose_service ?? "—")}</dd><dt>Compose working directory</dt><dd>${h(container.compose_working_dir ?? "—")}</dd><dt>Published ports</dt><dd>${container.ports.length ? container.ports.join(", ") : "—"}</dd></dl><div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>`);
}

function showSettings(): void {
  const intervalChoices = [...new Set([1000, 2000, 5000, 10000, 30000, settings.scan_interval_ms])].sort((a, b) => a - b);
  openModal("Settings", `<form id="settings-form" class="settings-form" onsubmit="return false">
    <section class="settings-section" aria-labelledby="appearance-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("theme", 18)}</span><div><h3 id="appearance-heading">Appearance</h3><p>Choose how Cutting Board looks on this device.</p></div></div>
      <fieldset class="choice-fieldset"><legend class="sr-only">Theme</legend><div class="theme-options">
        ${themeChoice("system", "System", "Follow your device")}${themeChoice("light", "Light", "Bright and clear")}${themeChoice("dark", "Dark", "Easy on the eyes")}
      </div></fieldset>
    </section>
    <section class="settings-section" aria-labelledby="scanning-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("scan", 18)}</span><div class="settings-heading-copy"><h3 id="scanning-heading">Scanning</h3><p>How often running local services are refreshed.</p></div><fieldset class="choice-fieldset interval-control"><legend class="sr-only">Scan interval</legend><div class="interval-options">
          ${intervalChoices.map((value) => `<label class="interval-choice"><input type="radio" name="scan_interval_ms" value="${value}" ${settings.scan_interval_ms === value ? "checked" : ""}><span>${h(formatInterval(value))}</span></label>`).join("")}
        </div></fieldset></div>
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
  profiles = await api.deleteProfile(id);
  renderHeaderCounts();
  renderLaunch(true);
  toast("Launch profile deleted.");
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
  pendingServiceStopId = null;
  pendingGroupSaveId = null;
  pendingGroupStopId = null;
  pendingProfileDeleteId = null;
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

function updateLiveMetrics(): void {
  if (!workspace) return;
  const services = new Map(workspace.services.map((service) => [service.id, service]));
  document.querySelectorAll<HTMLElement>("[data-metrics-id]").forEach((tile) => {
    const service = services.get(tile.dataset.metricsId ?? "");
    if (!service) return;
    const uptime = currentUptime(service);
    setMetricText(tile, "uptime", uptimeText(service, operations.has(`stop:${service.id}`)));
    setMetricText(tile, "memory", formatBytes(service.process?.memory_bytes ?? null));
    tile.querySelector(".metric-uptime")?.classList.toggle("is-fresh", uptime !== null && uptime < FRESH_UPTIME_SECONDS);
  });
}

function setMetricText(tile: HTMLElement, metric: string, value: string): void {
  const node = tile.querySelector<HTMLElement>(`[data-metric="${metric}"] [data-metric-text]`);
  if (node && node.textContent !== value) node.textContent = value;
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
