import "./styles.css";
import { open as choosePath } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import { techIcon, uiIcon } from "./icons";
import {
  FRESH_UPTIME_SECONDS,
  currentUptime,
  formatBytes,
  formatUptimeCompact,
  groupServices,
  imageTech,
  launchTasksEquivalent,
  middleEllipsis,
  pathIsEqualOrNested,
  portBadgeLabels,
  relatedContainersForGroup,
  serviceTitle,
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
        <button class="tab is-active" type="button" data-tab="services" aria-label="Services" title="Services">${uiIcon("power", 18)}<span class="tab-label">Services</span><span class="tab-count" id="services-count">0</span></button>
        <button class="tab" type="button" data-tab="docker" aria-label="Docker" title="Docker">${uiIcon("docker", 18)}<span class="tab-label">Docker</span><span class="tab-count" id="docker-count">0</span></button>
        <button class="tab" type="button" data-tab="launch" aria-label="Launch" title="Launch">${uiIcon("play", 18)}<span class="tab-label">Launch</span><span class="tab-count" id="launch-count">0</span></button>
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
type PendingLaunchAction =
  | { kind: "task"; direction: "start" | "stop"; profileId: string; taskName: string }
  | { kind: "profile"; direction: "start" | "stop"; profileId: string };
let pendingLaunchAction: PendingLaunchAction | null = null;
const operations = new Set<string>();
type LaunchTaskRef = { profile: LaunchProfile; task: LaunchTask };
let selectedTaskKey: string | null = null;
let consoleWrap = false;
let consoleFollow = true;
let consoleScrollTop = 0;
let consoleScrollTaskKey: string | null = null;
let restoringConsoleScroll = false;
type ConsoleContextState = { key: string | null; open: boolean; focused: boolean };
let consoleContextState: ConsoleContextState = { key: null, open: false, focused: false };
type ServiceLogState = {
  serviceId: string | null;
  logs: string;
  sourcePath: string | null;
  available: boolean;
  loading: boolean;
  message: string | null;
  error: string | null;
};
let selectedServiceId: string | null = null;
type ServicesConsoleTarget =
  | { kind: "service"; id: string }
  | { kind: "container"; id: string };
let servicesConsoleTarget: ServicesConsoleTarget | null = null;
let serviceLogState: ServiceLogState = {
  serviceId: null,
  logs: "",
  sourcePath: null,
  available: false,
  loading: false,
  message: null,
  error: null
};
let serviceLogRequestId = 0;
let serviceConsoleScrollTop = 0;
let serviceConsoleScrollServiceId: string | null = null;
let restoringServiceConsoleScroll = false;
let serviceConsoleContextState: ConsoleContextState = { key: null, open: false, focused: false };
type DockerLogState = {
  containerId: string | null;
  logs: string;
  loading: boolean;
  error: string | null;
};
type ContainerTab = "services" | "docker";
type ContainerViewState = {
  selectedContainerId: string | null;
  logState: DockerLogState;
  logRequestId: number;
  consoleScrollTop: number;
  consoleScrollContainerId: string | null;
  restoringConsoleScroll: boolean;
  consoleContextState: ConsoleContextState;
};
function emptyDockerLogState(): DockerLogState {
  return { containerId: null, logs: "", loading: false, error: null };
}
const containerViewStates: Record<ContainerTab, ContainerViewState> = {
  services: {
    selectedContainerId: null,
    logState: emptyDockerLogState(),
    logRequestId: 0,
    consoleScrollTop: 0,
    consoleScrollContainerId: null,
    restoringConsoleScroll: false,
    consoleContextState: { key: null, open: false, focused: false }
  },
  docker: {
    selectedContainerId: null,
    logState: emptyDockerLogState(),
    logRequestId: 0,
    consoleScrollTop: 0,
    consoleScrollContainerId: null,
    restoringConsoleScroll: false,
    consoleContextState: { key: null, open: false, focused: false }
  }
};
function containerViewState(tab: ContainerTab): ContainerViewState {
  return containerViewStates[tab];
}
function activeContainerTab(): ContainerTab {
  return activeTab === "services" ? "services" : "docker";
}

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
    syncSelectedService();
    renderHeaderCounts();
    renderFooter();
    if (activeTab === "services" && servicesConsoleTarget?.kind === "service") void refreshSelectedServiceLogs();
    render(force);
    if (activeTab === "services" || activeTab === "docker") await refreshContainers(force);
    if (activeTab === "launch") await refreshLaunch(force);
  } catch (error) {
    toast(messageOf(error), true);
  } finally {
    scanBusy = false;
  }
}

function syncSelectedService(): void {
  if (!selectedServiceId) return;
  const stillAvailable = workspace?.services.some((service) => service.relevance === "dev" && service.id === selectedServiceId);
  if (!stillAvailable) clearServiceSelection();
}

function clearServiceSelection(): void {
  selectedServiceId = null;
  if (servicesConsoleTarget?.kind === "service") servicesConsoleTarget = null;
  serviceLogRequestId += 1;
  serviceLogState = {
    serviceId: null,
    logs: "",
    sourcePath: null,
    available: false,
    loading: false,
    message: null,
    error: null
  };
  serviceConsoleScrollServiceId = null;
  serviceConsoleScrollTop = 0;
}

async function refreshSelectedServiceLogs(): Promise<void> {
  if (servicesConsoleTarget?.kind !== "service" || !selectedServiceId) return;
  const service = workspace?.services.find((item) => item.relevance === "dev" && item.id === selectedServiceId);
  if (!service) {
    clearServiceSelection();
    return;
  }
  if (serviceLogState.serviceId === selectedServiceId && serviceLogState.loading) return;
  await loadServiceLogs(selectedServiceId);
}

async function loadServiceLogs(serviceId: string, showLoading = false): Promise<void> {
  if (servicesConsoleTarget?.kind !== "service" || selectedServiceId !== serviceId) return;
  if (!showLoading && serviceLogState.serviceId === serviceId && serviceLogState.loading) return;
  const requestId = ++serviceLogRequestId;
  if (showLoading) {
    serviceLogState = {
      serviceId,
      logs: "",
      sourcePath: null,
      available: false,
      loading: true,
      message: null,
      error: null
    };
    if (activeTab === "services" && servicesConsoleTarget?.kind === "service") updateServiceConsoleDom();
  } else {
    serviceLogState = {
      ...serviceLogState,
      serviceId,
      loading: true,
      error: null
    };
  }
  try {
    const result = await api.serviceLogs(serviceId);
    if (requestId !== serviceLogRequestId || servicesConsoleTarget?.kind !== "service" || selectedServiceId !== serviceId) return;
    serviceLogState = {
      serviceId,
      logs: result.logs ?? "",
      sourcePath: result.source_path ?? null,
      available: result.available ?? Boolean(result.source_path || result.logs),
      loading: false,
      message: result.message ?? null,
      error: null
    };
    if (activeTab === "services" && servicesConsoleTarget?.kind === "service") updateServiceConsoleDom();
  } catch (error) {
    if (requestId !== serviceLogRequestId || servicesConsoleTarget?.kind !== "service" || selectedServiceId !== serviceId) return;
    serviceLogState = {
      ...serviceLogState,
      serviceId,
      loading: false,
      error: messageOf(error)
    };
    if (activeTab === "services" && servicesConsoleTarget?.kind === "service") updateServiceConsoleDom();
  }
}

