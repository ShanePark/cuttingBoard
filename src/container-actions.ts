import type { ContainerInfo, ContainerListing } from "./types";

export type ContainerActionsApi = {
  startContainer: (containerId: string) => Promise<{ success: boolean; message: string }>;
  stopContainer: (containerId: string) => Promise<{ success: boolean; message: string }>;
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
  toast: (message: string, error?: boolean) => void;
  messageOf: (error: unknown) => string;
};

export function createContainerActions(context: ContainerActionsContext) {
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

  return { findContainer, operateContainer };
}
