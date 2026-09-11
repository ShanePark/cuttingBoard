import "./styles.css";
import { listen } from "@tauri-apps/api/event";
import { open as choosePath } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import { renderAppShell } from "./app-shell";
import { createBoardLayout } from "./board-layout";
import { createContainerConsoleData } from "./container-console-data";
import { createConsoleController, type ConsoleOutputPatch } from "./console-controller";
import { createContainerActions } from "./container-actions";
import { createLaunchActions, launchProfileBlocksEditing, launchProfileOperationKey } from "./launch-actions";
import { patchLaunchSelection } from "./launch-dom";
import { launchProfileIsFullyStopped, orderLaunchProfiles } from "./launch-state";
import { createServiceActions } from "./service-actions";
import { createKeyboardNavigation, focusContainerCard, focusServiceCard, focusTaskRow } from "./keyboard-navigation";
import { createLaunchRefresh } from "./launch-refresh";
import { createListScroll } from "./list-scroll";
import { openModal, closeModal as closeModalView, trapModalFocus } from "./modal";
import { createModalForms, SOURCE_URL } from "./modal-forms";
import { createServiceConsoleData } from "./service-console-data";
import { updateSettingsFromRadio } from "./settings";
import { createUpdateController, UPDATE_CHECK_INTERVAL_MS } from "./update-controller";
import { createUpdateProgressView, type UpdateProgressEvent } from "./update-progress";
import { createServiceBoardGroupsSelector } from "./presentation";
import {
  launchConsoleOutputKind,
  ensureSelectedTask,
  launchTaskRefForKey,
  launchTaskRefs,
  launchTaskKey,
  selectedTaskDomKey as selectedTaskDomKeyForProfiles,
  renderConsoleOutput,
  renderLaunchConsole,
  renderProfile,
  type LaunchConsoleRenderingContext,
  type LaunchRenderingContext
} from "./launch-rendering";
import {
  containerStateText,
  dockerConsoleOutputKind,
  dockerRenderSignature,
  renderDockerLogOutput,
  renderDockerView
} from "./docker-rendering";
import type {
  ContainerTab,
  DockerConsoleRenderingContext,
  DockerRenderingContext,
  DockerTileRenderingContext,
} from "./docker-rendering";
import {
  renderGroupTitle,
  renderServiceLogOutput,
  renderServiceTile,
  renderServicesView,
  serviceConsoleOutputKind,
  servicesRenderSignature,
  uptimeText,
  type ServiceTileRenderingContext
} from "./services-rendering";
import type {
  ServicesConsoleTarget,
  ServicesRenderingContext
} from "./services-rendering";
import {
  applyTheme,
  byId,
  createUiSupport,
  ellipsis,
  emptyState,
  loadingState,
  messageOf,
  required
} from "./ui-support";
import type {
  AppInfo,
  ContainerListing,
  LaunchProfile,
  LaunchState,
  ManagedTaskSnapshot,
  SystemMetrics,
  UiSettings,
  WorkspaceSnapshot
} from "./types";

type Tab = "services" | "docker" | "launch";
const CONSOLE_LOG_POLL_INTERVAL = 750;
const SYSTEM_METRICS_POLL_INTERVAL = 2000;

const root = document.querySelector<HTMLDivElement>("#app");
if (!root) throw new Error("Missing application root");

root.innerHTML = renderAppShell();

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
let persistedSettings: UiSettings = { ...settings };
let settingsReady = false;
let workspace: WorkspaceSnapshot | null = null;
let containerListing: ContainerListing | null = null;
let profiles: LaunchProfile[] = [];
let taskSnapshots: ManagedTaskSnapshot[] = [];
let scanBusy = false;
let dockerBusy = false;
let scanTimer: number | null = null;
let consoleLogTimer: number | null = null;
let uptimeTimer: number | null = null;
let updateCheckTimer: number | null = null;
let systemMetricsTimer: number | null = null;
let systemMetricsBusy = false;
let settingsSaveQueue = Promise.resolve();
let settingsSaveRequestId = 0;
let serviceSignature = "";
let dockerSignature = "";
let launchSignature = "";
const operations = new Set<string>();
const expandedLaunchProfiles = new Set<string>();
let selectedTaskKey: string | null = null;
let selectedServiceId: string | null = null;
let servicesConsoleTarget: ServicesConsoleTarget | null = null;
let consolePollBusy = false;
const selectServiceBoardGroups = createServiceBoardGroupsSelector();
const getServiceBoardGroups = () => selectServiceBoardGroups(
  workspace?.services,
  containerListing?.available ? containerListing.containers : undefined
);

const uiSupport = createUiSupport({
  elements: { workspace: workspaceElement },
  getWorkspace: () => workspace,
  getContainerListing: () => containerListing,
  getProfiles: () => profiles,
  getSnapshots: () => taskSnapshots,
  getAppInfo: () => appInfo,
  getOperations: () => operations,
  getServiceBoardGroups
});
const {
  snapshotFor,
  findService,
  findContainer,
  findProfile,
  updateLiveMetrics,
  renderHeaderCounts,
  renderFooter,
  toast,
  showFatal
} = uiSupport;