async function refreshContainers(force = false): Promise<void> {
  if (dockerBusy) return;
  dockerBusy = true;
  try {
    containerListing = await api.containers();
    syncSelectedContainer();
    renderHeaderCounts();
    if (activeTab === "docker" || (activeTab === "services" && servicesConsoleTarget?.kind === "container")) await refreshSelectedContainerLogs();
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
  if (!containerListing?.available) {
    clearDockerSelection();
    return;
  }
  for (const tab of ["services", "docker"] as const) {
    const selectedId = containerViewState(tab).selectedContainerId;
    if (selectedId && !containerListing.containers.some((container) => container.id === selectedId)) clearContainerSelection(tab);
  }
  if (servicesConsoleTarget?.kind === "container" && containerViewState("services").selectedContainerId !== servicesConsoleTarget.id) {
    servicesConsoleTarget = null;
  }
}

function clearContainerSelection(tab: ContainerTab): void {
  const state = containerViewState(tab);
  const clearedId = state.selectedContainerId;
  state.selectedContainerId = null;
  state.logRequestId += 1;
  state.logState = emptyDockerLogState();
  state.consoleScrollContainerId = null;
  state.consoleScrollTop = 0;
  if (tab === "services" && servicesConsoleTarget?.kind === "container" && servicesConsoleTarget.id === clearedId) servicesConsoleTarget = null;
}

function clearDockerSelection(): void {
  clearContainerSelection("services");
  clearContainerSelection("docker");
}

async function refreshSelectedContainerLogs(): Promise<void> {
  const tab = activeContainerTab();
  const state = containerViewState(tab);
  if (tab === "services" && servicesConsoleTarget?.kind !== "container") return;
  if (!state.selectedContainerId || !containerListing?.available) return;
  if (state.logState.containerId === state.selectedContainerId && state.logState.loading) return;
  await loadContainerLogs(state.selectedContainerId, false, tab);
}

function updateContainerConsoleDom(tab: ContainerTab): void {
  if (tab === "docker" && activeTab === "docker") updateDockerConsoleDom();
  else if (tab === "services" && activeTab === "services" && servicesConsoleTarget?.kind === "container") updateServiceConsoleDom();
}

async function loadContainerLogs(containerId: string, showLoading = false, tab = activeContainerTab()): Promise<void> {
  const state = containerViewState(tab);
  if (state.selectedContainerId !== containerId) return;
  if (!showLoading && state.logState.containerId === containerId && state.logState.loading) return;
  const requestId = ++state.logRequestId;
  if (showLoading) {
    state.logState = { containerId, logs: "", loading: true, error: null };
    updateContainerConsoleDom(tab);
  }
  try {
    const result = await api.containerLogs(containerId);
    if (requestId !== state.logRequestId || state.selectedContainerId !== containerId) return;
    state.logState = { containerId, logs: result.logs ?? "", loading: false, error: null };
    updateContainerConsoleDom(tab);
  } catch (error) {
    if (requestId !== state.logRequestId || state.selectedContainerId !== containerId) return;
    state.logState = { ...state.logState, containerId, loading: false, error: messageOf(error) };
    updateContainerConsoleDom(tab);
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
  captureServicesConsoleState();
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
      service.process?.working_directory, service.process?.command,
      service.process?.launch_command, service.process?.create_time,
      service.active_profiles,
      operations.has(`stop:${service.id}`)
    ]),
    groups.map((group) => [
      group.id,
      group.containers.map((container) => [
        container.id, container.name, container.image, container.state, container.status,
        container.ports, container.compose_project, container.compose_service, container.compose_working_dir,
        containerOperationBusy(container.id)
      ])
    ]),
    selectedServiceId,
    containerViewState("services").selectedContainerId,
    servicesConsoleTarget
  ]);
  if (!force && signature === serviceSignature && workspaceElement.dataset.view === "services") {
    updateDockerContainerStatuses();
    updateServiceConsoleDom();
    updateLiveMetrics();
    return;
  }
  serviceSignature = signature;
  workspaceElement.dataset.view = "services";
  if (!workspace) {
    workspaceElement.innerHTML = `<div class="services-view split-view"><div class="split-view-list">${loadingState("Finding services")}</div>${renderServiceConsole()}</div>`;
    restoreServicesConsoleState();
    return;
  }
  if (services.length === 0) {
    workspaceElement.innerHTML = `<div class="services-view split-view"><div class="split-view-list">${emptyState("No development services are running", "Start a local server from a terminal, agent, or IDE.")}</div>${renderServiceConsole()}</div>`;
    restoreServicesConsoleState();
    return;
  }
  workspaceElement.innerHTML = `<div class="services-view split-view"><div class="split-view-list"><div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.services.length + group.containers.length}" aria-labelledby="group-${h(encodeURIComponent(group.id))}">
      <header class="section-header">
        <span class="section-accent accent-${h(group.accent)}"></span>
        <h2 id="group-${h(encodeURIComponent(group.id))}">${renderGroupTitle(group.name, group.services.length, "group-details", `data-group-id="${h(group.id)}"`, group.name.toUpperCase())}</h2>
        ${renderGroupActions(group)}
      </header>
      <div class="tile-grid">${renderServiceGroupTiles(group)}</div>
    </section>`).join("")}</div></div>${renderServiceConsole()}</div>`;
  applyBoardLayout();
  restoreServicesConsoleState();
  updateLiveMetrics();
}

function captureServicesConsoleState(): void {
  captureServiceContextState();
  captureServiceConsoleState();
  captureDockerContextState();
  captureDockerConsoleState();
}

function restoreServicesConsoleState(): void {
  if (servicesConsoleTarget?.kind === "container") restoreDockerConsoleState();
  else {
    restoreServiceContextState();
    restoreServiceConsoleScroll();
  }
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
  return `${group.services.map((service, index) => renderServiceTile(service, ordinalTotal ? index + 1 : undefined, ordinalTotal)).join("")}${group.containers.map((container, index) => renderContainerTile(container, ordinalTotal ? group.services.length + index + 1 : undefined, ordinalTotal, true)).join("")}`;
}

function renderGroupActions(group: ReturnType<typeof groupServices>[number]): string {
  const saveBusy = operations.has(`group-save:${group.id}`);
  const stopBusy = operations.has(`group-stop:${group.id}`);
  const profilePresent = profileForGroup(group) !== undefined;
  const saveAction = profilePresent
    ? `<span class="group-profile-saved" title="Launch profile saved" aria-label="Launch profile saved">${uiIcon("check", 14)}</span>`
    : `<button class="section-action icon-only-button save-group-action" type="button" data-action="save-service-group" data-group-id="${h(group.id)}" title="${saveBusy ? "Saving launch profile" : "Save launch profile"}" aria-label="${saveBusy ? "Saving" : "Save"} launch profile for ${h(group.name)}" ${saveBusy ? "disabled" : ""}>${uiIcon(saveBusy ? "refresh" : "save", 17)}</button>`;
  if (group.services.length === 1) return `<div class="section-actions">${saveAction}</div>`;
  const terminableCount = group.services.filter((service) => service.can_terminate).length;
  const stopDisabled = !terminableCount || stopBusy;
  return `<div class="section-actions">${saveAction}<button class="section-action icon-only-button stop-group-action" type="button" data-action="stop-service-group" data-group-id="${h(group.id)}" title="${stopBusy ? "Stopping services" : terminableCount ? "Stop all stoppable services" : "No services can be stopped safely"}" aria-label="${stopBusy ? "Stopping" : terminableCount ? "Stop all" : "No stoppable"} services in ${h(group.name)}" ${stopDisabled ? "disabled" : ""}>${uiIcon(stopBusy ? "refresh" : "stop", 15)}</button></div>`;
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

type ServiceTileActionScope = { select?: boolean; info?: boolean; stop?: boolean; open?: boolean };

type SharedServiceCardOptions = {
  category: string;
  cardClass?: string;
  metricsId?: string;
  cardAttributes?: string;
  ariaLabel: string;
  selected?: boolean;
  busy?: boolean;
  ordinal?: number;
  ordinalTotal?: number;
  iconMarkup: string;
  pipClass: string;
  title: string;
  overlayMarkup?: string;
  controlsMarkup?: string;
  metricsMarkup: string;
  ports: number[];
  emptyPortLabel: string;
  trailingMarkup?: string;
};

function renderSharedServiceCard(options: SharedServiceCardOptions): string {
  const cardClasses = `service-tile category-${options.category}${options.cardClass ? ` ${options.cardClass}` : ""}${options.busy ? " is-busy" : ""}${options.selected ? " is-selected" : ""}`;
  const metricsAttribute = options.metricsId ? ` data-metrics-id="${h(options.metricsId)}"` : "";
  return `
    <article class="${cardClasses}"${metricsAttribute}${options.cardAttributes ?? ""} aria-label="${h(options.ariaLabel)}">
      ${options.overlayMarkup ?? ""}
      <div class="tile-top">
        <span class="icon-well service-icon" aria-hidden="true">${renderTileOrdinal(options.ordinal, options.ordinalTotal)}${options.iconMarkup}<span class="status-pip state-${options.pipClass}"></span></span>
        ${renderTileHeading(options.title, "", "")}
        ${options.controlsMarkup ? `<div class="service-card-actions">${options.controlsMarkup}</div>` : ""}
      </div>
      <div class="tile-metrics">${options.metricsMarkup}</div>
      ${renderTileFoot(options.ports, options.emptyPortLabel, options.trailingMarkup ?? "")}
    </article>`;
}

function renderServiceTile(service: ServiceSnapshot, ordinal?: number, ordinalTotal?: number, actionScope?: ServiceTileActionScope): string {
  const scope = actionScope ?? { select: service.relevance === "dev", info: true, stop: service.can_terminate, open: Boolean(service.browser_url) };
  const busy = operations.has(`stop:${service.id}`);
  const uptime = currentUptime(service);
  const pip = busy ? "busy" : uptime === null ? "idle" : service.status === "limited" ? "limited" : "running";
  const selected = Boolean(scope.select) && servicesConsoleTarget?.kind === "service" && selectedServiceId === service.id;
  const tileAction = scope.select ? "select-service" : "service-details";
  const tileActionLabel = scope.select
    ? `${selected ? "Selected service logs for" : "View logs for"} ${service.display_name}`
    : `View ${service.display_name} details`;
  const tileActionTitle = scope.select
    ? selected ? "Selected service logs" : "View service logs"
    : "View details";
  const overlayMarkup = scope.select || !scope.info
    ? `<button class="tile-details-button${scope.select ? " service-select-button" : ""}" type="button" data-tile-action data-action="${tileAction}" data-service-id="${h(service.id)}"${scope.select ? ` aria-pressed="${selected ? "true" : "false"}"` : ""} aria-label="${h(tileActionLabel)}" title="${h(tileActionTitle)}"></button>`
    : "";
  const controlsMarkup = `${scope.info ? `<button type="button" class="info-button icon-only-button service-details-button service-card-control" data-tile-action data-action="service-details" data-service-id="${h(service.id)}" aria-label="View ${h(service.display_name)} details" title="View service details">${uiIcon("info", 15)}</button>` : ""}${scope.stop && service.can_terminate ? `<button type="button" class="stop-button service-card-control" data-tile-action data-action="stop-service" data-service-id="${h(service.id)}" aria-label="${busy ? "Stopping" : "Stop"} ${h(service.display_name)}" title="${busy ? "Stopping process" : "Stop process"}" ${busy ? "disabled" : ""}>${uiIcon("stop", 15)}</button>` : ""}`;
  const metricsMarkup = `<span class="metric metric-uptime${uptime !== null && uptime < FRESH_UPTIME_SECONDS ? " is-fresh" : ""}" data-metric="uptime" title="Uptime">${uiIcon("clock", 13)}<span class="sr-only">Uptime </span><span data-metric-text>${h(uptimeText(service, busy))}</span></span><span class="metric metric-memory" data-metric="memory" title="Memory used">${uiIcon("memory", 13)}<span class="sr-only">Memory </span><span data-metric-text>${h(formatBytes(service.process?.memory_bytes ?? null))}</span></span>`;
  const trailingMarkup = scope.open && service.browser_url
    ? `<button type="button" class="service-link icon-only-button service-card-control" data-tile-action data-action="open-service" data-service-id="${h(service.id)}" aria-label="Open ${h(service.display_name)} in the browser" title="Open ${h(service.browser_url)}">${uiIcon("external", 15)}</button>`
    : "";
  return renderSharedServiceCard({
    category: service.category,
    metricsId: service.id,
    ariaLabel: `${service.display_name} service`,
    selected,
    busy,
    ordinal,
    ordinalTotal,
    iconMarkup: techIcon(service.tech, 44),
    pipClass: pip,
    title: serviceTitle(service),
    overlayMarkup,
    controlsMarkup,
    metricsMarkup,
    ports: uniquePorts(service),
    emptyPortLabel: "No port information",
    trailingMarkup
  });
}

function renderTileOrdinal(ordinal?: number, total?: number): string {
  if (!ordinal || !total || total < 2) return "";
  return `<span class="tile-ordinal" title="Item ${ordinal} of ${total}" aria-label="Item ${ordinal} of ${total}">${ordinal}</span>`;
}

// The subtitle carries the technology once: skip the label when the title already says it.
function renderTileHeading(title: string, label: string, badge: string): string {
  const trimmedLabel = label.trim();
  const tech = trimmedLabel && trimmedLabel.toLowerCase() !== title.trim().toLowerCase() ? `<span class="tech-label">${h(trimmedLabel)}</span>` : "";
  return `<div class="tile-heading">
        <h3 class="tile-name" title="${h(title)}">${h(title)}</h3>
        ${tech || badge ? `<p class="tile-subtitle">${tech}${badge}</p>` : ""}
      </div>`;
}

function renderTileFoot(ports: number[], emptyLabel: string, trailing: string): string {
  const labels = portBadgeLabels(ports);
  return `<div class="tile-foot">
        <div class="port-row">
          ${labels.map((label) => `<span class="port-chip${label.startsWith("+") ? " port-overflow" : ""}" title="${h(portChipDescription(label, ports))}" aria-label="${h(portChipDescription(label, ports))}">${h(label)}</span>`).join("")}
          ${ports.length === 0 ? `<span class="no-port-label port-empty-icon" title="${h(emptyLabel)}" aria-label="${h(emptyLabel)}">${uiIcon("port", 14)}</span>` : ""}
        </div>
        ${trailing}
      </div>`;
}

function uptimeText(service: ServiceSnapshot, busy: boolean): string {
  if (busy) return "Stopping…";
  const uptime = currentUptime(service);
  return uptime === null ? "—" : formatUptimeCompact(uptime);
}

function renderConsoleToolbar(label: string, disabled: boolean): string {
  const disabledAttribute = disabled ? " disabled" : "";
  return `<div class="console-toolbar"><span class="console-toolbar-label" title="${h(label)}">${uiIcon("log", 14)}<span>${h(label)}</span></span><div class="console-tools"><button class="console-tool icon-only-button${consoleWrap ? " is-active" : ""}" type="button" data-action="toggle-log-wrap" aria-pressed="${consoleWrap ? "true" : "false"}" aria-label="${consoleWrap ? "Disable line wrapping" : "Wrap long lines"}" title="${consoleWrap ? "Disable line wrapping" : "Wrap long lines"}"${disabledAttribute}>${uiIcon("chevronDown", 13)}</button><button class="console-tool icon-only-button${consoleFollow ? " is-active" : ""}" type="button" data-action="toggle-log-follow" aria-pressed="${consoleFollow ? "true" : "false"}" aria-label="${consoleFollow ? "Pause following new output" : "Follow new output"}" title="${consoleFollow ? "Pause following new output" : "Follow new output"}"${disabledAttribute}>${uiIcon(consoleFollow ? "refresh" : "play", 13)}<span class="sr-only" data-console-follow-label>${consoleFollow ? "Following" : "Follow output"}</span></button></div></div>`;
}

function renderConsoleContext(label: string, content: string): string {
  const available = Boolean(content);
  return `<details class="console-context-details${available ? "" : " is-empty"}" data-console-context aria-disabled="${available ? "false" : "true"}"><summary aria-label="${h(available ? `Show ${label.toLowerCase()}` : `${label} unavailable`)}" title="${h(available ? `Show ${label.toLowerCase()}` : `${label} unavailable`)}">${uiIcon("info", 14)}<span class="sr-only">${h(available ? label : `${label} unavailable`)}</span></summary>${content}</details>`;
}

function renderConsoleSourceMeta(path: string | null, titlePrefix: string): string {
  if (!path) return `<span class="console-meta-item console-meta-source" data-console-source hidden></span>`;
  return `<span class="console-meta-item console-meta-source" data-console-source title="${h(`${titlePrefix}: ${path}`)}"><span class="sr-only">Log source </span>${uiIcon("log", 12)}<code>${h(middleEllipsis(path, 52))}</code></span>`;
}

function patchConsoleElement(target: HTMLElement | null, markup: string): void {
  if (!target) return;
  const template = document.createElement("template");
  template.innerHTML = markup.trim();
  const source = template.content.firstElementChild;
  if (!(source instanceof HTMLElement)) return;
  if (target.className !== source.className) target.className = source.className;
  target.hidden = source.hidden;
  if (target.title !== source.title) target.title = source.title;
  const sourceKey = source.dataset.consoleLogMetaKey ?? "";
  if (target.dataset.consoleLogMetaKey !== sourceKey) {
    if (sourceKey) target.dataset.consoleLogMetaKey = sourceKey;
    else delete target.dataset.consoleLogMetaKey;
  }
  if (target.innerHTML !== source.innerHTML) target.innerHTML = source.innerHTML;
}

function patchConsoleMessage(current: HTMLElement, next: HTMLElement): void {
  const currentStrong = current.querySelector<HTMLElement>(":scope > strong");
  const nextStrong = next.querySelector<HTMLElement>(":scope > strong");
  if (currentStrong && nextStrong && currentStrong.textContent !== nextStrong.textContent) currentStrong.textContent = nextStrong.textContent;
  const currentCopy = [...current.children].filter((child) => child instanceof HTMLElement && child.tagName === "SPAN" && !child.classList.contains("console-message-icon")).at(-1) as HTMLElement | undefined;
  const nextCopy = [...next.children].filter((child) => child instanceof HTMLElement && child.tagName === "SPAN" && !child.classList.contains("console-message-icon")).at(-1) as HTMLElement | undefined;
  if (currentCopy && nextCopy && currentCopy.textContent !== nextCopy.textContent) currentCopy.textContent = nextCopy.textContent;
}

function patchConsoleOutput(output: HTMLElement, markup: string, kind: string, log: string): void {
  const sameKind = output.dataset.consoleOutputKind === kind;
  const hasLog = kind === "log" || kind === "log-alert";
  if (!sameKind) {
    const previousScrollTop = output.scrollTop;
    output.innerHTML = markup;
    output.dataset.consoleOutputKind = kind;
    if (hasLog) {
      if (consoleFollow) output.scrollTop = output.scrollHeight;
      else output.scrollTop = Math.min(previousScrollTop, output.scrollHeight);
    }
    return;
  }

  const currentLog = output.querySelector<HTMLElement>(".console-log");
  if (hasLog && currentLog) {
    const previousScrollTop = output.scrollTop;
    const logChanged = currentLog.textContent !== log;
    if (logChanged) currentLog.textContent = log;
    const template = document.createElement("template");
    template.innerHTML = markup;
    const nextAlert = template.content.querySelector<HTMLElement>(".console-alert");
    const currentAlert = output.querySelector<HTMLElement>(".console-alert");
    const currentAlertText = currentAlert?.querySelector<HTMLElement>("span");
    const nextAlertText = nextAlert?.querySelector<HTMLElement>("span");
    if (currentAlertText && nextAlertText && currentAlertText.textContent !== nextAlertText.textContent) currentAlertText.textContent = nextAlertText.textContent;
    if (logChanged) {
      if (consoleFollow) output.scrollTop = output.scrollHeight;
      else output.scrollTop = Math.min(previousScrollTop, output.scrollHeight);
    }
    return;
  }

  const template = document.createElement("template");
  template.innerHTML = markup;
  const nextMessage = template.content.querySelector<HTMLElement>(".console-message");
  const currentMessage = output.querySelector<HTMLElement>(".console-message");
  const nextAlert = template.content.querySelector<HTMLElement>(".console-alert");
  const currentAlert = output.querySelector<HTMLElement>(".console-alert");
  if (nextMessage && currentMessage && !nextAlert && !currentAlert) {
    patchConsoleMessage(currentMessage, nextMessage);
    return;
  }
  if (nextMessage && currentMessage && nextAlert && currentAlert) {
    patchConsoleMessage(currentMessage, nextMessage);
    const currentAlertText = currentAlert.querySelector<HTMLElement>("span");
    const nextAlertText = nextAlert.querySelector<HTMLElement>("span");
    if (currentAlertText && nextAlertText && currentAlertText.textContent !== nextAlertText.textContent) currentAlertText.textContent = nextAlertText.textContent;
    return;
  }
  output.innerHTML = markup;
  output.dataset.consoleOutputKind = kind;
}

function updateServiceConsoleDom(): void {
  const consoleElement = workspaceElement.querySelector<HTMLElement>(".service-console");
  if (!consoleElement) return;
  if (consoleElement.dataset.consoleKind === "container") {
    const containerId = consoleElement.dataset.consoleContainerId || null;
    const container = containerId
      ? containerListing?.containers.find((item) => item.id === containerId) ?? null
      : null;
    const servicesState = containerViewState("services");
    if (servicesConsoleTarget?.kind !== "container" || containerId !== servicesState.selectedContainerId) return;
    patchConsoleElement(consoleElement.querySelector<HTMLElement>("[data-console-log-meta]"), renderDockerLogMeta(container, servicesState.logState));
    const output = consoleElement.querySelector<HTMLElement>(".console-output");
    if (!output) return;
    const log = container && servicesState.logState.containerId === container.id ? servicesState.logState.logs : "";
    patchConsoleOutput(output, renderDockerLogOutput(container, servicesState.logState), dockerConsoleOutputKind(container, servicesState.logState), log);
    return;
  }
  const serviceId = consoleElement.dataset.consoleServiceId || null;
  const service = serviceId
    ? workspace?.services.find((item) => item.relevance === "dev" && item.id === serviceId) ?? null
    : null;
  if (servicesConsoleTarget?.kind !== "service" || serviceId !== selectedServiceId) return;
  const state = service && serviceLogState.serviceId === service.id ? serviceLogState : null;
  patchConsoleElement(consoleElement.querySelector<HTMLElement>("[data-console-log-meta]"), renderServiceLogMeta(service, state));
  patchConsoleElement(consoleElement.querySelector<HTMLElement>("[data-console-source]"), renderConsoleSourceMeta(service && state?.sourcePath ? state.sourcePath : null, "Log source"));
  const output = consoleElement.querySelector<HTMLElement>(".console-output");
  if (!output) return;
  patchConsoleOutput(output, renderServiceLogOutput(service), serviceConsoleOutputKind(service), state?.logs ?? "");
}

function updateDockerConsoleDom(): void {
  const consoleElement = workspaceElement.querySelector<HTMLElement>(".docker-console, .docker-service-console");
  if (!consoleElement) return;
  const dockerState = containerViewState("docker");
  const containerId = consoleElement.dataset.consoleContainerId || null;
  const container = containerId
    ? containerListing?.containers.find((item) => item.id === containerId) ?? null
    : null;
  if (containerId !== dockerState.selectedContainerId) return;
  patchConsoleElement(consoleElement.querySelector<HTMLElement>("[data-console-log-meta]"), renderDockerLogMeta(container, dockerState.logState));
  const output = consoleElement.querySelector<HTMLElement>(".console-output");
  if (!output) return;
  const log = container && dockerState.logState.containerId === container.id ? dockerState.logState.logs : "";
  patchConsoleOutput(output, renderDockerLogOutput(container, dockerState.logState), dockerConsoleOutputKind(container, dockerState.logState), log);
}

function updateLaunchConsoleDom(): void {
  const output = workspaceElement.querySelector<HTMLElement>(".launch-view > .launch-console .console-output");
  if (!output) return;
  const ref = launchTaskRefForKey(selectedTaskKey);
  if (!ref) return;
  const snapshot = snapshotFor(ref.profile.id, ref.task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  patchConsoleOutput(output, renderConsoleOutput(snapshot, state), launchConsoleOutputKind(snapshot, state), snapshot?.log_tail ?? "");
}

function renderServiceLogMeta(service: ServiceSnapshot | null, state: ServiceLogState | null): string {
  if (!service || !state) return `<span class="console-meta-item" data-console-log-meta hidden></span>`;
  const loading = state.loading && !state.logs.trim();
  const unavailable = !state.loading && Boolean(state.error || state.available === false);
  const message = state.error ?? state.message ?? "Service output is unavailable.";
  if (loading) return `<span class="console-meta-item" data-console-log-meta data-console-log-meta-key="loading" title="Loading recent logs">${uiIcon("refresh", 12)}<span>Loading logs</span></span>`;
  if (unavailable) return `<span class="console-meta-item console-meta-error" data-console-log-meta data-console-log-meta-key="unavailable:${h(message)}" title="${h(message)}">${uiIcon("warning", 12)}<span>Logs unavailable</span></span>`;
  return `<span class="console-meta-item" data-console-log-meta data-console-log-meta-key="available" title="The backend returns the most recent log tail">${uiIcon("log", 12)}<span>Recent log tail</span></span>`;
}

function serviceConsoleOutputKind(service: ServiceSnapshot | null): string {
  if (!service) return "empty";
  const state = serviceLogState.serviceId === service.id ? serviceLogState : null;
  if (state?.loading && !state.logs.trim()) return "loading";
  const log = state?.logs ?? "";
  const message = state?.error ?? state?.message ?? "";
  const unavailable = Boolean(state?.error || state?.available === false);
  if (log.trim()) return unavailable && message.trim() ? "log-alert" : "log";
  if (unavailable) return "unavailable";
  return "empty";
}

function renderServiceConsole(): string {
  if (servicesConsoleTarget?.kind === "container") return renderDockerConsole("service-console docker-service-console", "container", "services");
  const service = servicesConsoleTarget?.kind === "service"
    ? workspace?.services.find((item) => item.relevance === "dev" && item.id === servicesConsoleTarget?.id) ?? null
    : null;
  const stateClass = service ? serviceStateClass(service) : "stopped";
  const statusText = service ? serviceStateText(service) : "No service selected";
  const title = service ? serviceTitle(service) || service.display_name : "Service console";
  const subtitle = service
    ? `${techLabel(service.tech)} · ${service.project?.name ?? service.origin_label ?? service.origin_kind}`
    : "Select a service to view its recent logs";
  const logState = service && serviceLogState.serviceId === service.id ? serviceLogState : null;
  const sourcePath = service && logState?.sourcePath ? logState.sourcePath : null;
  const sourceMeta = renderConsoleSourceMeta(sourcePath, "Log source");
  const portMeta = service
    ? uniquePorts(service).slice(0, 2).map((port) => `<span class="console-meta-item" title="Listening port"><span class="sr-only">Port </span>${uiIcon("port", 12)}<code>localhost:${port}</code></span>`).join("")
    : "";
  const process = service?.process ?? null;
  const context = service
    ? `<div class="console-context" aria-label="Service context"><div class="console-context-item"><span>COMMAND</span><code title="${h(process?.launch_command || process?.command || "—")}">${h(middleEllipsis(process?.launch_command || process?.command || "—", 240))}</code></div><div class="console-context-item"><span>WORKING DIRECTORY</span><code title="${h(process?.working_directory ?? "—")}">${h(middleEllipsis(process?.working_directory ?? "—", 240))}</code></div><div class="console-context-item"><span>PROJECT</span><code title="${h(service.project?.root_path ?? "—")}">${h(middleEllipsis(service.project?.root_path ?? "—", 240))}</code></div></div>`
    : "";
  const outputLabel = service ? `Logs for ${h(title)}` : "Service logs";
  return `<section class="launch-console service-console${service ? ` state-${stateClass}` : " launch-console-empty"}" aria-labelledby="service-console-title" data-console-service-id="${h(service?.id ?? "")}">
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${stateClass}" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="service-console-title">${h(title)}</h2><p>${h(subtitle)}</p></div></div>
      <div class="console-meta" aria-label="Service status"><span class="console-state state-${stateClass}"><span class="task-state-dot" aria-hidden="true"></span>${h(statusText)}</span>${process?.pid !== undefined && process?.pid !== null ? `<span class="console-meta-item" title="Process ID"><span class="sr-only">PID </span>${uiIcon("terminal", 12)}<code>${process.pid}</code></span>` : ""}${portMeta}${sourceMeta}${renderServiceLogMeta(service, logState)}</div>
    </header>
    ${renderConsoleContext("Service details", context)}
    ${renderConsoleToolbar("Recent logs", !service)}
    <div class="console-output${consoleWrap ? " is-wrapped" : ""}" data-console-output-kind="${serviceConsoleOutputKind(service)}" tabindex="0" role="log" aria-live="polite" aria-label="${outputLabel}">${renderServiceLogOutput(service)}</div>
  </section>`;
}

function serviceStateClass(service: ServiceSnapshot): "running" | "starting" | "stopped" {
  if (!service.process) return "stopped";
  return service.status === "limited" ? "starting" : "running";
}

function serviceStateText(service: ServiceSnapshot): string {
  if (!service.process) return "Not running";
  return service.status === "limited" ? "Limited" : "Running";
}

function renderServiceLogOutput(service: ServiceSnapshot | null): string {
  if (!service) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("terminal", 18)}</span><strong>No service selected</strong><span>Select a service card to view its recent logs.</span></div>`;
  }
  const state = serviceLogState.serviceId === service.id ? serviceLogState : null;
  if (state?.loading && !state.logs.trim()) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("refresh", 18)}</span><strong>Loading service logs</strong><span>Fetching the most recent output from the service…</span></div>`;
  }
  const log = state?.logs ?? "";
  const message = state?.error ?? state?.message ?? "";
  const unavailable = Boolean(state?.error || state?.available === false);
  const notice = unavailable && message.trim()
    ? `<div class="console-alert">${uiIcon("warning", 14)}<span>${h(message)}</span></div>`
    : "";
  if (log.trim()) return `${notice}<pre class="console-log">${h(log)}</pre>`;
  if (unavailable) {
    return `${notice}<div class="console-message is-external"><span class="console-message-icon">${uiIcon("warning", 18)}</span><strong>Service output is unavailable</strong><span>${h(message || "The service does not expose a readable stdout or stderr log source.")}</span></div>`;
  }
  return `<div class="console-message"><span class="console-message-icon">${uiIcon("log", 18)}</span><strong>No logs available</strong><span>This service has not produced any output yet.</span></div>`;
}

function renderDocker(force = false): void {
  captureDockerContextState();
  captureDockerConsoleState();
  const fallback = workspace?.services.filter((service) => service.relevance === "container") ?? [];
  const containerActions = containerListing?.containers.map((container) => [container.id, containerOperationBusy(container.id)]) ?? [];
  const containerStructure = containerListing
    ? {
        available: containerListing.available,
        message: containerListing.message,
        containers: containerListing.containers.map((container) => ({
          id: container.id,
          name: container.name,
          image: container.image,
          state: container.state,
          ports: container.ports,
          compose_project: container.compose_project,
          compose_service: container.compose_service,
          compose_working_dir: container.compose_working_dir
        }))
      }
    : null;
  const signature = JSON.stringify([
    containerStructure,
    containerActions,
    containerViewState("docker").selectedContainerId,
    fallback.map((service) => [service.id, uniquePorts(service)])
  ]);
  if (!force && signature === dockerSignature && workspaceElement.dataset.view === "docker") {
    updateDockerContainerStatuses();
    updateDockerConsoleDom();
    return;
  }
  dockerSignature = signature;
  workspaceElement.dataset.view = "docker";
  if (!containerListing) {
    workspaceElement.innerHTML = `<div class="docker-view split-view"><div class="split-view-list">${loadingState("Reading Docker containers")}</div>${renderDockerConsole()}</div>`;
    void refreshContainers();
    return;
  }
  if (!containerListing.available) {
    if (fallback.length) {
      workspaceElement.innerHTML = `<div class="docker-view split-view"><div class="split-view-list"><div class="inline-notice"><strong>Docker could not be queried.</strong><span>${h(containerListing.message ?? "Docker is unavailable.")}</span></div><div class="board"><section class="service-section" data-tiles="${fallback.length}" aria-labelledby="container-listeners-title"><header class="section-header"><span class="section-accent accent-container"></span><h2 id="container-listeners-title">CONTAINER LISTENERS</h2></header><div class="tile-grid">${fallback.map((service, index) => renderServiceTile(service, index + 1, fallback.length, { info: false, stop: service.can_terminate, open: Boolean(service.browser_url) })).join("")}</div></section></div></div>${renderDockerConsole()}</div>`;
      applyBoardLayout();
      restoreDockerConsoleState();
      return;
    }
    workspaceElement.innerHTML = `<div class="docker-view split-view"><div class="split-view-list">${emptyState("Docker is unavailable", containerListing.message ?? "The Docker CLI could not be queried.")}</div>${renderDockerConsole()}</div>`;
    restoreDockerConsoleState();
    return;
  }
  if (containerListing.containers.length === 0) {
    workspaceElement.innerHTML = `<div class="docker-view split-view"><div class="split-view-list">${emptyState("No containers found", containerListing.message ?? "Docker returned an empty list.")}</div>${renderDockerConsole()}</div>`;
    restoreDockerConsoleState();
    return;
  }
  const groups = groupContainers(containerListing.containers);
  workspaceElement.innerHTML = `<div class="docker-view split-view"><div class="split-view-list"><div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.containers.length}" aria-labelledby="container-group-${h(encodeURIComponent(group.name))}">
      <header class="section-header"><span class="section-accent accent-container"></span><h2 id="container-group-${h(encodeURIComponent(group.name))}">${renderGroupTitle(group.name, group.containers.length, "container-group-details", `data-group-name="${h(group.name)}"`, group.name.toUpperCase())}</h2></header>
      <div class="tile-grid">${group.containers.map((container, index) => renderContainerTile(container, index + 1, group.containers.length, true)).join("")}</div>
    </section>`).join("")}</div></div>${renderDockerConsole()}</div>`;
  applyBoardLayout();
  restoreDockerConsoleState();
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
  const selectedId = showActions ? containerViewState(activeTab === "services" ? "services" : "docker").selectedContainerId : null;
  const selected = showActions && selectedId === container.id && (activeTab !== "services" || servicesConsoleTarget?.kind === "container");
  const action = running ? "stop-container" : "start-container";
  const actionLabel = busy ? (running ? "Stopping" : "Starting") : running ? "Stop" : "Start";
  const stateText = busy ? `${actionLabel}…` : container.status || (running ? "Running" : "Stopped");
  const actionButton = showActions
    ? running
      ? `<button type="button" class="stop-button service-card-control container-action" data-tile-action data-action="${action}" data-container-id="${h(container.id)}" aria-label="${actionLabel} ${h(container.name)}" title="${actionLabel} container" ${busy ? "disabled aria-busy=\"true\"" : ""}>${uiIcon("stop", 15)}</button>`
      : `<button type="button" class="start-action icon-only-button service-card-control container-action" data-tile-action data-action="${action}" data-container-id="${h(container.id)}" aria-label="${actionLabel} ${h(container.name)}" title="${actionLabel} container" ${busy ? "disabled aria-busy=\"true\"" : ""}>${uiIcon(busy ? "refresh" : "play", 15)}</button>`
    : "";
  const detailsButton = showActions
    ? `<button type="button" class="info-button icon-only-button service-card-control container-details-button" data-tile-action data-action="container-details" data-container-id="${h(container.id)}" aria-label="View ${h(container.name)} details" title="View container details">${uiIcon("info", 15)}</button>`
    : `<button class="tile-details-button" type="button" data-tile-action data-action="container-details" data-container-id="${h(container.id)}" aria-label="View ${h(container.name)} details" title="View details"></button>`;
  const selectionAttributes = showActions
    ? ` data-action="select-container" tabindex="0" role="button" aria-pressed="${selected ? "true" : "false"}"`
    : "";
  return `
    <article class="service-tile container-tile ${running ? "is-running" : "is-stopped"}${selected ? " is-selected" : ""}${busy ? " is-busy" : ""}" data-container-id="${h(container.id)}" aria-label="${h(container.name)} container"${selectionAttributes}${busy ? " aria-busy=\"true\"" : ""}>
      ${showActions ? "" : detailsButton}
      <div class="tile-top">
        <span class="icon-well service-icon" aria-hidden="true">${renderTileOrdinal(ordinal, ordinalTotal)}${techIcon(imageTech(container.image), 44)}<span class="status-pip state-${busy ? "busy" : running ? "running" : "idle"}"></span></span>
        ${renderTileHeading(container.name, "", "")}
        ${showActions ? `<div class="service-card-actions">${detailsButton}${actionButton}</div>` : actionButton}
      </div>
      <div class="tile-metrics">
        <span class="metric metric-state ${busy ? "is-busy" : running ? "is-running" : "is-stopped"}" title="Container state">${uiIcon("docker", 13)}<span class="sr-only">State </span><span data-container-status-text>${h(ellipsis(stateText, 30))}</span></span>
      </div>
      ${renderTileFoot(container.ports, "No published ports", "")}
    </article>`;
}

