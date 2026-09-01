import { techIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import { imageTech, uniquePorts } from "./presentation";
import { renderGroupCount, renderTileFoot, renderTileHeading, renderTileOrdinal } from "./tile-rendering";
import type { ContainerInfo, ContainerListing, ServiceSnapshot } from "./types";

export type DockerLogState = {
  containerId: string | null;
  logs: string;
  loading: boolean;
  error: string | null;
};

export type ContainerTab = "services" | "docker";

export type ContainerViewState = {
  selectedContainerId: string | null;
  logState: DockerLogState;
  logRequestId: number;
};

export type DockerTileRenderingContext = {
  activeTab: ContainerTab;
  activeBottomPanel: string | null;
  selectedContainerId: string | null;
  servicesConsoleTargetKind: "service" | "container" | null;
  containerOperationBusy: (id: string) => boolean;
};

export type DockerConsoleRenderingContext = {
  consoleOpen: boolean;
  container: ContainerInfo | null;
  logState: DockerLogState;
  containerOperationBusy: (id: string) => boolean;
  renderConsoleResizer: () => string;
  renderConsoleJumpButton: () => string;
};

export type DockerRenderingContext = {
  containerListing: ContainerListing | null;
  fallbackServices: readonly ServiceSnapshot[];
  tile: DockerTileRenderingContext;
  console: DockerConsoleRenderingContext;
  renderFallbackServiceTile: (service: ServiceSnapshot, ordinal: number, ordinalTotal: number) => string;
  renderGroupTitle: (title: string, itemCount: number, action: string, attributes: string, displayTitle?: string, actionTitle?: string) => string;
  loadingState: (message: string) => string;
  emptyState: (title: string, message: string) => string;
};

export function dockerRenderSignature(context: DockerRenderingContext): string {
  const containerStructure = context.containerListing
    ? {
        available: context.containerListing.available,
        message: context.containerListing.message,
        containers: context.containerListing.containers.map((container) => ({
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
  const containerActions = context.containerListing?.containers.map((container) => [container.id, context.tile.containerOperationBusy(container.id)]) ?? [];
  return JSON.stringify([
    containerStructure,
    containerActions,
    context.tile.selectedContainerId,
    context.fallbackServices.map((service) => [service.id, uniquePorts(service)]),
    context.tile.activeBottomPanel
  ]);
}

export function renderDockerView(context: DockerRenderingContext): string {
  const fallback = context.fallbackServices;
  const consoleMarkup = renderDockerConsole(context.console);
  if (!context.containerListing) {
    return `<div class="docker-view split-view"><div class="split-view-list">${context.loadingState("Reading Docker containers")}</div>${consoleMarkup}</div>`;
  }
  if (!context.containerListing.available) {
    if (fallback.length) {
      return `<div class="docker-view split-view"><div class="split-view-list"><div class="inline-notice"><strong>Docker could not be queried.</strong><span>${h(context.containerListing.message ?? "Docker is unavailable.")}</span></div><div class="board"><section class="service-section" data-tiles="${fallback.length}" aria-labelledby="container-listeners-title"><header class="section-header"><span class="section-accent accent-container"></span><h2 id="container-listeners-title"><span class="group-title-text">CONTAINER LISTENERS</span>${renderGroupCount(fallback.length)}</h2></header><div class="tile-grid">${fallback.map((service, index) => context.renderFallbackServiceTile(service, index + 1, fallback.length)).join("")}</div></section></div></div>${consoleMarkup}</div>`;
    }
    return `<div class="docker-view split-view"><div class="split-view-list">${context.emptyState("Docker is unavailable", context.containerListing.message ?? "The Docker CLI could not be queried.")}</div>${consoleMarkup}</div>`;
  }
  if (context.containerListing.containers.length === 0) {
    return `<div class="docker-view split-view"><div class="split-view-list">${context.emptyState("No containers found", context.containerListing.message ?? "Docker returned an empty list.")}</div>${consoleMarkup}</div>`;
  }
  const groups = groupContainers(context.containerListing.containers);
  return `<div class="docker-view split-view"><div class="split-view-list"><div class="board">${groups.map((group) => `
    <section class="service-section" data-tiles="${group.containers.length}" aria-labelledby="container-group-${h(encodeURIComponent(group.name))}">
      <header class="section-header"><span class="section-accent accent-container"></span><h2 id="container-group-${h(encodeURIComponent(group.name))}">${context.renderGroupTitle(group.name, group.containers.length, "container-group-details", `data-group-name="${h(group.name)}"`, group.name.toUpperCase())}${renderGroupCount(group.containers.length)}</h2></header>
      <div class="tile-grid">${group.containers.map((container, index) => renderContainerTile(container, index + 1, group.containers.length, true, context.tile)).join("")}</div>
    </section>`).join("")}</div></div>${consoleMarkup}</div>`;
}

export function groupContainers(containers: ContainerInfo[]): Array<{ name: string; containers: ContainerInfo[] }> {
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

export function renderContainerTile(container: ContainerInfo, ordinal: number | undefined, ordinalTotal: number | undefined, showActions: boolean, context: DockerTileRenderingContext): string {
  const running = container.state === "running";
  const busy = showActions && context.containerOperationBusy(container.id);
  const selectedId = showActions ? context.selectedContainerId : null;
  const selected = showActions && selectedId === container.id && (context.activeTab !== "services" || context.servicesConsoleTargetKind === "container");
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
        ${renderTileHeading(container.name)}
        ${showActions ? `<div class="service-card-actions">${detailsButton}${actionButton}</div>` : actionButton}
      </div>
      <div class="tile-metrics">
        <span class="metric metric-state ${busy ? "is-busy" : running ? "is-running" : "is-stopped"}" title="Container state">${uiIcon("docker", 13)}<span class="sr-only">State </span><span data-container-status-text>${h(ellipsis(stateText, 30))}</span></span>
      </div>
      ${renderTileFoot(container.ports, "No published ports", "")}
    </article>`;
}

export function dockerConsoleOutputKind(container: ContainerInfo | null, logState: DockerLogState): string {
  if (!container) return "empty";
  const log = logState.containerId === container.id ? logState.logs : "";
  if (logState.loading && !log.trim()) return "loading";
  if (logState.error) return log.trim() ? "log-alert" : "error";
  return log.trim() ? "log" : "empty";
}

export function renderDockerConsole(context: DockerConsoleRenderingContext, consoleClass = "docker-console", consoleKind = "container"): string {
  if (!context.consoleOpen) return "";
  const container = context.container;
  const stateClass = container ? containerStateClass(container) : "idle";
  const busy = container ? context.containerOperationBusy(container.id) : false;
  const statusText = container
    ? busy ? `${container.state === "running" ? "Stopping" : "Starting"}…` : containerStateText(container)
    : "No container selected";
  const title = container?.name ?? "Container console";
  const outputLabel = container ? `Logs for ${h(container.name)}` : "Docker container logs";
  return `<section class="launch-console ${consoleClass}${container ? ` state-${stateClass}` : " launch-console-empty"}" aria-labelledby="docker-console-title" data-console-kind="${h(consoleKind)}" data-console-container-id="${h(container?.id ?? "")}">
    ${context.renderConsoleResizer()}
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${stateClass}" aria-hidden="true">${uiIcon("docker", 16)}</span><div><h2 id="docker-console-title">${h(title)}</h2></div></div>
      <div class="console-meta" aria-label="Container status"><span class="console-state state-${stateClass}"><span class="task-state-dot" aria-hidden="true"></span><span data-console-status-text>${h(statusText)}</span></span></div>
    </header>
    <div class="console-output-shell"><div class="console-output docker-console-output" data-console-output-kind="${dockerConsoleOutputKind(container, context.logState)}" tabindex="0" role="log" aria-live="polite" aria-label="${outputLabel}">${renderDockerLogOutput(container, context.logState)}</div>${context.renderConsoleJumpButton()}</div>
  </section>`;
}

export function renderDockerLogOutput(container: ContainerInfo | null, logState: DockerLogState): string {
  if (!container) {
    return `<div class="console-message"><span class="console-message-icon">${uiIcon("docker", 18)}</span><strong>No container selected</strong><span>Select a Docker container card to view its recent logs.</span></div>`;
  }
  const log = logState.containerId === container.id ? logState.logs : "";
  if (logState.loading && !log.trim()) {
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

export function containerStateClass(container: ContainerInfo): "running" | "starting" | "stopped" {
  const state = container.state.trim().toLowerCase();
  if (state === "running") return "running";
  if (["created", "restarting", "paused"].includes(state)) return "starting";
  return "stopped";
}

export function containerStateText(container: ContainerInfo): string {
  return container.status.trim() || (container.state.trim() ? container.state : "Unknown");
}

function ellipsis(value: string, limit: number): string {
  return value.length <= limit ? value : `${value.slice(0, limit - 1)}…`;
}