const updateButton = byId("update-button") as HTMLButtonElement;
const systemMetricsElement = byId("system-metrics");
const updateProgressView = createUpdateProgressView();
const updateProgressListenerReady = listen<UpdateProgressEvent>("update-progress", (event) => updateProgressView.update(event.payload)).catch(() => undefined);
const updateController = createUpdateController(
  {
    checkForUpdate: api.checkForUpdate,
    updateAndRestart: api.updateAndRestart
  },
  {
    setUpdateAvailable: (available) => {
      updateButton.hidden = !available;
    },
    setUpdateBusy: (busy) => {
      updateButton.disabled = busy;
      updateButton.classList.toggle("is-busy", busy);
      if (busy) {
        updateButton.setAttribute("aria-busy", "true");
        updateButton.setAttribute("aria-label", "Updating Cutting Board");
        updateButton.title = "Updating Cutting Board — building and restarting";
      } else {
        updateButton.removeAttribute("aria-busy");
        updateButton.setAttribute("aria-label", "Update Cutting Board");
        updateButton.title = "Update available — build and restart";
      }
    },
    showUpdateStarted: () => updateProgressView.start(),
    showError: (message) => {
      updateProgressView.fail();
      toast(`Update failed: ${message}`, true);
    }
  }
);

let containerConsoleData!: ReturnType<typeof createContainerConsoleData>;
const consoleController = createConsoleController({
  elements: {
    workspace: workspaceElement,
    bottomTabs: byId("bottom-tabs")
  },
  state: {
    selectedTaskKey: () => selectedTaskKey,
    selectedTaskDomKey: () => selectedTaskDomKeyForProfiles(profiles, selectedTaskKey),
    selectedServiceId: () => selectedServiceId,
    servicesConsoleTarget: () => servicesConsoleTarget,
    container: (tab) => containerConsoleData.state(tab)
  },
  render: {
    rerender: (force) => render(force),
    service: renderServiceConsolePatch,
    docker: renderDockerConsolePatch,
    launch: renderLaunchConsolePatch
  }
});
byId("bottom-tabs").innerHTML = consoleController.renderBottomPanelTabs();

const serviceConsoleData = createServiceConsoleData({
  api: { serviceLogs: api.serviceLogs },
  getActiveTab: () => activeTab,
  getWorkspace: () => workspace,
  getProfiles: () => profiles,
  getSelection: () => ({ serviceId: selectedServiceId, consoleTarget: servicesConsoleTarget }),
  setSelection: (selection) => {
    selectedServiceId = selection.serviceId;
    servicesConsoleTarget = selection.consoleTarget;
  },
  resetServiceSelection: (serviceId) => consoleController.resetServiceSelection(serviceId),
  renderServices,
  updateServiceConsoleDom: () => consoleController.updateServiceConsoleDom(),
  messageOf,
  operations,
  workspaceElement,
  scheduler: window
});

containerConsoleData = createContainerConsoleData({
  api: { containerLogs: api.containerLogs },
  getActiveTab: () => activeTab,
  getContainerListing: () => containerListing,
  getServicesConsoleTarget: () => servicesConsoleTarget,
  setServicesConsoleTarget: (target) => { servicesConsoleTarget = target; },
  captureServicesConsoleState: () => consoleController.captureServicesConsoleState(),
  resetContainerSelection: (tab, containerId) => consoleController.resetContainerSelection(tab, containerId),
  invalidateServiceSelection: serviceConsoleData.invalidateForContainerSelection,
  updateDockerConsoleDom: () => consoleController.updateDockerConsoleDom(),
  updateServiceConsoleDom: () => consoleController.updateServiceConsoleDom(),
  renderDocker,
  renderServices,
  messageOf
});

const { applyBoardLayout, installBoardObserver } = createBoardLayout({
  workspace: workspaceElement,
  onResize: () => consoleController.applyConsoleHeight()
});

const listScroll = createListScroll(
  () => workspaceElement.querySelector<HTMLElement>(".split-view-list"),
  () => workspaceElement.dataset.view ?? null
);

const modalForms = createModalForms({
  openModal,
  getSettings: () => settings,
  getAppInfo: () => appInfo,
  getContainers: () => containerListing?.available ? containerListing.containers : [],
  getServices: () => workspace?.services ?? [],
  snapshotFor,
  blocksEditing: (profile) => launchProfileBlocksEditing(profile, snapshotFor)
});

const { refreshLaunch, refreshSelectedLaunchLogs } = createLaunchRefresh({
  api,
  getActiveTab: () => activeTab,
  getSelectedTaskKey: () => selectedTaskKey,
  setProfiles: (next) => { profiles = next; },
  setSnapshots: (next) => { taskSnapshots = next; },
  renderHeaderCounts,
  renderLaunch,
  messageOf,
  toast
});

const launchActions = createLaunchActions({
  api: {
    saveProfile: api.saveProfile,
    deleteProfile: api.deleteProfile,
    startTask: api.startTask,
    stopTask: api.stopTask,
    restartTask: api.restartTask,
    taskLogTail: api.taskLogTail,
    stopProfile: api.stopProfile
  },
  operations,
  getProfiles: () => profiles,
  setProfiles: (next) => { profiles = next; },
  getSnapshots: () => taskSnapshots,
  setSnapshots: (next) => { taskSnapshots = next; },
  snapshotFor,
  setSelectedTask,
  focusTaskConsole,
  updateTaskLogTail,
  renderLaunch,
  refreshLaunch,
  renderHeaderCounts,
  hasProfileForm: modalForms.hasProfileForm,
  readProfileForm: modalForms.readProfileForm,
  openModal,
  closeModal,
  toast
});