function containerOperationBusy(id: string): boolean {
  return operations.has(`container:${id}`);
}

function updateDockerContainerStatuses(): void {
  if (!containerListing?.available) return;
  for (const element of workspaceElement.querySelectorAll<HTMLElement>("[data-container-status-text]")) {
    const tile = element.closest<HTMLElement>("[data-container-id]");
    const id = tile?.dataset.containerId;
    if (!id) continue;
    const container = containerListing.containers.find((item) => item.id === id);
    if (!container) continue;
    const busy = containerOperationBusy(container.id);
    const actionLabel = container.state === "running" ? "Stopping" : "Starting";
    const stateText = busy ? `${actionLabel}…` : container.status || (container.state === "running" ? "Running" : "Stopped");
    const nextText = ellipsis(stateText, 30);
    if (element.textContent !== nextText) element.textContent = nextText;
  }
  const consoleElement = workspaceElement.querySelector<HTMLElement>(".docker-console, .docker-service-console");
  const containerId = consoleElement?.dataset.consoleContainerId;
  if (!consoleElement || !containerId) return;
  const container = containerListing.containers.find((item) => item.id === containerId);
  const busy = container ? containerOperationBusy(container.id) : false;
  const statusText = container
    ? busy ? `${container.state === "running" ? "Stopping" : "Starting"}…` : containerStateText(container)
    : "No container selected";
  const statusElement = consoleElement.querySelector<HTMLElement>("[data-console-status-text]");
  if (statusElement && statusElement.textContent !== statusText) statusElement.textContent = statusText;
}

