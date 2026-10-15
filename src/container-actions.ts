import { uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import { composeDownOperationKey, startComposeDownLogPolling, type ComposeDownLogScheduler } from "./docker-groups";
import type { openModal } from "./modal";
import type { ContainerInfo, ContainerListing } from "./types";

export type ContainerActionsApi = {
  startContainer: (containerId: string) => Promise<{ success: boolean; message: string }>;
  stopContainer: (containerId: string) => Promise<{ success: boolean; message: string }>;
  composeDown: (project: string) => Promise<{ success: boolean; message: string }>;
  composeDownLog: (project: string) => Promise<string>;
};

/** Shows a `docker compose down` run in the Docker console while it is in flight. */
export type ComposeDownConsole = {
  begin: (project: string) => void;
  updateLog: (project: string, log: string) => void;
  finish: (project: string, succeeded: boolean, error: string | null) => void;
};



export type ContainerActionsContext = {
  api: ContainerActionsApi;
  operations: Set<string>;
  getContainerListing: () => ContainerListing | null;
  getActiveTab: () => "services" | "docker" | "launch";
  refreshContainers: (force?: boolean) => Promise<void>;
  renderDocker: (force?: boolean) => void;
  renderServices: (force?: boolean) => void;
  updateDockerContainerStatuses: () => void;
  updateDockerConsoleDom: () => void;
  updateServiceConsoleDom: () => void;
  openModal: typeof openModal;
  closeModal: () => void;
  composeDownConsole: ComposeDownConsole;
  scheduler: ComposeDownLogScheduler;
  toast: (message: string, error?: boolean) => void;
  messageOf: (error: unknown) => string;
};

export function createContainerActions(context: ContainerActionsContext) {
  let pendingComposeDownProject: string | null = null;

  const findContainer = (id: string): ContainerInfo => {
    const container = context.getContainerListing()?.containers.find((item) => item.id === id);
    if (!container) throw new Error("The container is no longer available.");
    return container;
  };

  async function operateContainer(id: string, start: boolean): Promise<void> {
    findContainer(id);
    const key = `container:${id}`;
    if (context.operations.has(key)) return;
    context.operations.add(key);
    if (context.getActiveTab() === "docker") {
      context.updateDockerContainerStatuses();
      context.updateDockerConsoleDom();
    } else if (context.getActiveTab() === "services") {
      context.updateDockerContainerStatuses();
      context.updateServiceConsoleDom();
    }
    try {
      const result = start ? await context.api.startContainer(id) : await context.api.stopContainer(id);
      context.toast(result.message, !result.success);
    } catch (error) {
      context.toast(context.messageOf(error), true);
    } finally {
      try {
        await context.refreshContainers(true);
      } finally {
        context.operations.delete(key);
        if (context.getActiveTab() === "docker") context.renderDocker(true);
        else if (context.getActiveTab() === "services") context.renderServices(true);
      }
    }
  }

  function resetPending(): void {
    pendingComposeDownProject = null;
  }

  function requestComposeDown(project: string): void {
    if (pendingComposeDownProject !== null || context.operations.has(composeDownOperationKey(project))) return;
    const containers = context.getContainerListing()?.containers.filter((container) => container.compose_project === project) ?? [];
    if (!containers.some((container) => container.state === "running")) throw new Error(`Compose project ${project} has no running containers.`);
    pendingComposeDownProject = project;
    context.openModal("Stop compose project?", `<p class="confirm-copy">Run <strong>docker compose down</strong> for <strong>${h(project)}</strong>? This stops and removes its containers and networks; volumes and images are kept.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-compose-down-group" data-compose-project="${h(project)}">${uiIcon("stop", 13)} Stop All</button></div>`);
  }

  async function confirmComposeDown(project: string): Promise<void> {
    if (pendingComposeDownProject !== project) return;
    resetPending();
    context.closeModal();
    await composeDown(project);
  }

  async function composeDown(project: string): Promise<void> {
    const key = composeDownOperationKey(project);
    if (context.operations.has(key)) return;
    context.operations.add(key);
    let stopPolling: (() => Promise<void>) | null = null;
    let succeeded = false;
    let error: string | null = null;
    try {
      context.composeDownConsole.begin(project);
      if (context.getActiveTab() === "docker") context.renderDocker(true);
      stopPolling = startComposeDownLogPolling(
        () => context.api.composeDownLog(project),
        (log) => context.composeDownConsole.updateLog(project, log),
        context.scheduler
      );
      const result = await context.api.composeDown(project);
      succeeded = result.success;
      if (!result.success) error = result.message;
      context.toast(result.message, !result.success);
    } catch (reason) {
      error = context.messageOf(reason);
      context.toast(error, true);
    } finally {
      await stopPolling?.();
      context.composeDownConsole.finish(project, succeeded, error);
      if (context.getActiveTab() === "docker") context.renderDocker(true);
      try {
        await context.refreshContainers(true);
      } finally {
        context.operations.delete(key);
        if (context.getActiveTab() === "docker") context.renderDocker(true);
        else if (context.getActiveTab() === "services") context.renderServices(true);
      }
    }
  }

  return { findContainer, operateContainer, resetPending, requestComposeDown, confirmComposeDown };
}