const serviceActions = createServiceActions({
  api: {
    terminate: api.terminate,
    restartService: api.restartService,
    restartTask: api.restartTask,
    taskLogTail: api.taskLogTail,
    saveProfile: api.saveProfile
  },
  operations,
  getWorkspace: () => workspace,
  getContainers: () => containerListing?.available ? containerListing.containers : [],
  getProfiles: () => profiles,
  setProfiles: (next) => { profiles = next; },
  refreshWorkspace,
  renderServices,
  renderCurrentView: render,
  renderHeaderCounts,
  openUrl,
  openModal,
  closeModal,
  toast,
  focusServiceConsole: (serviceId) => focusServiceConsole(serviceId),
  beginRestartProgress: beginServiceRestartProgress,
  updateRestartProgress: updateServiceRestartProgress,
  finishRestartProgress: finishServiceRestartProgress,
  isRestartingService: serviceRestartInProgress
});

const containerActions = createContainerActions({
  api,
  operations,
  getContainerListing: () => containerListing,
  getActiveTab: () => activeTab,
  refreshContainers,
  renderDocker,
  renderServices,
  updateDockerContainerStatuses,
  updateDockerConsoleDom: () => consoleController.updateDockerConsoleDom(),
  updateServiceConsoleDom: () => consoleController.updateServiceConsoleDom(),
  toast,
  messageOf
});

const keyboardNavigation = createKeyboardNavigation({
  handleResizeKey: (event) => consoleController.handleResizeKey(event),
  selectTask,
  selectContainer
});

root.addEventListener("click", (event) => {
  if (updateProgressView.isActive()) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  void handleClick(event);
});
root.addEventListener("change", (event) => {
  if (updateProgressView.isActive()) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  handleChange(event);
});
root.addEventListener("keydown", (event) => {
  if (updateProgressView.isActive()) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  if (!consoleController.handleOutputKey(event)) keyboardNavigation.handleKeyboard(event);
});
root.addEventListener("beforeinput", (event) => {
  consoleController.handleOutputMutation(event);
});
root.addEventListener("input", (event) => {
  if (!consoleController.handleSearchInput(event)) consoleController.handleOutputMutation(event);
});
root.addEventListener("paste", (event) => {
  consoleController.handleOutputMutation(event);
});
root.addEventListener("cut", (event) => {
  consoleController.handleOutputMutation(event);
});
root.addEventListener("drop", (event) => {
  consoleController.handleOutputMutation(event);
});
root.addEventListener("dragover", (event) => {
  consoleController.handleOutputMutation(event);
});
root.addEventListener("pointerdown", (event) => {
  if (updateProgressView.isActive()) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  consoleController.handleResizeStart(event);
});
root.addEventListener("scroll", (event) => {
  if (!updateProgressView.isActive()) consoleController.handleScroll(event);
}, true);
document.addEventListener("keydown", (event) => {
  if (updateProgressView.isActive()) {
    event.preventDefault();
    event.stopPropagation();
    return;
  }
  if (!document.querySelector(".modal-backdrop")) return;
  if (event.key === "Escape") closeModal();
  else if (event.key === "Tab") trapModalFocus(event);
});

void bootstrap();

async function bootstrap(): Promise<void> {
  try {
    const [info, loadedSettings, loadedProfiles] = await Promise.all([
      api.appInfo(), api.loadSettings(), api.profiles()
    ]);
    appInfo = info;
    settings = loadedSettings;
    persistedSettings = { ...loadedSettings };
    profiles = loadedProfiles;
    applyTheme(settings.theme_mode);
    renderHeaderCounts();

    // Scan before loading task snapshots so processes that survived an earlier
    // Cutting Board session are matched back to their launch tasks immediately.
    await refreshWorkspace(true);
    taskSnapshots = await api.taskSnapshots();
    renderHeaderCounts();
    render(true);
    consoleController.applyConsoleHeight();
    await refreshSystemMetrics();
    await updateProgressListenerReady;
    installTimers();
    installBoardObserver();
    settingsReady = true;
    if (appInfo.update_supported && !appInfo.demo) void updateController.checkForUpdate();
  } catch (error) {
    showFatal(error);
  }
}

function installTimers(): void {
  if (scanTimer !== null) window.clearInterval(scanTimer);
  scanTimer = window.setInterval(() => void refreshWorkspace(), Math.max(500, settings.scan_interval_ms));
  if (consoleLogTimer !== null) window.clearInterval(consoleLogTimer);
  consoleLogTimer = window.setInterval(() => void pollActiveConsoleLogs(), CONSOLE_LOG_POLL_INTERVAL);
  if (uptimeTimer !== null) window.clearInterval(uptimeTimer);
  uptimeTimer = window.setInterval(updateLiveMetrics, 1000);
  if (updateCheckTimer !== null) window.clearInterval(updateCheckTimer);
  updateCheckTimer = null;
  if (appInfo?.update_supported && !appInfo.demo) {
    updateCheckTimer = window.setInterval(() => void updateController.checkForUpdate(), UPDATE_CHECK_INTERVAL_MS);
  }
  if (systemMetricsTimer !== null) window.clearInterval(systemMetricsTimer);
  systemMetricsTimer = window.setInterval(() => void refreshSystemMetrics(), SYSTEM_METRICS_POLL_INTERVAL);
}