function renderDockerLogMeta(container: ContainerInfo | null, logState = containerViewState(activeContainerTab()).logState): string {
  if (!container) return `<span class="console-meta-item" data-console-log-meta hidden></span>`;
  if (logState.loading) return `<span class="console-meta-item" data-console-log-meta data-console-log-meta-key="loading" title="Loading recent logs">${uiIcon("refresh", 12)}<span>Loading logs</span></span>`;
  if (logState.error) return `<span class="console-meta-item console-meta-error" data-console-log-meta data-console-log-meta-key="error:${h(logState.error)}" title="${h(logState.error)}">${uiIcon("warning", 12)}<span>Logs unavailable</span></span>`;
  return `<span class="console-meta-item" data-console-log-meta data-console-log-meta-key="available" title="The backend returns the most recent 200 lines">${uiIcon("log", 12)}<span>Last 200 lines</span></span>`;
}

function dockerConsoleOutputKind(container: ContainerInfo | null, logState = containerViewState(activeContainerTab()).logState): string {
  if (!container) return "empty";
  const log = logState.containerId === container.id ? logState.logs : "";
  if (logState.loading) return "loading";
  if (logState.error) return log.trim() ? "log-alert" : "error";
  return log.trim() ? "log" : "empty";
}

function renderDockerConsole(consoleClass = "docker-console", consoleKind = "container", tab = activeContainerTab()): string {
  const viewState = containerViewState(tab);
  const container = viewState.selectedContainerId
    ? containerListing?.containers.find((item) => item.id === viewState.selectedContainerId) ?? null
    : null;
  const stateClass = container ? containerStateClass(container) : "idle";
  const busy = container ? containerOperationBusy(container.id) : false;
  const statusText = container
    ? busy ? `${container.state === "running" ? "Stopping" : "Starting"}…` : containerStateText(container)
    : "No container selected";
  const title = container?.name ?? "Container console";
  const subtitle = container ? container.image : "Select a container to view its recent logs";
  const context = container
    ? `<div class="console-context" aria-label="Container context"><div class="console-context-item"><span>CONTAINER ID</span><code title="${h(container.id)}">${h(container.id)}</code></div><div class="console-context-item"><span>IMAGE</span><code title="${h(container.image)}">${h(middleEllipsis(container.image, 240))}</code></div><div class="console-context-item"><span>COMPOSE SERVICE</span><code title="${h(container.compose_service ?? "Standalone container")}">${h(container.compose_service ?? "Standalone container")}</code></div><div class="console-context-item"><span>COMPOSE PROJECT</span><code title="${h(container.compose_project ?? "—")}">${h(container.compose_project ?? "—")}</code></div></div>`
    : "";
  const outputLabel = container ? `Logs for ${h(container.name)}` : "Docker container logs";
  return `<section class="launch-console ${consoleClass}${container ? ` state-${stateClass}` : " launch-console-empty"}" aria-labelledby="docker-console-title" data-console-kind="${h(consoleKind)}" data-console-container-id="${h(container?.id ?? "")}">
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${stateClass}" aria-hidden="true">${uiIcon("docker", 16)}</span><div><h2 id="docker-console-title">${h(title)}</h2><p>${h(subtitle)}</p></div></div>
      <div class="console-meta" aria-label="Container status"><span class="console-state state-${stateClass}"><span class="task-state-dot" aria-hidden="true"></span><span data-console-status-text>${h(statusText)}</span></span>${container ? `<span class="console-meta-item" title="Container ID"><span class="sr-only">Container ID </span>${uiIcon("docker", 12)}<code>${h(middleEllipsis(container.id, 16))}</code></span>` : ""}${renderDockerLogMeta(container, viewState.logState)}</div>
    </header>
    ${renderConsoleContext("Container details", context)}
    ${renderConsoleToolbar("Recent logs", !container)}
    <div class="console-output${consoleWrap ? " is-wrapped" : ""} docker-console-output" data-console-output-kind="${dockerConsoleOutputKind(container, viewState.logState)}" tabindex="0" role="log" aria-live="polite" aria-label="${outputLabel}">${renderDockerLogOutput(container, viewState.logState)}</div>
  </section>`;
}

