import { restartIcon, techIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import {
  FRESH_UPTIME_SECONDS,
  currentUptime,
  formatBytes,
  boardGroupCards,
  formatUptimeCompact,
  serviceBoardGroups,
  serviceTitle,
  uniquePorts,
  type ServiceBoardGroup
} from "./presentation";
import { renderContainerTile, renderDockerConsole, type ContainerTab, type DockerConsoleRenderingContext, type DockerTileRenderingContext } from "./docker-rendering";
import { renderGroupCount, renderOpenServiceButton, renderSharedServiceCard } from "./tile-rendering";
import type {
  ContainerInfo,
  LaunchProfile,
  LaunchTask,
  ServiceSnapshot,
  WorkspaceSnapshot
} from "./types";

export type ServiceLogState = {
  serviceId: string | null;
  logs: string;
  available: boolean;
  loading: boolean;
  loadingStartedAt: number | null;
  message: string | null;
  error: string | null;
};

export type ServicesConsoleTarget =
  | { kind: "service"; id: string }
  | { kind: "container"; id: string };

export type ServicesSelectionContext = {
  selectedServiceId: string | null;
  selectedContainerId: string | null;
  container: {
    tab: ContainerTab;
    selectedId: string | null;
  };
  consoleTarget: ServicesConsoleTarget | null;
  activeBottomPanel: string | null;
};

export type ServiceTileRenderingContext = {
  operations: ReadonlySet<string>;
  selectedServiceId: string | null;
  consoleTargetKind: "service" | "container" | null;
};

export type ServicesConsoleRenderingContext = {
  open: boolean;
  serviceLogState: ServiceLogState;
  serviceLogElapsedSeconds: (state: ServiceLogState) => number;
  docker: DockerConsoleRenderingContext;
  renderConsoleResizer: () => string;
  renderConsoleJumpButton: () => string;
};

export type ServicesRenderingContext = {
  workspace: WorkspaceSnapshot | null;
  services: readonly ServiceSnapshot[];
  containers: readonly ContainerInfo[];
  selection: ServicesSelectionContext;
  tile: ServiceTileRenderingContext;
  console: ServicesConsoleRenderingContext;
  containerOperationBusy: (id: string) => boolean;
  profileForGroup: (group: ServiceBoardGroup) => LaunchProfile | undefined;
  loadingState: (message: string) => string;
  emptyState: (title: string, message: string) => string;
};

export function servicesRenderSignature(context: ServicesRenderingContext): string {
  const groups = serviceGroups(context);
  return JSON.stringify([
    context.services.map((service) => [
      service.id, service.display_name, service.tech, uniquePorts(service), service.category, service.status,
      service.origin_kind, service.origin_label, service.can_terminate, service.browser_url,
      service.project?.id, service.project?.name, service.project?.root_path,
      service.project?.workspace_name, service.project?.workspace_root_path,
      service.process?.pid, service.process?.name,
      service.process?.executable, service.process?.working_directory, service.process?.command,
      service.process?.launch_command, service.process?.create_time,
      service.active_profiles,
      context.tile.operations.has(`stop:${service.id}`),
      context.tile.operations.has(`restart:${service.id}`)
    ]),
    groups.map((group) => [
      group.id,
      group.containers.map((container) => [
        container.id, container.name, container.image, container.state, container.status,
        container.ports, container.compose_project, container.compose_service, container.compose_working_dir,
        context.containerOperationBusy(container.id)
      ])
    ]),
    context.selection.selectedServiceId,
    context.selection.selectedContainerId,
    context.selection.consoleTarget,
    context.selection.activeBottomPanel
  ]);
}

export function renderServicesView(context: ServicesRenderingContext): string {
  if (!context.workspace) {
    return `<div class="services-view split-view"><div class="split-view-list">${context.loadingState("Finding services")}</div>${renderServiceConsole(context)}</div>`;
  }
  if (context.services.length === 0) {
    return `<div class="services-view split-view"><div class="split-view-list">${context.emptyState("No development services are running", "Start a local server from a terminal, agent, or IDE.")}</div>${renderServiceConsole(context)}</div>`;
  }
  const groups = serviceGroups(context);
  return `<div class="services-view split-view"><div class="split-view-list"><div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.services.length + group.containers.length}" aria-labelledby="group-${h(encodeURIComponent(group.id))}">
      <header class="section-header">
        <span class="section-accent accent-${h(group.accent)}"></span>
        <h2 id="group-${h(encodeURIComponent(group.id))}">${renderGroupTitle(group.name, group.services.length, "group-details", `data-group-id="${h(group.id)}"`, group.name.toUpperCase())}${renderGroupCount(boardGroupCards(group))}</h2>
        ${renderGroupActions(group, context)}
      </header>
      <div class="tile-grid">${renderServiceGroupTiles(group, context)}</div>
    </section>`).join("")}</div></div>${renderServiceConsole(context)}</div>`;
}

export function renderGroupTitle(title: string, itemCount: number, action: string, attributes: string, displayTitle = title, actionTitle = "View group details"): string {
  const escapedTitle = h(title);
  return itemCount > 1
    ? `<button class="group-title-button" type="button" data-action="${h(action)}" ${attributes} aria-label="View ${escapedTitle} details" title="${h(actionTitle)}">${h(displayTitle)}</button>`
    : `<span class="group-title-text" title="${escapedTitle}">${h(displayTitle)}</span>`;
}

export function renderGroupActions(group: ServiceBoardGroup, context: ServicesRenderingContext): string {
  const saveBusy = context.tile.operations.has(`group-save:${group.id}`);
  const stopBusy = context.tile.operations.has(`group-stop:${group.id}`);
  const profilePresent = context.profileForGroup(group) !== undefined;
  const saveAction = profilePresent
    ? `<span class="group-profile-saved" title="Launch profile saved" aria-label="Launch profile saved">${uiIcon("check", 17)}</span>`
    : `<button class="section-action icon-only-button save-group-action" type="button" data-action="save-service-group" data-group-id="${h(group.id)}" title="${saveBusy ? "Saving launch profile" : "Save launch profile"}" aria-label="${saveBusy ? "Saving" : "Save"} launch profile for ${h(group.name)}" ${saveBusy ? "disabled" : ""}>${uiIcon(saveBusy ? "refresh" : "save", 17)}</button>`;
  if (group.services.length === 1) return `<div class="section-actions">${saveAction}</div>`;
  const terminableCount = group.services.filter((service) => service.can_terminate).length;
  const stopDisabled = !terminableCount || stopBusy;
  return `<div class="section-actions">${saveAction}<button class="section-action icon-only-button stop-group-action" type="button" data-action="stop-service-group" data-group-id="${h(group.id)}" title="${stopBusy ? "Stopping services" : terminableCount ? "Stop all stoppable services" : "No services can be stopped safely"}" aria-label="${stopBusy ? "Stopping" : terminableCount ? "Stop all" : "No stoppable"} services in ${h(group.name)}" ${stopDisabled ? "disabled" : ""}>${uiIcon(stopBusy ? "refresh" : "stop", 15)}</button></div>`;
}

export function generatedTasksForGroup(services: ServiceSnapshot[]): LaunchTask[] {
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

export function canRestartService(service: ServiceSnapshot): boolean {
  return Boolean(
    service.can_terminate
    && service.process?.executable?.trim()
    && service.process.working_directory?.trim()
    && service.process.command.trim()
  );
}

export function renderServiceTile(service: ServiceSnapshot, ordinal: number | undefined, ordinalTotal: number | undefined, actionScope: ServiceTileActionScope | undefined, context: ServiceTileRenderingContext): string {
  const scope = actionScope ?? { select: service.relevance === "dev", info: true, stop: service.can_terminate, open: Boolean(service.browser_url) };
  const stopping = context.operations.has(`stop:${service.id}`);
  const restarting = context.operations.has(`restart:${service.id}`);
  const busy = stopping || restarting;
  const uptime = currentUptime(service);
  const pip = busy ? "busy" : uptime === null ? "idle" : service.status === "limited" ? "limited" : "running";
  const selected = Boolean(scope.select) && context.consoleTargetKind === "service" && context.selectedServiceId === service.id;
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
  const controlsMarkup = `${scope.info ? `<button type="button" class="info-button icon-only-button service-details-button service-card-control" data-tile-action data-action="service-details" data-service-id="${h(service.id)}" aria-label="View ${h(service.display_name)} details" title="View service details">${uiIcon("info", 15)}</button>` : ""}${scope.stop && canRestartService(service) ? `<button type="button" class="restart-action icon-only-button service-card-control" data-tile-action data-action="restart-service" data-service-id="${h(service.id)}" aria-label="${restarting ? "Restarting" : "Restart"} ${h(service.display_name)}" title="${restarting ? "Restarting process" : "Restart process"}" ${busy ? "disabled" : ""}>${restartIcon(15)}</button>` : ""}${scope.stop && service.can_terminate ? `<button type="button" class="stop-button service-card-control" data-tile-action data-action="stop-service" data-service-id="${h(service.id)}" aria-label="${stopping ? "Stopping" : "Stop"} ${h(service.display_name)}" title="${stopping ? "Stopping process" : "Stop process"}" ${busy ? "disabled" : ""}>${uiIcon("stop", 15)}</button>` : ""}`;
  const metricsMarkup = `<span class="metric metric-uptime${uptime !== null && uptime < FRESH_UPTIME_SECONDS ? " is-fresh" : ""}" data-metric="uptime" title="Uptime">${uiIcon("clock", 13)}<span class="sr-only">Uptime </span><span data-metric-text>${h(uptimeText(service, stopping, restarting))}</span></span><span class="metric metric-memory" data-metric="memory" title="Memory used">${uiIcon("memory", 13)}<span class="sr-only">Memory </span><span data-metric-text>${h(formatBytes(service.process?.memory_bytes ?? null))}</span></span>`;
  const trailingMarkup = scope.open ? renderOpenServiceButton(service, service.display_name) : "";
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

export function uptimeText(service: ServiceSnapshot, stopping: boolean, restarting = false): string {
  if (restarting) return "Restarting…";
  if (stopping) return "Stopping…";
  const uptime = currentUptime(service);
  return uptime === null ? "—" : formatUptimeCompact(uptime);
}

export function serviceConsoleOutputKind(service: ServiceSnapshot | null, context: ServicesConsoleRenderingContext): string {
  if (!service) return "empty";
  const state = context.serviceLogState.serviceId === service.id ? context.serviceLogState : null;
  if (state?.loading && !state.logs.trim()) return "loading";
  const log = state?.logs ?? "";
  const message = state?.error ?? state?.message ?? "";
  const unavailable = Boolean(state?.error || state?.available === false);
  if (log.trim()) return unavailable && message.trim() ? "log-alert" : "log";
  if (unavailable) return "unavailable";
  return "empty";
}

export function renderServiceConsole(context: ServicesRenderingContext): string {
  if (!context.console.open) return "";
  if (context.selection.consoleTarget?.kind === "container") return renderDockerConsole(context.console.docker, "service-console docker-service-console", "container");
  const service = context.selection.consoleTarget?.kind === "service"
    ? context.services.find((item) => item.id === context.selection.consoleTarget?.id) ?? null
    : null;
  const stateClass = service ? serviceStateClass(service) : "stopped";
  const title = service ? serviceTitle(service) || service.display_name : "Service console";
  const outputLabel = service ? `Logs for ${h(title)}` : "Service logs";
  return `<section class="launch-console service-console${service ? ` state-${stateClass}` : " launch-console-empty"}" aria-labelledby="service-console-title" data-console-service-id="${h(service?.id ?? "")}">
    ${context.console.renderConsoleResizer()}
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${stateClass}" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="service-console-title">${h(title)}</h2></div></div>
    </header>
    <div class="console-output-shell"><div class="console-output" data-console-output-kind="${serviceConsoleOutputKind(service, context.console)}" tabindex="0" role="log" aria-live="polite" aria-label="${outputLabel}">${renderServiceLogOutput(service, context.console)}</div>${context.console.renderConsoleJumpButton()}</div>
  </section>`;
}

export function serviceStateClass(service: ServiceSnapshot): "running" | "starting" | "stopped" {
  if (!service.process) return "stopped";
  return service.status === "limited" ? "starting" : "running";
}

export function renderServiceLogOutput(service: ServiceSnapshot | null, context: ServicesConsoleRenderingContext): string {
  if (!service) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("terminal", 18)}</span><strong>No service selected</strong><span>Select a service card to view its recent logs.</span></div>`;
  }
  const state = context.serviceLogState.serviceId === service.id ? context.serviceLogState : null;
  if (state?.loading && !state.logs.trim()) {
    const elapsed = context.serviceLogElapsedSeconds(state);
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("refresh", 18, "service-log-spinner")}</span><strong>Loading service logs · <span data-service-log-elapsed>${elapsed}s</span></strong><span>Fetching the most recent output from the service…</span></div>`;
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

type ServiceTileActionScope = { select?: boolean; info?: boolean; stop?: boolean; open?: boolean };

function serviceGroups(context: ServicesRenderingContext): ServiceBoardGroup[] {
  return serviceBoardGroups(context.services, context.containers);
}

function renderServiceGroupTiles(group: ServiceBoardGroup, context: ServicesRenderingContext): string {
  const total = group.services.length + group.containers.length;
  const ordinalTotal = total > 1 ? total : undefined;
  const tileContext: DockerTileRenderingContext = {
    activeTab: context.selection.container.tab,
    activeBottomPanel: context.selection.activeBottomPanel,
    selectedContainerId: context.selection.container.selectedId,
    servicesConsoleTargetKind: context.selection.consoleTarget?.kind ?? null,
    containerOperationBusy: context.containerOperationBusy
  };
  return `${group.services.map((service, index) => renderServiceTile(service, ordinalTotal ? index + 1 : undefined, ordinalTotal, undefined, context.tile)).join("")}${group.containers.map((container, index) => renderContainerTile(container, ordinalTotal ? group.services.length + index + 1 : undefined, ordinalTotal, true, tileContext)).join("")}`;
}
