import { uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import {
  FRESH_UPTIME_SECONDS,
  currentUptime,
  formatBytes
} from "./presentation";
import { uptimeText } from "./services-rendering";
import type {
  AppInfo,
  ContainerInfo,
  ContainerListing,
  LaunchProfile,
  ManagedTaskSnapshot,
  ServiceSnapshot,
  ThemeMode,
  WorkspaceSnapshot
} from "./types";

export type UiSupportContext = {
  elements: {
    workspace: HTMLElement;
  };
  getWorkspace: () => WorkspaceSnapshot | null;
  getContainerListing: () => ContainerListing | null;
  getProfiles: () => readonly LaunchProfile[];
  getSnapshots: () => readonly ManagedTaskSnapshot[];
  getAppInfo: () => AppInfo | null;
  getOperations: () => ReadonlySet<string>;
};

export function createUiSupport(context: UiSupportContext) {
  const snapshotFor = (profileId: string, taskName: string): ManagedTaskSnapshot | undefined =>
    context.getSnapshots().find((snapshot) => snapshot.profile_id === profileId && snapshot.task_name === taskName);

  const findService = (id: string): ServiceSnapshot => {
    const value = context.getWorkspace()?.services.find((service) => service.id === id);
    if (!value) throw new Error("The service is no longer available.");
    return value;
  };

  const findContainer = (id: string): ContainerInfo => {
    const value = context.getContainerListing()?.containers.find((container) => container.id === id);
    if (!value) throw new Error("The container is no longer available.");
    return value;
  };

  const findProfile = (id: string): LaunchProfile => {
    const value = context.getProfiles().find((profile) => profile.id === id);
    if (!value) throw new Error("The launch profile no longer exists.");
    return value;
  };

  const updateLiveMetrics = (): void => {
    const workspace = context.getWorkspace();
    if (!workspace) return;
    const services = new Map(workspace.services.map((service) => [service.id, service]));
    context.elements.workspace.querySelectorAll<HTMLElement>("[data-metrics-id]").forEach((tile) => {
      const service = services.get(tile.dataset.metricsId ?? "");
      if (!service) return;
      const uptime = currentUptime(service);
      setMetricText(tile, "uptime", uptimeText(service, context.getOperations().has(`stop:${service.id}`)));
      setMetricText(tile, "memory", formatBytes(service.process?.memory_bytes ?? null));
      tile.querySelector(".metric-uptime")?.classList.toggle("is-fresh", uptime !== null && uptime < FRESH_UPTIME_SECONDS);
    });
  };

  const renderHeaderCounts = (): void => {
    const workspace = context.getWorkspace();
    const services = workspace?.services.filter((service) => service.relevance === "dev").length ?? 0;
    const fallbackContainers = workspace?.services.filter((service) => service.relevance === "container").length ?? 0;
    byId("services-count").textContent = String(services);
    const listing = context.getContainerListing();
    byId("docker-count").textContent = String(listing?.available ? listing.containers.length : fallbackContainers);
    byId("launch-count").textContent = String(context.getProfiles().length);
  };

  const renderFooter = (): void => {
    const workspace = context.getWorkspace();
    if (!workspace) return;
    const status = workspace.errors[0] ?? (context.getAppInfo()?.demo ? "Demonstration mode" : "");
    byId("app-status").textContent = status;
    byId("status-footer").toggleAttribute("hidden", !status);
  };

  const toast = (message: string, error = false): void => {
    const host = byId("toast-root");
    host.innerHTML = `<div class="toast${error ? " is-error" : ""}">${h(message)}</div>`;
    window.setTimeout(() => host.replaceChildren(), 3500);
  };

  const showFatal = (error: unknown): void => {
    context.elements.workspace.innerHTML = `<div class="empty-state"><h2>Cutting Board could not start</h2><p>${h(messageOf(error))}</p></div>`;
  };

  return {
    snapshotFor,
    findService,
    findContainer,
    findProfile,
    updateLiveMetrics,
    renderHeaderCounts,
    renderFooter,
    toast,
    showFatal
  };
}

export function applyTheme(mode: ThemeMode): void {
  document.documentElement.dataset.theme = mode;
  document.documentElement.style.colorScheme = mode === "system" ? "dark light" : mode;
}

export function setMetricText(tile: HTMLElement, metric: string, value: string): void {
  const node = tile.querySelector<HTMLElement>(`[data-metric="${metric}"] [data-metric-text]`);
  if (node && node.textContent !== value) node.textContent = value;
}

export function loadingState(message: string): string {
  return `<div class="empty-state"><span class="spinner"></span><p>${h(message)}</p></div>`;
}

export function emptyState(title: string, message: string): string {
  return `<div class="empty-state"><h2>${h(title)}</h2><p>${h(message)}</p><button class="secondary-button icon-only-button" type="button" onclick="location.reload()" aria-label="Refresh" title="Refresh">${uiIcon("refresh", 16)}</button></div>`;
}

export function byId(id: string): HTMLElement {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing #${id}`);
  return value;
}

export function required(value: string | undefined): string {
  if (!value) throw new Error("Missing action identifier.");
  return value;
}

export function ellipsis(value: string, limit: number): string {
  return value.length <= limit ? value : `${value.slice(0, limit - 1)}…`;
}

export function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