function renderDockerLogOutput(container: ContainerInfo | null, logState = containerViewState(activeContainerTab()).logState): string {
  if (!container) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("docker", 18)}</span><strong>No container selected</strong><span>Select a Docker container card to view its recent logs.</span></div>`;
  }
  const log = logState.containerId === container.id ? logState.logs : "";
  if (logState.loading) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("refresh", 18)}</span><strong>Loading container logs</strong><span>Fetching the most recent output from Docker…</span></div>`;
  }
  if (logState.error) {
    const alert = `<div class="console-alert">${uiIcon("warning", 14)}<span>${h(logState.error)}</span></div>`;
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
  captureConsoleContextState();
  captureConsoleState();
  const signature = JSON.stringify([
    profiles,
    taskSnapshots.map(({ log_tail: _logTail, ...snapshot }) => snapshot),
    appInfo?.demo,
    [...operations]
  ]);
  if (!force && signature === launchSignature && workspaceElement.dataset.view === "launch") {
    updateLaunchConsoleDom();
    return;
  }
  launchSignature = signature;
  workspaceElement.dataset.view = "launch";
  if (profiles.length === 0) {
    selectedTaskKey = null;
    workspaceElement.innerHTML = `<div class="launch-view split-view"><div class="split-view-list"><div class="launch-list board">${renderLaunchAddCard()}</div></div>${renderLaunchConsole(null)}</div>`;
    applyBoardLayout();
    return;
  }
  const selected = ensureSelectedTask();
  workspaceElement.innerHTML = `<div class="launch-view split-view"><div class="split-view-list"><div class="launch-list board">${profiles.map(renderProfile).join("")}${renderLaunchAddCard()}</div></div>${renderLaunchConsole(selected)}</div>`;
  applyBoardLayout();
  restoreConsoleContextState();
  restoreConsoleScroll();
}

function renderLaunchAddCard(): string {
  return `<article class="launch-add-card" aria-labelledby="launch-add-title">
    <button class="launch-add-action" type="button" data-action="add-profile" aria-label="Add launch profile" title="Add launch profile" ${appInfo?.demo ? "disabled" : ""}>
      <span class="launch-add-icon" aria-hidden="true">${uiIcon("plus", 24)}</span>
      <strong id="launch-add-title">Add profile</strong>
    </button>
    <button class="info-button icon-only-button launch-add-info" type="button" data-action="show-info" data-info-kind="launch" aria-label="About launch profiles" title="About launch profiles">${uiIcon("info", 15)}</button>
  </article>`;
}

function bulkActionIcon(name: "play" | "stop"): string {
  return `<span class="bulk-action-icon bulk-action-${name}" aria-hidden="true">${uiIcon(name, 15, "bulk-action-icon-back")}${uiIcon(name, 15, "bulk-action-icon-front")}</span>`;
}