async function refreshSystemMetrics(): Promise<void> {
  if (systemMetricsBusy) return;
  systemMetricsBusy = true;
  try {
    updateSystemMetrics(await api.systemMetrics());
  } catch {
    updateSystemMetrics(null);
  } finally {
    systemMetricsBusy = false;
  }
}

function updateSystemMetrics(metrics: SystemMetrics | null): void {
  const valueText = (value: number | null | undefined): string =>
    value === null || value === undefined || !Number.isFinite(value) ? "—" : `${Math.round(value)}%`;
  const cpuText = valueText(metrics?.cpu_percent);
  const memoryText = valueText(metrics?.memory_percent);
  const cpuValue = systemMetricsElement.querySelector<HTMLElement>('[data-system-metric-value="cpu"]');
  const memoryValue = systemMetricsElement.querySelector<HTMLElement>('[data-system-metric-value="memory"]');
  if (cpuValue && cpuValue.textContent !== cpuText) cpuValue.textContent = cpuText;
  if (memoryValue && memoryValue.textContent !== memoryText) memoryValue.textContent = memoryText;
  systemMetricsElement.setAttribute("aria-label", `System resource usage: CPU ${cpuText}, memory ${memoryText}`);
}

async function pollActiveConsoleLogs(): Promise<void> {
  if (consolePollBusy || !consoleController.isPanelOpen("console")) return;
  consolePollBusy = true;
  try {
    if (activeTab === "services") {
      if (servicesConsoleTarget?.kind === "service") await refreshSelectedServiceLogs();
      else if (servicesConsoleTarget?.kind === "container") await refreshSelectedContainerLogs();
      return;
    }
    if (activeTab === "docker") {
      await refreshSelectedContainerLogs();
      return;
    }
    await refreshSelectedLaunchLogs();
  } finally {
    consolePollBusy = false;
  }
}

