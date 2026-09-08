import type { ContainerInfo, ContainerListing, ContainerLogSnapshot } from "./types";
import type {
  ContainerTab,
  ContainerViewState,
  DockerLogState
} from "./docker-rendering";
import type { ServicesConsoleTarget } from "./services-rendering";

export type ContainerConsoleDataContext = {
  api: {
    containerLogs: (containerId: string) => Promise<ContainerLogSnapshot>;
  };
  getActiveTab: () => "services" | "docker" | "launch";
  getContainerListing: () => ContainerListing | null;
  getServicesConsoleTarget: () => ServicesConsoleTarget | null;
  setServicesConsoleTarget: (target: ServicesConsoleTarget | null) => void;
  captureServicesConsoleState: () => void;
  resetContainerSelection: (tab: ContainerTab, containerId: string | null) => void;
  invalidateServiceSelection: () => void;
  updateDockerConsoleDom: () => void;
  updateServiceConsoleDom: () => void;
  renderDocker: (force?: boolean) => void;
  renderServices: (force?: boolean) => void;
  messageOf: (error: unknown) => string;
};

function emptyDockerLogState(): DockerLogState {
  return { containerId: null, logs: "", loading: false, error: null };
}

export function createContainerConsoleData(context: ContainerConsoleDataContext) {
  const containerViewStates: Record<ContainerTab, ContainerViewState> = {
    services: {
      selectedContainerId: null,
      logState: emptyDockerLogState(),
      logRequestId: 0
    },
    docker: {
      selectedContainerId: null,
      logState: emptyDockerLogState(),
      logRequestId: 0
    }
  };

  function state(tab: ContainerTab): ContainerViewState {
    return containerViewStates[tab];
  }

  function activeContainerTab(): ContainerTab {
    return context.getActiveTab() === "services" ? "services" : "docker";
  }

  function findContainer(id: string): ContainerInfo {
    const container = context.getContainerListing()?.containers.find((item) => item.id === id);
    if (!container) throw new Error("The container is no longer available.");
    return container;
  }

  function syncSelectedContainer(): void {
    const listing = context.getContainerListing();
    if (!listing?.available) {
      clearDockerSelection();
      return;
    }
    for (const tab of ["services", "docker"] as const) {
      const selectedId = state(tab).selectedContainerId;
      if (selectedId && !listing.containers.some((container) => container.id === selectedId)) clearContainerSelection(tab);
    }
    const target = context.getServicesConsoleTarget();
    if (target?.kind === "container" && state("services").selectedContainerId !== target.id) {
      context.setServicesConsoleTarget(null);
    }
  }

  function clearContainerSelection(tab: ContainerTab): void {
    const viewState = state(tab);
    const clearedId = viewState.selectedContainerId;
    viewState.selectedContainerId = null;
    viewState.logRequestId += 1;
    viewState.logState = emptyDockerLogState();
    context.resetContainerSelection(tab, null);
    const target = context.getServicesConsoleTarget();
    if (tab === "services" && target?.kind === "container" && target.id === clearedId) context.setServicesConsoleTarget(null);
  }

  function clearDockerSelection(): void {
    clearContainerSelection("services");
    clearContainerSelection("docker");
  }

  async function refreshSelectedContainerLogs(): Promise<void> {
    const tab = activeContainerTab();
    const viewState = state(tab);
    if (tab === "services" && context.getServicesConsoleTarget()?.kind !== "container") return;
    if (!viewState.selectedContainerId || !context.getContainerListing()?.available) return;
    if (viewState.logState.containerId === viewState.selectedContainerId && viewState.logState.loading) return;
    await loadContainerLogs(viewState.selectedContainerId, false, tab);
  }

  function updateContainerConsoleDom(tab: ContainerTab): void {
    if (tab === "docker" && context.getActiveTab() === "docker") context.updateDockerConsoleDom();
    else if (tab === "services" && context.getActiveTab() === "services" && context.getServicesConsoleTarget()?.kind === "container") context.updateServiceConsoleDom();
  }

  async function loadContainerLogs(containerId: string, showLoading = false, tab = activeContainerTab()): Promise<void> {
    const viewState = state(tab);
    if (viewState.selectedContainerId !== containerId) return;
    if (!showLoading && viewState.logState.containerId === containerId && viewState.logState.loading) return;
    const requestId = ++viewState.logRequestId;
    if (showLoading) {
      viewState.logState = { containerId, logs: "", loading: true, error: null };
      updateContainerConsoleDom(tab);
    } else {
      viewState.logState = { ...viewState.logState, containerId, loading: true, error: null };
    }
    try {
      const result = await context.api.containerLogs(containerId);
      if (requestId !== viewState.logRequestId || viewState.selectedContainerId !== containerId) return;
      viewState.logState = { containerId, logs: result.logs ?? "", loading: false, error: null };
      updateContainerConsoleDom(tab);
    } catch (error) {
      if (requestId !== viewState.logRequestId || viewState.selectedContainerId !== containerId) return;
      viewState.logState = { ...viewState.logState, containerId, loading: false, error: context.messageOf(error) };
      updateContainerConsoleDom(tab);
    }
  }

  function selectContainer(id: string): void {
    findContainer(id);
    const tab = activeContainerTab();
    if (context.getActiveTab() === "services") context.captureServicesConsoleState();
    const viewState = state(tab);
    viewState.selectedContainerId = id;
    if (tab === "services") {
      context.setServicesConsoleTarget({ kind: "container", id });
      context.invalidateServiceSelection();
    }
    context.resetContainerSelection(tab, id);
    viewState.logState = { containerId: id, logs: "", loading: true, error: null };
    if (context.getActiveTab() === "services") context.renderServices(true);
    else context.renderDocker(true);
    void loadContainerLogs(id, true, tab);
  }

  return {
    state,
    activeContainerTab,
    syncSelectedContainer,
    clearDockerSelection,
    refreshSelectedContainerLogs,
    selectContainer
  };
}