function renderProfile(profile: LaunchProfile): string {
  const snapshots = profile.tasks.map((task) => snapshotFor(profile.id, task.name));
  const canStop = snapshots.some((snapshot) => snapshot && ["starting", "running", "stopping"].includes(snapshot.state));
  const canStart = profile.tasks.some((task) => !["starting", "running", "stopping", "external"].includes(snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  const profileBusy = operations.has(launchProfileOperationKey(profile.id)) || launchProfileHasTaskOperation(profile);
  const runAll = profile.tasks.length > 1 && canStart
    ? `<button class="primary-button icon-only-button bulk-action-button" type="button" data-action="start-profile" data-profile-id="${h(profile.id)}" aria-label="${canStop ? "Start remaining tasks" : "Run all tasks"}" title="${profileBusy ? "Starting tasks" : canStop ? "Start remaining tasks" : "Run all tasks"}" ${profileBusy ? "disabled" : ""}>${bulkActionIcon("play")}</button>`
    : "";
  const stopAll = profile.tasks.length > 1 && canStop
    ? `<button class="secondary-button danger-button icon-only-button bulk-action-button" type="button" data-action="stop-profile" data-profile-id="${h(profile.id)}" aria-label="Stop all tasks" title="${profileBusy ? "Stopping tasks" : "Stop all tasks"}" ${profileBusy ? "disabled" : ""}>${bulkActionIcon("stop")}</button>`
    : "";
  return `<section class="launch-profile service-section" data-tiles="${profile.tasks.length}" aria-labelledby="launch-profile-${h(profile.id)}">
    <header class="section-header launch-profile-header">
      <span class="section-accent accent-runtime" aria-hidden="true"></span>
      <div class="launch-profile-heading"><h2 id="launch-profile-${h(profile.id)}">${renderGroupTitle(profile.name, profile.tasks.length, "profile-details", `data-profile-id="${h(profile.id)}"`, profile.name.toUpperCase(), "View profile details")}</h2></div>
      <div class="section-actions launch-profile-actions">
        ${runAll}${stopAll}
        <button class="section-action icon-only-button" type="button" data-action="edit-profile" data-profile-id="${h(profile.id)}" aria-label="Edit ${h(profile.name)}" title="Edit profile" ${appInfo?.demo || canStop ? "disabled" : ""}>${uiIcon("settings", 15)}</button>
      </div>
    </header>
    <div class="task-list" role="list" aria-label="Tasks in ${h(profile.name)}">${profile.tasks.map((task) => renderTask(profile, task)).join("")}</div>
  </section>`;
}

function pathsMatch(left: string | null | undefined, right: string | null | undefined): boolean {
  const candidate = left?.trim();
  const parent = right?.trim();
  if (!candidate || !parent || candidate === "." || parent === ".") return false;
  return candidate === parent || pathIsEqualOrNested(candidate, parent) || pathIsEqualOrNested(parent, candidate);
}

function matchedServiceForTask(profile: LaunchProfile, task: LaunchTask): ServiceSnapshot | null {
  const services = workspace?.services.filter((service) => service.relevance === "dev") ?? [];
  const expectedPort = task.expected_port;
  const taskRoots = [task.cwd, profile.project_root];
  const scored = services.map((service) => {
    const ports = uniquePorts(service);
    if (expectedPort !== null && expectedPort !== undefined && !ports.includes(expectedPort)) return { service, score: -1 };
    const serviceRoots = [service.process?.working_directory, service.project?.root_path];
    const pathMatch = taskRoots.some((taskRoot) => serviceRoots.some((serviceRoot) => pathsMatch(taskRoot, serviceRoot)));
    const projectMatch = pathsMatch(profile.project_root, service.project?.root_path);
    const score = (expectedPort !== null && expectedPort !== undefined ? 4 : 0) + (pathMatch ? 5 : 0) + (projectMatch ? 2 : 0);
    return { service, score };
  }).filter(({ score }) => score > 0).sort((left, right) => right.score - left.score || left.service.id.localeCompare(right.service.id));
  return scored[0]?.service ?? null;
}

function renderTask(profile: LaunchProfile, task: LaunchTask): string {
  const snapshot = snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  const active = ["starting", "running", "stopping"].includes(state);
  const external = state === "external";
  const matchedService = matchedServiceForTask(profile, task);
  const taskOperation = operations.has(`task:${profile.id}:${task.name}`);
  const serviceOperation = matchedService ? operations.has(`stop:${matchedService.id}`) : false;
  const profileOperation = operations.has(launchProfileOperationKey(profile.id));
  const busy = taskOperation || serviceOperation || profileOperation;
  const selected = selectedTaskKey === launchTaskKey(profile.id, task.name);
  const externalCanStop = external && Boolean(matchedService?.can_terminate);
  const stopUnavailable = external ? !externalCanStop : !active;
  const startAction = !active && !external
    ? `<button class="quiet-button start-action icon-only-button service-card-control task-card-action" type="button" data-tile-action data-action="start-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" aria-label="Start ${h(task.name)}" title="Start task" ${busy ? "disabled" : ""}>${uiIcon(busy ? "refresh" : "play", 15)}</button>`
    : "";
  const stopAction = `<button class="stop-button service-card-control task-card-action${stopUnavailable ? " is-unavailable" : ""}" type="button" data-tile-action data-action="${externalCanStop ? "stop-service" : "stop-task"}"${externalCanStop ? ` data-service-id="${h(matchedService?.id ?? "")}"` : ` data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}"`} aria-label="${externalCanStop ? "Stop externally managed service" : `Stop ${task.name}`}" title="${active ? busy ? "Stopping task" : "Stop task" : externalCanStop ? "Stop externally managed service" : external ? "Cannot stop an externally managed task" : "Task is not running"}" ${stopUnavailable || busy ? "disabled" : ""}>${uiIcon("stop", 15)}</button>`;
  const matchedUptime = matchedService ? currentUptime(matchedService) : null;
  const metricsMarkup = matchedService
    ? `<span class="metric metric-uptime${matchedUptime !== null && matchedUptime < FRESH_UPTIME_SECONDS ? " is-fresh" : ""}" data-metric="uptime" title="Uptime">${uiIcon("clock", 13)}<span class="sr-only">Uptime </span><span data-metric-text>${h(uptimeText(matchedService, serviceOperation))}</span></span><span class="metric metric-memory" data-metric="memory" title="Memory used">${uiIcon("memory", 13)}<span class="sr-only">Memory </span><span data-metric-text>${h(formatBytes(matchedService.process?.memory_bytes ?? null))}</span></span>`
    : state === "external"
      ? `<span class="metric metric-state is-external" title="Running externally">${uiIcon("terminal", 13)}<span class="sr-only">Running externally</span></span>`
      : `<span class="metric metric-state ${busy ? "is-busy" : `state-${state}`}" title="Task state">${uiIcon(state === "running" ? "play" : "terminal", 13)}<span class="sr-only">State </span>${h(stateLabel(state))}</span>`;
  const cardAttributes = ` data-action="select-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" tabindex="0" role="listitem" aria-current="${selected ? "true" : "false"}"`;
  return renderSharedServiceCard({
    category: matchedService?.category ?? "runtime",
    cardClass: `task-card state-${state}`,
    metricsId: matchedService?.id,
    cardAttributes,
    ariaLabel: `${profile.name} · ${task.name}, ${stateLabel(state)}${task.expected_port ? `, port ${task.expected_port}` : ""}`,
    selected,
    busy,
    iconMarkup: matchedService ? techIcon(matchedService.tech, 44) : uiIcon("terminal", 25),
    pipClass: busy ? "busy" : state === "external" ? "external" : state === "stopped" || state === "failed" ? "idle" : "running",
    title: task.name,
    controlsMarkup: `${startAction}${stopAction}`,
    metricsMarkup,
    ports: task.expected_port ? [task.expected_port] : [],
    emptyPortLabel: "No expected port"
  });
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
    return `<section class="launch-console launch-console-empty" aria-labelledby="launch-console-title"><header class="console-header"><div class="console-title"><span class="console-icon" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="launch-console-title">Task console</h2><p>No task selected</p></div></div><div class="console-meta" aria-label="Task status"><span class="console-state"><span class="task-state-dot" aria-hidden="true"></span>No task selected</span><span class="console-meta-item" data-console-log-meta hidden></span></div></header>${renderConsoleContext("Task details", "")}${renderConsoleToolbar("Recent logs", true)}<div class="console-output console-empty-output" data-console-output-kind="empty" role="status"><div class="console-message"><strong>No tasks available</strong><span>Add at least one task to this launch profile.</span></div></div></section>`;
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
      <div class="console-meta" aria-label="Task status"><span class="console-state state-${state}"><span class="task-state-dot" aria-hidden="true"></span>${h(stateLabel(state))}</span>${pid !== null ? `<span class="console-meta-item" title="Process ID"><span class="sr-only">PID </span>${uiIcon("terminal", 12)}<code>${pid}</code></span>` : ""}${port ? `<span class="console-meta-item" title="Port"><span class="sr-only">Port </span>${uiIcon("port", 12)}<code>localhost:${port}</code></span>` : ""}${externalLogPath ? `<span class="console-meta-item console-meta-source" title="External log source: ${h(externalLogPath)}"><span class="sr-only">Log source </span>${uiIcon("log", 12)}<code>${h(middleEllipsis(externalLogPath, 52))}</code></span>` : ""}</div>
    </header>
    ${renderConsoleContext("Task details", `<div class="console-context" aria-label="Task context"><div class="console-context-item"><span>COMMAND</span><code title="${h(task.command)}">${h(middleEllipsis(command, 240))}</code></div><div class="console-context-item"><span>WORKING DIRECTORY</span><code title="${h(cwd)}">${h(middleEllipsis(cwd, 240))}</code></div></div>`)}
    ${renderConsoleToolbar("Recent logs", false)}
    <div class="console-output${consoleWrap ? " is-wrapped" : ""}" data-console-output-kind="${launchConsoleOutputKind(snapshot, state)}" tabindex="0" role="log" aria-live="polite" aria-label="Output for ${h(task.name)}">${renderConsoleOutput(snapshot, state)}</div>
  </section>`;
}

function launchConsoleOutputKind(snapshot: ManagedTaskSnapshot | undefined, state: LaunchState): string {
  const log = snapshot?.log_tail ?? "";
  if (state === "external") return log.length > 0 ? "log" : "external";
  if (state === "failed") return log.length > 0 ? "log-alert" : "failed";
  return log.length > 0 ? "log" : "empty";
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

function captureConsoleContextState(): void {
  const details = document.querySelector<HTMLElement>(".launch-view > .launch-console .console-context-details");
  if (!details) return;
  const taskKey = details.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
  if (taskKey !== selectedTaskDomKey()) return;
  consoleContextState = {
    key: selectedTaskKey,
    open: details instanceof HTMLDetailsElement ? details.open : false,
    focused: document.activeElement === details.querySelector("summary")
  };
}

function restoreConsoleContextState(): void {
  const details = document.querySelector<HTMLElement>(".launch-view > .launch-console .console-context-details");
  if (!details || consoleContextState.key !== selectedTaskKey) return;
  const nextTaskKey = details.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
  if (nextTaskKey !== selectedTaskDomKey()) return;
  if (details instanceof HTMLDetailsElement) details.open = consoleContextState.open;
  if (consoleContextState.focused) details.querySelector<HTMLElement>("summary")?.focus();
}

function captureConsoleState(): void {
  const output = document.querySelector<HTMLElement>(".launch-console:not(.docker-console):not(.docker-service-console) .console-output");
  if (!output) return;
  const taskKey = output.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
  if (taskKey !== selectedTaskDomKey()) return;
  consoleScrollTaskKey = selectedTaskKey;
  consoleScrollTop = output.scrollTop;
}

function captureServiceConsoleState(): void {
  const output = document.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
  if (!output) return;
  const serviceId = output.closest<HTMLElement>(".service-console:not(.docker-service-console)")?.dataset.consoleServiceId || null;
  if (servicesConsoleTarget?.kind !== "service" || serviceId !== selectedServiceId) return;
  serviceConsoleScrollServiceId = selectedServiceId;
  serviceConsoleScrollTop = output.scrollTop;
}

function captureServiceContextState(): void {
  const details = document.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-context-details");
  if (!details) return;
  const serviceId = details.closest<HTMLElement>(".service-console:not(.docker-service-console)")?.dataset.consoleServiceId || null;
  if (servicesConsoleTarget?.kind !== "service" || serviceId !== selectedServiceId) return;
  serviceConsoleContextState = {
    key: selectedServiceId,
    open: details instanceof HTMLDetailsElement ? details.open : false,
    focused: document.activeElement === details.querySelector("summary")
  };
}

function restoreServiceContextState(): void {
  const details = document.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-context-details");
  if (!details || servicesConsoleTarget?.kind !== "service" || serviceConsoleContextState.key !== selectedServiceId) return;
  const serviceId = details.closest<HTMLElement>(".service-console:not(.docker-service-console)")?.dataset.consoleServiceId || null;
  if (serviceId !== selectedServiceId) return;
  if (details instanceof HTMLDetailsElement) details.open = serviceConsoleContextState.open;
  if (serviceConsoleContextState.focused) details.querySelector<HTMLElement>("summary")?.focus();
}

function restoreConsoleScroll(): void {
  const output = document.querySelector<HTMLElement>(".launch-console:not(.docker-console):not(.docker-service-console) .console-output");
  if (!output) return;
  restoringConsoleScroll = true;
  const savedScrollTop = consoleScrollTaskKey === selectedTaskKey ? consoleScrollTop : 0;
  output.scrollTop = consoleFollow ? output.scrollHeight : Math.min(savedScrollTop, output.scrollHeight);
  consoleScrollTaskKey = selectedTaskKey;
  consoleScrollTop = output.scrollTop;
  window.setTimeout(() => { restoringConsoleScroll = false; }, 0);
}

function restoreServiceConsoleScroll(): void {
  const output = document.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
  if (!output) return;
  restoringServiceConsoleScroll = true;
  const savedScrollTop = serviceConsoleScrollServiceId === selectedServiceId ? serviceConsoleScrollTop : 0;
  output.scrollTop = consoleFollow ? output.scrollHeight : Math.min(savedScrollTop, output.scrollHeight);
  serviceConsoleScrollServiceId = selectedServiceId;
  serviceConsoleScrollTop = output.scrollTop;
  window.setTimeout(() => { restoringServiceConsoleScroll = false; }, 0);
}

function captureDockerConsoleState(): void {
  const output = document.querySelector<HTMLElement>(".docker-console .console-output, .docker-service-console .console-output");
  if (!output) return;
  const consoleElement = output.closest<HTMLElement>(".docker-console, .docker-service-console");
  const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
  const state = containerViewState(tab);
  const containerId = consoleElement?.dataset.consoleContainerId || null;
  if (containerId !== state.selectedContainerId) return;
  state.consoleScrollContainerId = state.selectedContainerId;
  state.consoleScrollTop = output.scrollTop;
}

function captureDockerContextState(): void {
  const details = document.querySelector<HTMLElement>(".docker-console .console-context-details, .docker-service-console .console-context-details");
  if (!details) return;
  const consoleElement = details.closest<HTMLElement>(".docker-console, .docker-service-console");
  const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
  const state = containerViewState(tab);
  const containerId = consoleElement?.dataset.consoleContainerId || null;
  if (containerId !== state.selectedContainerId) return;
  state.consoleContextState = {
    key: state.selectedContainerId,
    open: details instanceof HTMLDetailsElement ? details.open : false,
    focused: document.activeElement === details.querySelector("summary")
  };
}

function restoreDockerContextState(): void {
  const details = document.querySelector<HTMLElement>(".docker-console .console-context-details, .docker-service-console .console-context-details");
  if (!details) return;
  const consoleElement = details.closest<HTMLElement>(".docker-console, .docker-service-console");
  const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
  const state = containerViewState(tab);
  const containerId = consoleElement?.dataset.consoleContainerId || null;
  if (state.consoleContextState.key !== state.selectedContainerId || containerId !== state.selectedContainerId) return;
  if (details instanceof HTMLDetailsElement) details.open = state.consoleContextState.open;
  if (state.consoleContextState.focused) details.querySelector<HTMLElement>("summary")?.focus();
}

function restoreDockerConsoleScroll(): void {
  const output = document.querySelector<HTMLElement>(".docker-console .console-output, .docker-service-console .console-output");
  if (!output) return;
  const consoleElement = output.closest<HTMLElement>(".docker-console, .docker-service-console");
  const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
  const state = containerViewState(tab);
  state.restoringConsoleScroll = true;
  const savedScrollTop = state.consoleScrollContainerId === state.selectedContainerId ? state.consoleScrollTop : 0;
  output.scrollTop = consoleFollow ? output.scrollHeight : Math.min(savedScrollTop, output.scrollHeight);
  state.consoleScrollContainerId = state.selectedContainerId;
  state.consoleScrollTop = output.scrollTop;
  window.setTimeout(() => { state.restoringConsoleScroll = false; }, 0);
}

function restoreDockerConsoleState(): void {
  restoreDockerContextState();
  restoreDockerConsoleScroll();
}

function handleConsoleScroll(event: Event): void {
  const target = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-output") : null;
  if (!target) return;
  const serviceConsole = target.closest<HTMLElement>(".service-console:not(.docker-service-console)");
  if (serviceConsole) {
    const serviceId = serviceConsole.dataset.consoleServiceId || null;
    if (servicesConsoleTarget?.kind !== "service" || serviceId !== selectedServiceId) return;
    serviceConsoleScrollServiceId = selectedServiceId;
    serviceConsoleScrollTop = target.scrollTop;
    if (restoringServiceConsoleScroll) return;
    const distanceFromEnd = target.scrollHeight - target.clientHeight - target.scrollTop;
    consoleFollow = distanceFromEnd <= 18;
    updateConsoleControls();
    return;
  }
  const dockerConsole = target.closest<HTMLElement>(".docker-console, .docker-service-console");
  if (dockerConsole) {
    const tab: ContainerTab = dockerConsole.classList.contains("docker-service-console") ? "services" : "docker";
    const state = containerViewState(tab);
    const containerId = dockerConsole.dataset.consoleContainerId || null;
    if (dockerConsole.classList.contains("docker-service-console") && servicesConsoleTarget?.kind !== "container") return;
    if (containerId !== state.selectedContainerId) return;
    state.consoleScrollContainerId = state.selectedContainerId;
    state.consoleScrollTop = target.scrollTop;
    if (state.restoringConsoleScroll) return;
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

function selectService(id: string, focus = false): void {
  const service = findService(id);
  if (service.relevance !== "dev") throw new Error("This service is not available in the Services view.");
  captureServicesConsoleState();
  selectedServiceId = id;
  servicesConsoleTarget = { kind: "service", id };
  serviceConsoleScrollServiceId = id;
  serviceConsoleScrollTop = 0;
  serviceLogRequestId += 1;
  serviceLogState = {
    serviceId: id,
    logs: "",
    sourcePath: null,
    available: false,
    loading: true,
    message: null,
    error: null
  };
  renderServices(true);
  void loadServiceLogs(id, true);
  if (focus) focusServiceCard(id);
}

function setSelectedTask(profileId: string, taskName: string): void {
  const key = launchTaskKey(profileId, taskName);
  if (selectedTaskKey === key) return;
  selectedTaskKey = key;
  consoleScrollTaskKey = key;
  consoleScrollTop = 0;
}

function focusTaskRow(profileId: string, taskName: string): void {
  const row = [...document.querySelectorAll<HTMLElement>(".task-card")].find((item) => item.dataset.profileId === profileId && item.dataset.taskName === taskName);
  row?.focus();
}

function focusServiceCard(id: string): void {
  const card = [...document.querySelectorAll<HTMLElement>(".service-select-button")]
    .find((item) => item.dataset.serviceId === id);
  card?.focus();
}

function selectContainer(id: string, focus = false): void {
  findContainer(id);
  const tab = activeContainerTab();
  if (activeTab === "services") captureServicesConsoleState();
  const state = containerViewState(tab);
  state.selectedContainerId = id;
  if (tab === "services") {
    servicesConsoleTarget = { kind: "container", id };
    serviceLogRequestId += 1;
  }
  state.consoleScrollContainerId = id;
  state.consoleScrollTop = 0;
  state.logState = { containerId: id, logs: "", loading: true, error: null };
  if (activeTab === "services") renderServices(true);
  else renderDocker(true);
  void loadContainerLogs(id, true, tab);
  if (focus) focusContainerCard(id);
}

function focusContainerCard(id: string): void {
  const card = [...document.querySelectorAll<HTMLElement>(".container-tile[data-action='select-container']")]
    .find((item) => item.dataset.containerId === id);
  card?.focus();
}

async function handleClick(event: Event): Promise<void> {
  const eventElement = event.target instanceof Element ? event.target : null;
  if (eventElement?.matches(".modal-backdrop")) {
    closeModal();
    return;
  }
  const target = eventElement?.closest<HTMLElement>("[data-action], [data-tab]") ?? null;
  if (!target) return;
  event.stopPropagation();
  const tab = target.dataset.tab as Tab | undefined;
  if (tab) {
    if (tab !== activeTab) {
      captureConsoleContextState();
      captureServiceContextState();
      captureDockerContextState();
      captureConsoleState();
      captureServiceConsoleState();
      captureDockerConsoleState();
    }
    activeTab = tab;
    document.querySelectorAll<HTMLElement>("[data-tab]").forEach((item) => item.classList.toggle("is-active", item.dataset.tab === tab));
    render(true);
    if (tab === "services" && servicesConsoleTarget?.kind === "service") void refreshSelectedServiceLogs();
    if (tab === "services" || tab === "docker") await refreshContainers(true);
    if (tab === "launch") await refreshLaunch(true);
    return;
  }
  const action = target.dataset.action;
  if (!action) return;
  try {
    if (action === "service-details") showServiceDetails(findService(required(target.dataset.serviceId)));
    else if (action === "select-service") selectService(required(target.dataset.serviceId), false);
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
    else if (action === "toggle-settings-info") toggleSettingsInfo(target);
    else if (action === "show-info") showInfo(required(target.dataset.infoKind));
    else if (action === "profile-details") showProfileDetails(findProfile(required(target.dataset.profileId)));
    else if (action === "close-modal") closeModal();
    else if (action === "save-settings") await saveSettingsFromModal();
    else if (action === "open-source") await openUrl(SOURCE_URL);
    else if (action === "add-profile") showProfileEditor(null);
    else if (action === "edit-profile") showProfileEditor(findProfile(required(target.dataset.profileId)));
    else if (action === "save-profile") await saveProfileFromModal(target.dataset.profileId ?? null);
    else if (action === "delete-profile") requestDeleteProfile(required(target.dataset.profileId));
    else if (action === "confirm-delete-profile") await confirmDeleteProfile(required(target.dataset.profileId));
    else if (action === "start-profile") requestLaunchAction({ kind: "profile", direction: "start", profileId: required(target.dataset.profileId) });
    else if (action === "stop-profile") requestLaunchAction({ kind: "profile", direction: "stop", profileId: required(target.dataset.profileId) });
    else if (action === "select-task") selectTask(required(target.dataset.profileId), required(target.dataset.taskName), true);
    else if (action === "start-task") requestLaunchAction({ kind: "task", direction: "start", profileId: required(target.dataset.profileId), taskName: required(target.dataset.taskName) });
    else if (action === "stop-task") requestLaunchAction({ kind: "task", direction: "stop", profileId: required(target.dataset.profileId), taskName: required(target.dataset.taskName) });
    else if (action === "confirm-launch-action") await confirmLaunchAction();
    else if (action === "toggle-log-wrap") {
      consoleWrap = !consoleWrap;
      if (activeTab === "services") renderServices(true);
      else if (activeTab === "docker") renderDocker(true);
      else if (activeTab === "launch") renderLaunch(true);
    }
    else if (action === "toggle-log-follow") {
      consoleFollow = !consoleFollow;
      if (activeTab === "services") renderServices(true);
      else if (activeTab === "docker") renderDocker(true);
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
  const taskRow = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>(".task-card") : null;
  if (taskRow && event.target === taskRow) {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      selectTask(required(taskRow.dataset.profileId), required(taskRow.dataset.taskName), true);
      return;
    }
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      const rows = [...document.querySelectorAll<HTMLElement>(".task-card")];
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

function launchTaskIsActive(state: LaunchState): boolean {
  return ["starting", "running", "stopping"].includes(state);
}

function launchProfileOperationKey(profileId: string): string {
  return `profile:${profileId}`;
}

function launchProfileHasTaskOperation(profile: LaunchProfile): boolean {
  return profile.tasks.some((task) => operations.has(`task:${profile.id}:${task.name}`));
}

function requestLaunchAction(action: PendingLaunchAction): void {
  if (pendingLaunchAction !== null) return;
  const profile = findProfile(action.profileId);
  if (operations.has(launchProfileOperationKey(profile.id))) return;
  if (action.kind === "task") {
    const task = profile.tasks.find((item) => item.name === action.taskName);
    if (!task) throw new Error("The launch task no longer exists.");
    const state = snapshotFor(profile.id, task.name)?.state ?? "stopped";
    if (operations.has(`task:${profile.id}:${task.name}`)) return;
    if (action.direction === "start" && (launchTaskIsActive(state) || state === "external")) return;
    if (action.direction === "stop" && !launchTaskIsActive(state)) return;
    pendingLaunchAction = action;
    const title = action.direction === "start" ? "Start task?" : "Stop task?";
    const description = action.direction === "start"
      ? `Start <strong>${h(task.name)}</strong> in <strong>${h(profile.name)}</strong>? This will launch the task process.`
      : `Stop <strong>${h(task.name)}</strong> in <strong>${h(profile.name)}</strong>? This will terminate the task process.`;
    const button = action.direction === "start"
      ? `<button class="primary-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("play", 13)} Start</button>`
      : `<button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("stop", 13)} Stop</button>`;
    openModal(title, `<p class="confirm-copy">${description}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button>${button}</div>`);
    return;
  }

  if (launchProfileHasTaskOperation(profile)) return;
  const canStart = profile.tasks.some((task) => {
    const state = snapshotFor(profile.id, task.name)?.state ?? "stopped";
    return !launchTaskIsActive(state) && state !== "external";
  });
  const canStop = profile.tasks.some((task) => launchTaskIsActive(snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  if (action.direction === "start" && !canStart) return;
  if (action.direction === "stop" && !canStop) return;
  pendingLaunchAction = action;
  const title = action.direction === "start" ? "Run all tasks?" : "Stop all tasks?";
  const description = action.direction === "start"
    ? `Run all tasks in <strong>${h(profile.name)}</strong>? Stopped tasks will be started; tasks that are already running will be left unchanged.`
    : `Stop all tasks? This will terminate all running or starting task processes in <strong>${h(profile.name)}</strong>.`;
  const button = action.direction === "start"
    ? `<button class="primary-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("play", 13)} Run All</button>`
    : `<button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("stop", 13)} Stop All</button>`;
  openModal(title, `<p class="confirm-copy">${description}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button>${button}</div>`);
}

async function confirmLaunchAction(): Promise<void> {
  const action = pendingLaunchAction;
  if (!action) return;
  closeModal();
  const profile = findProfile(action.profileId);
  if (action.kind === "task") {
    const task = profile.tasks.find((item) => item.name === action.taskName);
    if (!task) {
      throw new Error("The launch task is no longer available.");
    }
    const state = snapshotFor(profile.id, task.name)?.state ?? "stopped";
    if (operations.has(launchProfileOperationKey(profile.id)) || operations.has(`task:${profile.id}:${task.name}`) || (action.direction === "start" && (launchTaskIsActive(state) || state === "external")) || (action.direction === "stop" && !launchTaskIsActive(state))) {
      return;
    }
    await runTask(profile.id, task.name, action.direction === "start");
    return;
  }

  if (operations.has(launchProfileOperationKey(profile.id)) || launchProfileHasTaskOperation(profile)) return;
  const canStart = profile.tasks.some((task) => {
    const state = snapshotFor(profile.id, task.name)?.state ?? "stopped";
    return !launchTaskIsActive(state) && state !== "external";
  });
  const canStop = profile.tasks.some((task) => launchTaskIsActive(snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  if ((action.direction === "start" && !canStart) || (action.direction === "stop" && !canStop)) {
    return;
  }
  if (action.direction === "start") await startProfile(profile.id);
  else await stopProfile(profile.id);
}

function requestStopService(id: string): void {
  if (pendingServiceStopId !== null || operations.has(`stop:${id}`)) return;
  const service = findService(id);
  if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
  pendingServiceStopId = id;
  openModal("Stop service?", `<p class="confirm-copy">Stop <strong>${h(service.display_name)}</strong>? This will terminate process PID <span class="mono">${service.process?.pid ?? "unknown"}</span>.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service" data-service-id="${h(id)}">${uiIcon("stop", 13)} Stop</button></div>`);
}

function requestSaveGroup(id: string): void {
  if (pendingGroupSaveId !== null || operations.has(`group-save:${id}`)) return;
  const group = groupForId(id);
  const tasks = validateGroupProfile(group);
  if (profileForGroup(group)) return;
  pendingGroupSaveId = id;
  openModal("Save launch profile?", `<p class="confirm-copy">Save <strong>${h(group.name)}</strong> as a launch profile with ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}?</p><p class="form-note mono">${h(group.path)}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button icon-button-label" type="button" data-action="confirm-save-service-group" data-group-id="${h(id)}">${uiIcon("save", 15)} Save Profile</button></div>`);
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
  openModal("Stop all services?", `<p class="confirm-copy">Stop <strong>${candidates.length} stoppable ${candidates.length === 1 ? "service" : "services"}</strong> in ${h(group.name)}? Services that cannot be terminated safely will remain untouched.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service-group" data-group-id="${h(id)}">${uiIcon("stop", 13)} Stop All</button></div>`);
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
  openModal("Delete launch profile?", `<p class="confirm-copy">Delete <strong>${h(profile.name)}</strong>? This removes its saved commands.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button" type="button" data-action="confirm-delete-profile" data-profile-id="${h(id)}">Delete</button></div>`);
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
  if (activeTab === "docker") {
    updateDockerContainerStatuses();
    updateDockerConsoleDom();
  }
  else if (activeTab === "services") {
    updateDockerContainerStatuses();
    updateServiceConsoleDom();
  }
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
      else if (activeTab === "services") renderServices(true);
    }
  }
}

async function startProfile(id: string): Promise<void> {
  const profile = findProfile(id);
  const operationKey = launchProfileOperationKey(id);
  if (operations.has(operationKey) || launchProfileHasTaskOperation(profile)) return;
  const firstStartable = profile.tasks.find((task) => !["starting", "running", "stopping", "external"].includes(snapshotFor(id, task.name)?.state ?? "stopped"));
  if (!firstStartable) return;
  operations.add(operationKey);
  renderLaunch(true);
  try {
    setSelectedTask(id, firstStartable.name);
    for (const task of profile.tasks) {
      const state = snapshotFor(id, task.name)?.state ?? "stopped";
      if (!["starting", "running", "stopping", "external"].includes(state)) await runTask(id, task.name, true, false, false);
    }
    await refreshLaunch(true);
  } finally {
    operations.delete(operationKey);
    renderLaunch(true);
  }
}

async function stopProfile(id: string): Promise<void> {
  const profile = findProfile(id);
  const operationKey = launchProfileOperationKey(id);
  if (operations.has(operationKey) || launchProfileHasTaskOperation(profile)) return;
  const canStop = profile.tasks.some((task) => launchTaskIsActive(snapshotFor(id, task.name)?.state ?? "stopped"));
  if (!canStop) return;
  operations.add(operationKey);
  renderLaunch(true);
  try {
    taskSnapshots = mergeSnapshots(taskSnapshots, await api.stopProfile(id));
    await refreshLaunch(true);
  } finally {
    operations.delete(operationKey);
    renderLaunch(true);
  }
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
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("theme", 20)}</span><div class="settings-heading-copy"><h3 id="appearance-heading">Appearance</h3></div><button class="info-button icon-only-button settings-info-button" type="button" data-action="toggle-settings-info" data-info-kind="appearance" data-info-target="settings-info-appearance" aria-expanded="false" aria-controls="settings-info-appearance" aria-label="About appearance settings" title="About appearance settings">${uiIcon("info", 16)}</button></div>
      <p id="settings-info-appearance" class="settings-inline-info" hidden>Choose whether Cutting Board follows the system theme or always uses a light or dark appearance.</p>
      <fieldset class="choice-fieldset"><legend class="sr-only">Theme</legend><div class="theme-options">
        ${themeChoice("system", "System", "Follow your device")}${themeChoice("light", "Light", "Bright and clear")}${themeChoice("dark", "Dark", "Easy on the eyes")}
      </div></fieldset>
    </section>
    <section class="settings-section" aria-labelledby="scanning-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("scan", 20)}</span><div class="settings-heading-copy"><h3 id="scanning-heading">Scanning</h3></div><button class="info-button icon-only-button settings-info-button" type="button" data-action="toggle-settings-info" data-info-kind="scanning" data-info-target="settings-info-scanning" aria-expanded="false" aria-controls="settings-info-scanning" aria-label="About scanning settings" title="About scanning settings">${uiIcon("info", 16)}</button></div>
      <p id="settings-info-scanning" class="settings-inline-info" hidden>This controls how often Cutting Board refreshes the list of running local services. A longer interval uses less CPU.</p>
      <fieldset class="choice-fieldset interval-control"><legend class="sr-only">Scan interval</legend><div class="interval-options">
          ${intervalChoices.map((value) => `<label class="interval-choice"><input type="radio" name="scan_interval_ms" value="${value}" ${settings.scan_interval_ms === value ? "checked" : ""}><span>${h(formatInterval(value))}</span></label>`).join("")}
        </div></fieldset>
    </section>
    <div class="settings-links"><button class="source-link icon-only-button" type="button" data-action="open-source" aria-label="View Cutting Board source code on GitHub" title="Open source on GitHub">${uiIcon("github", 20)}<span class="sr-only">Open source on GitHub</span>${uiIcon("external", 16)}</button><button class="info-button icon-only-button settings-info-button" type="button" data-action="toggle-settings-info" data-info-kind="privacy" data-info-target="settings-info-privacy" aria-expanded="false" aria-controls="settings-info-privacy" aria-label="About privacy" title="About privacy">${uiIcon("info", 16)}</button></div>
    <p id="settings-info-privacy" class="settings-inline-info" hidden>Settings are stored locally on this device. Cutting Board does not collect telemetry or start automatically at login.</p>
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
  const active = profile?.tasks.some((task) => ["starting", "running", "stopping"].includes(snapshotFor(profile.id, task.name)?.state ?? "stopped")) ?? false;
  const deleteAction = profile
    ? `<button class="secondary-button danger-button icon-button-label profile-delete-action" type="button" data-action="delete-profile" data-profile-id="${h(profile.id)}" aria-label="Delete ${h(profile.name)}" title="Delete profile" ${appInfo?.demo || active ? "disabled" : ""}>${uiIcon("trash", 15)} Delete</button>`
    : "";
  openModal(profile ? "Edit Launch Profile" : "Add Launch Profile", `<form id="profile-form" class="form-stack" onsubmit="return false">
    <label>Profile name<input name="name" required maxlength="80" value="${h(profile?.name ?? "")}"></label>
    <label>Project root<div class="field-with-button"><input id="project-root" name="project_root" required value="${h(profile?.project_root ?? "")}"><button class="secondary-button" type="button" data-action="choose-root">Choose</button></div></label>
    <div class="task-editor-heading"><strong>Tasks</strong><button class="quiet-button icon-button-label" type="button" data-action="add-task-row">${uiIcon("plus", 13)} Add task</button></div>
    <div id="task-editors">${tasks.map(renderTaskEditor).join("")}</div>
    <div class="modal-actions">${deleteAction}<button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button" type="button" data-action="save-profile" ${profile ? `data-profile-id="${h(profile.id)}"` : ""}>Save</button></div>
  </form>`);
}

function renderTaskEditor(task: LaunchTask): string {
  return `<fieldset class="task-editor"><button class="remove-task" type="button" data-action="remove-task-row" aria-label="Remove task" title="Remove task">${uiIcon("trash", 15)}</button><label>Name<input data-task-field="name" required value="${h(task.name)}"></label><label>Working directory<input data-task-field="cwd" required value="${h(task.cwd)}"></label><label>Command<input data-task-field="command" required value="${h(task.command)}"></label><label>Expected port (optional)<input data-task-field="expected_port" type="number" min="1" max="65535" value="${task.expected_port ?? ""}"></label></fieldset>`;
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

function openModal(title: string, body: string): void {
  const activeElement = document.activeElement;
  modalFocusReturn = activeElement instanceof HTMLElement && activeElement !== document.body ? activeElement : null;
  byId("modal-root").innerHTML = `<div class="modal-backdrop" role="presentation"><section class="modal" tabindex="-1" role="dialog" aria-modal="true" aria-labelledby="modal-title"><header class="modal-header"><h2 id="modal-title">${h(title)}</h2><button type="button" class="modal-close" data-action="close-modal" aria-label="Close">${uiIcon("close", 18)}</button></header><div class="modal-body">${body}</div></section></div>`;
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
  pendingLaunchAction = null;
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
  return `<label class="theme-choice" title="${h(description)}"><input type="radio" name="theme_mode" value="${value}" ${settings.theme_mode === value ? "checked" : ""}><span class="theme-preview theme-preview-${value}" aria-hidden="true"><i></i><i></i><i></i></span><span class="theme-copy"><strong>${label}</strong></span><span class="choice-check">${uiIcon("check", 14)}</span></label>`;
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
function emptyState(title: string, message: string): string { return `<div class="empty-state"><h2>${h(title)}</h2><p>${h(message)}</p><button class="secondary-button icon-only-button" type="button" onclick="location.reload()" aria-label="Refresh" title="Refresh">${uiIcon("refresh", 16)}</button></div>`; }
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