async function refreshWorkspace(force = false): Promise<void> {
  if (scanBusy) return;
  scanBusy = true;
  try {
    workspace = await api.scan();
    remapServiceSelectionForRestart();
    syncSelectedService();
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

function syncSelectedService(): void {
  serviceConsoleData.syncSelectedService();
}

function remapServiceSelectionForRestart(): void {
  serviceConsoleData.remapServiceSelectionForRestart();
}

function focusServiceConsole(serviceId: string): void {
  if (activeTab !== "services") {
    consoleController.captureLaunchConsoleState();
    consoleController.captureServicesConsoleState();
    serviceConsoleData.stopServiceLogElapsedTimer();
    activeTab = "services";
    document.querySelectorAll<HTMLElement>("[data-tab]").forEach((item) => item.classList.toggle("is-active", item.dataset.tab === "services"));
  }
  selectService(serviceId);
  if (!consoleController.isPanelOpen("console")) consoleController.toggleBottomPanel("console");
}

function beginServiceRestartProgress(serviceId: string, profileId: string, taskName: string): void {
  serviceConsoleData.beginServiceRestartProgress(serviceId, profileId, taskName);
}

function updateServiceRestartProgress(profileId: string, taskName: string, logTail: string): void {
  serviceConsoleData.updateServiceRestartProgress(profileId, taskName, logTail);
}

function finishServiceRestartProgress(profileId: string, taskName: string, succeeded: boolean): void {
  serviceConsoleData.finishServiceRestartProgress(profileId, taskName, succeeded);
}

function serviceRestartInProgress(serviceId: string): boolean {
  return serviceConsoleData.serviceRestartInProgress(serviceId);
}

function clearServiceSelection(): void {
  serviceConsoleData.clearServiceSelection();
}

async function refreshSelectedServiceLogs(): Promise<void> {
  const selectedServiceId = serviceConsoleData.selectedServiceId();
  const service = serviceConsoleData.currentService();
  const serviceRestartProgress = serviceConsoleData.restartProgress();
  if (serviceConsoleData.selectedConsoleTarget()?.kind !== "service" || !selectedServiceId) return;
  if (!service) {
    if (serviceRestartProgress?.serviceId === selectedServiceId) return;
    clearServiceSelection();
    return;
  }
  await serviceConsoleData.refreshSelectedServiceLogs();
}

async function refreshContainers(force = false): Promise<void> {
  if (dockerBusy) return;
  dockerBusy = true;
  try {
    containerListing = await api.containers();
    syncSelectedContainer();
    renderHeaderCounts();
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
  containerConsoleData.syncSelectedContainer();
}

function clearDockerSelection(): void {
  containerConsoleData.clearDockerSelection();
}

async function refreshSelectedContainerLogs(): Promise<void> {
  await containerConsoleData.refreshSelectedContainerLogs();
}

function render(force = false): void {
  if (activeTab === "services") renderServices(force);
  else if (activeTab === "docker") renderDocker(force);
  else renderLaunch(force);
}

function servicesRenderingContext(): ServicesRenderingContext {
  const containerTab = containerConsoleData.activeContainerTab();
  return {
    workspace,
    services: workspace?.services.filter((service) => service.relevance === "dev") ?? [],
    containers: containerListing?.available ? containerListing.containers : [],
    boardGroups: getServiceBoardGroups(),
    selection: {
      selectedServiceId,
      selectedContainerId: containerConsoleData.state("services").selectedContainerId,
      container: {
        tab: containerTab,
        selectedId: containerConsoleData.state(containerTab).selectedContainerId
      },
      consoleTarget: servicesConsoleTarget,
      activeBottomPanel: consoleController.activePanel()
    },
    tile: serviceTileRenderingContext(),
    console: {
      open: consoleController.isPanelOpen("console"),
      serviceLogState: serviceConsoleData.logState(),
      restartProgress: serviceConsoleData.restartProgress(),
      serviceLogElapsedSeconds: serviceConsoleData.serviceLogElapsedSeconds,
      docker: dockerConsoleRenderingContext("services"),
      renderConsoleResizer: () => consoleController.renderConsoleResizer(),
      renderConsoleJumpButton: () => consoleController.renderConsoleJumpButton()
    },
    containerOperationBusy,
    profileForGroup: serviceActions.profileForGroup,
    loadingState,
    emptyState
  };
}

function renderServices(force = false): void {
  consoleController.captureServicesConsoleState();
  listScroll.capture();
  const renderingContext = servicesRenderingContext();
  const signature = servicesRenderSignature(renderingContext);
  if (!force && signature === serviceSignature && workspaceElement.dataset.view === "services") {
    updateDockerContainerStatuses();
    consoleController.updateServiceConsoleDom();
    updateLiveMetrics();
    return;
  }
  serviceSignature = signature;
  workspaceElement.dataset.view = "services";
  if (!renderingContext.workspace) {
    workspaceElement.innerHTML = renderServicesView(renderingContext);
    consoleController.restoreServicesConsoleState();
    return;
  }
  workspaceElement.innerHTML = renderServicesView(renderingContext);
  if (renderingContext.services.length === 0) {
    consoleController.restoreServicesConsoleState();
    return;
  }
  applyBoardLayout();
  listScroll.restore("services");
  consoleController.restoreServicesConsoleState();
  updateLiveMetrics();
}

function renderServiceConsolePatch(serviceId: string | null): ConsoleOutputPatch {
  const service = serviceId
    ? workspace?.services.find((item) => item.relevance === "dev" && item.id === serviceId) ?? null
    : null;
  const serviceLogState = serviceConsoleData.logState();
  const state = service && serviceLogState.serviceId === service.id ? serviceLogState : null;
  const renderingContext = servicesRenderingContext();
  const restartLog = renderingContext.console.restartProgress
    && (!service || renderingContext.console.restartProgress.serviceId === service.id)
    ? renderingContext.console.restartProgress.logTail
    : null;
  return {
    markup: renderServiceLogOutput(service, renderingContext.console),
    kind: serviceConsoleOutputKind(service, renderingContext.console),
    log: restartLog ?? state?.logs ?? ""
  };
}

function renderDockerConsolePatch(tab: ContainerTab, containerId: string | null): ConsoleOutputPatch {
  const state = containerConsoleData.state(tab);
  const container = containerId
    ? containerListing?.containers.find((item) => item.id === containerId) ?? null
    : null;
  return {
    markup: renderDockerLogOutput(container, state.logState),
    kind: dockerConsoleOutputKind(container, state.logState),
    log: container && state.logState.containerId === container.id ? state.logState.logs : ""
  };
}

function renderLaunchConsolePatch(): ConsoleOutputPatch | null {
  const ref = launchTaskRefForKey(profiles, selectedTaskKey);
  if (!ref) return null;
  const snapshot = snapshotFor(ref.profile.id, ref.task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  return {
    markup: renderConsoleOutput(snapshot, state),
    kind: launchConsoleOutputKind(snapshot, state),
    log: snapshot?.log_tail ?? ""
  };
}

function dockerTileRenderingContext(tab: ContainerTab): DockerTileRenderingContext {
  return {
    activeTab: tab,
    activeBottomPanel: consoleController.activePanel(),
    selectedContainerId: containerConsoleData.state(tab).selectedContainerId,
    servicesConsoleTargetKind: servicesConsoleTarget?.kind ?? null,
    containerOperationBusy
  };
}

function serviceTileRenderingContext(): ServiceTileRenderingContext {
  return {
    operations,
    selectedServiceId,
    consoleTargetKind: servicesConsoleTarget?.kind ?? null,
    restartingServiceId: serviceConsoleData.restartProgress()?.serviceId ?? null
  };
}

function dockerConsoleRenderingContext(tab: ContainerTab): DockerConsoleRenderingContext {
  const viewState = containerConsoleData.state(tab);
  const container = viewState.selectedContainerId
    ? containerListing?.containers.find((item) => item.id === viewState.selectedContainerId) ?? null
    : null;
  return {
    consoleOpen: consoleController.isPanelOpen("console"),
    container,
    logState: viewState.logState,
    containerOperationBusy,
    renderConsoleResizer: () => consoleController.renderConsoleResizer(),
    renderConsoleJumpButton: () => consoleController.renderConsoleJumpButton()
  };
}

function dockerRenderingContext(): DockerRenderingContext {
  const fallbackServices = workspace?.services.filter((service) => service.relevance === "container") ?? [];
  const serviceTileContext = serviceTileRenderingContext();
  return {
    containerListing,
    fallbackServices,
    tile: dockerTileRenderingContext("docker"),
    console: dockerConsoleRenderingContext("docker"),
    renderFallbackServiceTile: (service, ordinal, ordinalTotal) => renderServiceTile(service, ordinal, ordinalTotal, { info: false, stop: service.can_terminate, open: Boolean(service.browser_url) }, serviceTileContext),
    renderGroupTitle,
    loadingState,
    emptyState
  };
}

function renderDocker(force = false): void {
  consoleController.captureDockerConsoleState();
  listScroll.capture();
  const renderingContext = dockerRenderingContext();
  const signature = dockerRenderSignature(renderingContext);
  if (!force && signature === dockerSignature && workspaceElement.dataset.view === "docker") {
    updateDockerContainerStatuses();
    consoleController.updateDockerConsoleDom();
    return;
  }
  dockerSignature = signature;
  workspaceElement.dataset.view = "docker";
  if (!renderingContext.containerListing) {
    workspaceElement.innerHTML = renderDockerView(renderingContext);
    void refreshContainers();
    return;
  }
  workspaceElement.innerHTML = renderDockerView(renderingContext);
  if (!renderingContext.containerListing.available) {
    if (renderingContext.fallbackServices.length) {
      applyBoardLayout();
      listScroll.restore("docker");
      consoleController.restoreDockerConsoleState();
      return;
    }
    consoleController.restoreDockerConsoleState();
    return;
  }
  if (renderingContext.containerListing.containers.length === 0) {
    consoleController.restoreDockerConsoleState();
    return;
  }
  applyBoardLayout();
  listScroll.restore("docker");
  consoleController.restoreDockerConsoleState();
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

function renderLaunch(force = false): void {
  consoleController.captureLaunchConsoleState();
  listScroll.capture();
  // Launch card metrics point at scanned service IDs, which change when a restart creates a replacement process.
  const launchServiceIds = workspace?.services
    .filter((service) => service.relevance === "dev")
    .map((service) => service.id)
    .sort() ?? [];
  const signature = JSON.stringify([
    profiles,
    taskSnapshots.map(({ log_tail: _logTail, ...snapshot }) => snapshot),
    appInfo?.demo,
    launchServiceIds,
    // Container task icons follow the image behind the container, so the listing is part of the view.
    containerListing?.available ? containerListing.containers.map((container) => `${container.name}=${container.image}`).sort() : [],
    [...operations],
    consoleController.activePanel()
  ]);
  if (!force && signature === launchSignature && workspaceElement.dataset.view === "launch") {
    consoleController.updateLaunchConsoleDom();
    return;
  }
  launchSignature = signature;
  workspaceElement.dataset.view = "launch";
  const consoleRenderingContext = launchConsoleRenderingContext();
  if (profiles.length === 0) {
    selectedTaskKey = null;
    workspaceElement.innerHTML = `<div class="launch-view split-view"><div class="split-view-list">${emptyState("No launch profiles saved", "Go to Services and save a service group to register a launch profile.")}</div>${renderLaunchConsole(null, consoleRenderingContext)}</div>`;
    applyBoardLayout();
    return;
  }
  const selected = ensureSelectedTask(profiles, selectedTaskKey, snapshotFor);
  selectedTaskKey = selected ? launchTaskKey(selected.profile.id, selected.task.name) : null;
  const selectedRenderingContext = launchRenderingContext();
  const orderedProfiles = orderLaunchProfiles(profiles, snapshotFor);
  workspaceElement.innerHTML = `<div class="launch-view split-view"><div class="split-view-list"><div class="launch-list board">${orderedProfiles.map((profile) => renderProfile(profile, selectedRenderingContext)).join("")}</div></div>${renderLaunchConsole(selected, consoleRenderingContext)}</div>`;
  applyBoardLayout();
  listScroll.restore("launch");
  consoleController.restoreLaunchConsoleScroll();
}

function launchRenderingContext(): LaunchRenderingContext {
  return {
    appIsDemo: Boolean(appInfo?.demo),
    operations,
    selectedTaskKey,
    services: workspace?.services ?? [],
    containers: containerListing?.available ? containerListing.containers : [],
    snapshotFor,
    launchProfileOperationKey,
    launchProfileHasTaskOperation: launchActions.hasTaskOperation,
    expandedProfiles: expandedLaunchProfiles,
    renderGroupTitle,
    uptimeText
  };
}

function launchConsoleRenderingContext(): LaunchConsoleRenderingContext {
  return {
    consoleOpen: consoleController.isPanelOpen("console"),
    snapshotFor,
    renderConsoleResizer: () => consoleController.renderConsoleResizer(),
    renderConsoleJumpButton: () => consoleController.renderConsoleJumpButton()
  };
}

function selectTask(profileId: string, taskName: string, focus = false): void {
  const ref = launchTaskRefs(profiles).find(({ profile, task }) => profile.id === profileId && task.name === taskName);
  if (!ref) throw new Error("The launch task no longer exists.");
  setSelectedTask(profileId, taskName);
  // Patch the selection in place: a full rebuild re-lays out every card and repaints the board just to move a highlight.
  if (patchLaunchSelection(workspaceElement, selectedTaskKey, renderLaunchConsole(ref, launchConsoleRenderingContext()))) {
    consoleController.restoreLaunchConsoleScroll();
  } else {
    renderLaunch(true);
  }
  if (focus) focusTaskRow(profileId, taskName);
}

function focusTaskConsole(profileId: string, taskName: string): void {
  selectTask(profileId, taskName);
  if (!consoleController.isPanelOpen("console")) consoleController.toggleBottomPanel("console");
}

function updateTaskLogTail(profileId: string, taskName: string, logTail: string): void {
  const current = snapshotFor(profileId, taskName);
  if (!current) return;
  taskSnapshots = taskSnapshots.map((snapshot) => snapshot.profile_id === profileId && snapshot.task_name === taskName
    ? { ...snapshot, log_tail: logTail }
    : snapshot);
  if (activeTab === "launch" && selectedTaskKey === launchTaskKey(profileId, taskName)) {
    consoleController.updateLaunchConsoleDom();
  }
}

function selectService(id: string, focus = false): void {
  const service = findService(id);
  if (service.relevance !== "dev") throw new Error("This service is not available in the Services view.");
  consoleController.captureServicesConsoleState();
  serviceConsoleData.selectService(id);
  if (focus) focusServiceCard(id);
}

function setSelectedTask(profileId: string, taskName: string): void {
  const key = launchTaskKey(profileId, taskName);
  if (selectedTaskKey === key) return;
  selectedTaskKey = key;
  consoleController.resetTaskSelection(key);
}

function selectContainer(id: string, focus = false): void {
  containerConsoleData.selectContainer(id);
  if (focus) focusContainerCard(id);
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
      consoleController.captureLaunchConsoleState();
      consoleController.captureServicesConsoleState();
      if (activeTab === "services") serviceConsoleData.stopServiceLogElapsedTimer();
    }
    activeTab = tab;
    document.querySelectorAll<HTMLElement>("[data-tab]").forEach((item) => item.classList.toggle("is-active", item.dataset.tab === tab));
    render(true);
    if (tab === "services") serviceConsoleData.resumeServiceLogElapsedTimer();
    if (tab === "services" && servicesConsoleTarget?.kind === "service") void refreshSelectedServiceLogs();
    if (tab === "services" || tab === "docker") await refreshContainers(true);
    if (tab === "launch") await refreshLaunch(true);
    return;
  }
  const action = target.dataset.action;
  if (!action) return;
  try {
    if (action === "service-details") modalForms.showServiceDetails(findService(required(target.dataset.serviceId)));
    else if (action === "select-service") selectService(required(target.dataset.serviceId), false);
    else if (action === "open-service") await serviceActions.openService(required(target.dataset.serviceId));
    else if (action === "restart-service") serviceActions.requestRestartService(required(target.dataset.serviceId));
    else if (action === "confirm-restart-service") await serviceActions.confirmRestartService(required(target.dataset.serviceId));
    else if (action === "stop-service") serviceActions.requestStopService(required(target.dataset.serviceId));
    else if (action === "confirm-stop-service") await serviceActions.confirmStopService(required(target.dataset.serviceId));
    else if (action === "save-service-group") serviceActions.requestSaveGroup(required(target.dataset.groupId));
    else if (action === "confirm-save-service-group") await serviceActions.confirmSaveGroup(required(target.dataset.groupId));
    else if (action === "stop-service-group") serviceActions.requestStopGroup(required(target.dataset.groupId));
    else if (action === "confirm-stop-service-group") await serviceActions.confirmStopGroup(required(target.dataset.groupId));
    else if (action === "group-details") modalForms.showGroupDetails(serviceActions.groupForId(required(target.dataset.groupId)));
    else if (action === "container-group-details") modalForms.showContainerGroupDetails(required(target.dataset.groupName));
    else if (action === "select-container") selectContainer(required(target.dataset.containerId), false);
    else if (action === "start-container") await containerActions.operateContainer(required(target.dataset.containerId), true);
    else if (action === "stop-container") await containerActions.operateContainer(required(target.dataset.containerId), false);
    else if (action === "container-details") modalForms.showContainerDetails(findContainer(required(target.dataset.containerId)));
    else if (action === "settings") {
      if (settingsReady) modalForms.showSettings();
    }
    else if (action === "update" && appInfo?.update_supported && !appInfo.demo) await updateController.updateAndRestart();
    else if (action === "toggle-settings-info") modalForms.toggleSettingsInfo(target);
    else if (action === "show-info") modalForms.showInfo(required(target.dataset.infoKind));
    else if (action === "profile-details") modalForms.showProfileDetails(findProfile(required(target.dataset.profileId)));
    else if (action === "task-details") modalForms.showTaskDetails(findProfile(required(target.dataset.profileId)), required(target.dataset.taskName));
    else if (action === "close-modal") closeModal();
    else if (action === "open-source") await openUrl(SOURCE_URL);
    else if (action === "toggle-profile") {
      const profileId = required(target.dataset.profileId);
      const profile = findProfile(profileId);
      const states = profile.tasks.map((task) => snapshotFor(profile.id, task.name)?.state ?? "stopped");
      if (!launchProfileIsFullyStopped(states)) return;
      if (expandedLaunchProfiles.has(profileId)) expandedLaunchProfiles.delete(profileId);
      else expandedLaunchProfiles.add(profileId);
      renderLaunch(true);
      [...workspaceElement.querySelectorAll<HTMLButtonElement>('[data-action="toggle-profile"]')]
        .find((button) => button.dataset.profileId === profileId)
        ?.focus();
    }
    else if (action === "edit-profile") modalForms.showProfileEditor(findProfile(required(target.dataset.profileId)));
    else if (action === "save-profile") await launchActions.saveProfileFromModal(target.dataset.profileId ?? null);
    else if (action === "delete-profile") launchActions.requestDeleteProfile(required(target.dataset.profileId));
    else if (action === "confirm-delete-profile") await launchActions.confirmDeleteProfile(required(target.dataset.profileId));
    else if (action === "start-profile") await launchActions.requestLaunchAction({ kind: "profile", direction: "start", profileId: required(target.dataset.profileId) });
    else if (action === "stop-profile") await launchActions.requestLaunchAction({ kind: "profile", direction: "stop", profileId: required(target.dataset.profileId) });
    else if (action === "select-task") selectTask(required(target.dataset.profileId), required(target.dataset.taskName), true);
    else if (action === "start-task") await launchActions.requestLaunchAction({ kind: "task", direction: "start", profileId: required(target.dataset.profileId), taskName: required(target.dataset.taskName) });
    else if (action === "restart-task") await launchActions.requestLaunchAction({ kind: "task", direction: "restart", profileId: required(target.dataset.profileId), taskName: required(target.dataset.taskName) });
    else if (action === "stop-task") await launchActions.requestLaunchAction({ kind: "task", direction: "stop", profileId: required(target.dataset.profileId), taskName: required(target.dataset.taskName) });
    else if (action === "confirm-launch-action") await launchActions.confirmLaunchAction();
    else if (action === "jump-to-bottom") consoleController.jumpToBottom(target);
    else if (action === "console-search-next" || action === "console-search-previous" || action === "close-console-search") consoleController.handleSearchAction(action, target);
    else if (action === "toggle-bottom-panel") consoleController.toggleBottomPanel(required(target.dataset.panelId));
    else if (action === "choose-root") await chooseProfileRoot();
    else if (action === "add-task-row") modalForms.addTaskRow();
    else if (action === "remove-task-row") target.closest(".task-editor")?.remove();
  } catch (error) {
    toast(messageOf(error), true);
  }
}

function handleChange(event: Event): void {
  const target = event.target instanceof HTMLInputElement ? event.target : null;
  if (!settingsReady || !target || target.type !== "radio" || !target.checked || !target.closest("#settings-form")) return;
  try {
    settings = updateSettingsFromRadio(settings, target.name, target.value);
    applyTheme(settings.theme_mode);
    installTimers();
    render(true);
    queueSettingsSave(settings);
  } catch (error) {
    toast(messageOf(error), true);
  }
}

function queueSettingsSave(nextSettings: UiSettings): void {
  const requestId = ++settingsSaveRequestId;
  settingsSaveQueue = settingsSaveQueue.then(async () => {
    try {
      const saved = await api.saveSettings(nextSettings);
      persistedSettings = { ...saved };
      if (requestId !== settingsSaveRequestId) return;
      settings = { ...saved };
      applyTheme(settings.theme_mode);
      installTimers();
      render(true);
      toast("Settings saved.");
    } catch (error) {
      if (requestId !== settingsSaveRequestId) return;
      rollbackSettingsToPersisted();
      toast(messageOf(error), true);
    }
  });
}

function rollbackSettingsToPersisted(): void {
  settings = { ...persistedSettings };
  applyTheme(settings.theme_mode);
  installTimers();
  render(true);
  syncSettingsForm();
}

function syncSettingsForm(): void {
  const form = document.querySelector<HTMLFormElement>("#settings-form");
  if (!form) return;
  form.querySelectorAll<HTMLInputElement>("input[type='radio']").forEach((input) => {
    if (input.name === "theme_mode") input.checked = input.value === settings.theme_mode;
    else if (input.name === "scan_interval_ms") input.checked = Number(input.value) === settings.scan_interval_ms;
  });
}

async function chooseProfileRoot(): Promise<void> {
  const result = await choosePath({ directory: true, multiple: false, title: "Choose project root" });
  const path = Array.isArray(result) ? result[0] : result;
  if (path) {
    const input = document.querySelector<HTMLInputElement>("#project-root");
    if (input) input.value = path;
  }
}

function closeModal(): void {
  serviceActions.resetPending();
  launchActions.resetPending();
  closeModalView();
}
