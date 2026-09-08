import { matchedServiceForTask } from "./presentation-services";
import {
  appendProgressLine,
  initialRestartProgress,
  progressFromTaskLog,
  remapRestartProgress,
  restartProgressBusyForService,
  shouldClearCompletedRestartProgress,
  type RestartProgressEvent,
  type RestartProgressScheduler,
  type ServiceRestartProgress
} from "./restart-progress";
import type { LaunchProfile, ServiceLogSnapshot, ServiceSnapshot, WorkspaceSnapshot } from "./types";
import type { ServiceLogState, ServicesConsoleTarget } from "./services-rendering";

export type ServiceConsoleSelection = {
  serviceId: string | null;
  consoleTarget: ServicesConsoleTarget | null;
};

export type ServiceConsoleDataContext = {
  api: {
    serviceLogs: (serviceId: string) => Promise<ServiceLogSnapshot>;
  };
  getActiveTab: () => "services" | "docker" | "launch";
  getWorkspace: () => WorkspaceSnapshot | null;
  getProfiles: () => readonly LaunchProfile[];
  getSelection: () => ServiceConsoleSelection;
  setSelection: (selection: ServiceConsoleSelection) => void;
  resetServiceSelection: (serviceId: string | null) => void;
  renderServices: (force?: boolean) => void;
  updateServiceConsoleDom: () => void;
  messageOf: (error: unknown) => string;
  operations: ReadonlySet<string>;
  workspaceElement: HTMLElement;
  scheduler: RestartProgressScheduler;
};

function emptyServiceLogState(): ServiceLogState {
  return {
    serviceId: null,
    logs: "",
    available: false,
    loading: false,
    loadingStartedAt: null,
    message: null,
    error: null
  };
}

export function createServiceConsoleData(context: ServiceConsoleDataContext) {
  let serviceLogState: ServiceLogState = emptyServiceLogState();
  let serviceRestartProgress: ServiceRestartProgress | null = null;
  let serviceLogRequestId = 0;
  const pendingServiceLogRequests = new Map<string, number>();
  let serviceLogElapsedTimer: number | null = null;

  function selectedServiceId(): string | null {
    return context.getSelection().serviceId;
  }

  function selectedConsoleTarget(): ServicesConsoleTarget | null {
    return context.getSelection().consoleTarget;
  }

  function currentService(): ServiceSnapshot | null {
    const id = selectedServiceId();
    if (!id) return null;
    return context.getWorkspace()?.services.find((service) => service.relevance === "dev" && service.id === id) ?? null;
  }

  function stopServiceLogElapsedTimer(): void {
    if (serviceLogElapsedTimer === null) return;
    context.scheduler.clearInterval(serviceLogElapsedTimer);
    serviceLogElapsedTimer = null;
  }

  function serviceLogElapsedSeconds(state: ServiceLogState = serviceLogState): number {
    if (state.loadingStartedAt === null) return 0;
    return Math.max(0, Math.floor((Date.now() - state.loadingStartedAt) / 1000));
  }

  function updateServiceLogElapsedDom(): void {
    const serviceId = selectedServiceId();
    if (context.getActiveTab() !== "services" || selectedConsoleTarget()?.kind !== "service" || !serviceId || !serviceLogState.loading || serviceLogState.serviceId !== serviceId) return;
    const consoleElement = [...context.workspaceElement.querySelectorAll<HTMLElement>(".service-console:not(.docker-service-console)")]
      .find((element) => element.dataset.consoleServiceId === serviceId);
    if (!consoleElement) return;
    const elapsed = `${serviceLogElapsedSeconds()}s`;
    consoleElement.querySelectorAll<HTMLElement>("[data-service-log-elapsed]").forEach((element) => {
      if (element.textContent !== elapsed) element.textContent = elapsed;
    });
  }

  function startServiceLogElapsedTimer(serviceId: string, requestId: number): void {
    stopServiceLogElapsedTimer();
    serviceLogElapsedTimer = context.scheduler.setInterval(() => {
      if (requestId !== serviceLogRequestId || selectedConsoleTarget()?.kind !== "service" || selectedServiceId() !== serviceId || serviceLogState.serviceId !== serviceId || !serviceLogState.loading || pendingServiceLogRequests.get(serviceId) !== requestId) {
        stopServiceLogElapsedTimer();
        return;
      }
      if (context.getActiveTab() === "services") updateServiceLogElapsedDom();
    }, 1000);
  }

  function resumeServiceLogElapsedTimer(): void {
    const serviceId = selectedServiceId();
    if (context.getActiveTab() !== "services" || selectedConsoleTarget()?.kind !== "service" || !serviceId || !serviceLogState.loading || pendingServiceLogRequests.get(serviceId) !== serviceLogRequestId) return;
    startServiceLogElapsedTimer(serviceId, serviceLogRequestId);
  }

  function updateServiceConsoleDomIfVisible(): void {
    if (context.getActiveTab() === "services" && selectedConsoleTarget()?.kind === "service") context.updateServiceConsoleDom();
  }

  function remapServiceSelectionForRestart(): void {
    const progress = serviceRestartProgress;
    const workspace = context.getWorkspace();
    if (!progress || !workspace) return;
    const profile = context.getProfiles().find((item) => item.id === progress.profileId);
    const task = profile?.tasks.find((item) => item.name === progress.taskName);
    if (!profile || !task) return;
    const replacement = matchedServiceForTask(profile, task, workspace.services);
    if (!replacement || replacement.id === progress.serviceId) return;
    const previousServiceId = progress.serviceId;
    serviceRestartProgress = remapRestartProgress(progress, replacement.id);
    const selection = context.getSelection();
    context.setSelection({
      serviceId: selection.serviceId === previousServiceId ? replacement.id : selection.serviceId,
      consoleTarget: selection.consoleTarget?.kind === "service" && selection.consoleTarget.id === previousServiceId
        ? { kind: "service", id: replacement.id }
        : selection.consoleTarget
    });
    serviceLogRequestId += 1;
    serviceLogState = {
      serviceId: replacement.id,
      logs: "",
      available: false,
      loading: false,
      loadingStartedAt: null,
      message: null,
      error: null
    };
    if (serviceRestartProgress.phase === "completed") serviceRestartProgress = null;
  }

  function syncSelectedService(): void {
    const serviceId = selectedServiceId();
    if (!serviceId) return;
    const stillAvailable = Boolean(context.getWorkspace()?.services.some((service) => service.relevance === "dev" && service.id === serviceId));
    if (!stillAvailable && serviceRestartProgress?.serviceId === serviceId) return;
    if (!stillAvailable) clearServiceSelection();
  }

  function clearServiceSelection(): void {
    stopServiceLogElapsedTimer();
    const selection = context.getSelection();
    context.setSelection({
      serviceId: null,
      consoleTarget: selection.consoleTarget?.kind === "service" ? null : selection.consoleTarget
    });
    serviceRestartProgress = null;
    serviceLogRequestId += 1;
    serviceLogState = emptyServiceLogState();
    context.resetServiceSelection(null);
  }

  async function refreshSelectedServiceLogs(): Promise<void> {
    const serviceId = selectedServiceId();
    if (selectedConsoleTarget()?.kind !== "service" || !serviceId) return;
    const service = currentService();
    if (!service) {
      if (serviceRestartProgress?.serviceId === serviceId) return;
      clearServiceSelection();
      return;
    }
    if (serviceLogState.serviceId === serviceId && serviceLogState.loading) return;
    await loadServiceLogs(serviceId);
  }

  async function loadServiceLogs(serviceId: string, showLoading = false): Promise<void> {
    if (selectedConsoleTarget()?.kind !== "service" || selectedServiceId() !== serviceId) return;
    if (!showLoading && serviceLogState.serviceId === serviceId && serviceLogState.loading) return;
    if (!showLoading && pendingServiceLogRequests.has(serviceId)) return;
    const requestId = ++serviceLogRequestId;
    if (showLoading) {
      serviceLogState = {
        serviceId,
        logs: "",
        available: false,
        loading: true,
        loadingStartedAt: Date.now(),
        message: null,
        error: null
      };
      updateServiceConsoleDomIfVisible();
    } else {
      serviceLogState = {
        ...serviceLogState,
        serviceId,
        loading: true,
        loadingStartedAt: Date.now(),
        error: null
      };
    }
    pendingServiceLogRequests.set(serviceId, requestId);
    startServiceLogElapsedTimer(serviceId, requestId);
    try {
      const result = await context.api.serviceLogs(serviceId);
      if (requestId !== serviceLogRequestId || selectedConsoleTarget()?.kind !== "service" || selectedServiceId() !== serviceId) return;
      stopServiceLogElapsedTimer();
      serviceLogState = {
        serviceId,
        logs: result.logs ?? "",
        available: result.available ?? Boolean(result.source_path || result.logs),
        loading: false,
        loadingStartedAt: null,
        message: result.message ?? null,
        error: null
      };
      updateServiceConsoleDomIfVisible();
    } catch (error) {
      if (requestId !== serviceLogRequestId || selectedConsoleTarget()?.kind !== "service" || selectedServiceId() !== serviceId) return;
      stopServiceLogElapsedTimer();
      serviceLogState = {
        ...serviceLogState,
        serviceId,
        loading: false,
        loadingStartedAt: null,
        error: context.messageOf(error)
      };
      updateServiceConsoleDomIfVisible();
    } finally {
      if (pendingServiceLogRequests.get(serviceId) === requestId) pendingServiceLogRequests.delete(serviceId);
    }
  }

  function selectService(id: string): void {
    const service = context.getWorkspace()?.services.find((item) => item.id === id);
    if (!service) throw new Error("The service is no longer available.");
    if (service.relevance !== "dev") throw new Error("This service is not available in the Services view.");
    if (serviceRestartProgress && serviceRestartProgress.serviceId !== id) serviceRestartProgress = null;
    stopServiceLogElapsedTimer();
    context.setSelection({ serviceId: id, consoleTarget: { kind: "service", id } });
    context.resetServiceSelection(id);
    serviceLogRequestId += 1;
    serviceLogState = {
      serviceId: id,
      logs: "",
      available: false,
      loading: true,
      loadingStartedAt: Date.now(),
      message: null,
      error: null
    };
    context.renderServices(true);
    void loadServiceLogs(id, true);
  }

  function beginServiceRestartProgress(serviceId: string, profileId: string, taskName: string): void {
    serviceRestartProgress = initialRestartProgress(serviceId, profileId, taskName);
    if (context.getActiveTab() === "services") context.renderServices(true);
  }

  function updateServiceRestartProgress(profileId: string, taskName: string, logTail: string): void {
    if (!serviceRestartProgress || serviceRestartProgress.profileId !== profileId || serviceRestartProgress.taskName !== taskName) return;
    serviceRestartProgress = progressFromTaskLog(serviceRestartProgress, logTail);
    const selection = context.getSelection();
    if (context.getActiveTab() === "services" && selection.consoleTarget?.kind === "service" && selection.serviceId === serviceRestartProgress.serviceId) {
      context.updateServiceConsoleDom();
    }
  }

  function finishServiceRestartProgress(profileId: string, taskName: string, succeeded: boolean): void {
    if (!serviceRestartProgress || serviceRestartProgress.profileId !== profileId || serviceRestartProgress.taskName !== taskName) return;
    if (succeeded) {
      if (serviceRestartProgress.phase !== "completed") {
        const completion: RestartProgressEvent = {
          profile_id: serviceRestartProgress.profileId,
          task_name: serviceRestartProgress.taskName,
          phase: "completed",
          message: "Restart completed."
        };
        serviceRestartProgress = {
          ...serviceRestartProgress,
          phase: completion.phase,
          message: completion.message,
          detail: null,
          logTail: appendProgressLine(serviceRestartProgress.logTail, completion)
        };
      }
      if (serviceRestartProgress && shouldClearCompletedRestartProgress(serviceRestartProgress)) serviceRestartProgress = null;
    } else if (serviceRestartProgress.phase !== "failed") {
      const failure: RestartProgressEvent = {
        profile_id: serviceRestartProgress.profileId,
        task_name: serviceRestartProgress.taskName,
        phase: "failed",
        message: "Restart preparation failed.",
        detail: "See the task log for the failure details."
      };
      serviceRestartProgress = {
        ...serviceRestartProgress,
        phase: failure.phase,
        message: failure.message,
        detail: failure.detail ?? null,
        logTail: appendProgressLine(serviceRestartProgress.logTail, failure)
      };
    }
    if (context.getActiveTab() === "services") context.renderServices(true);
  }

  function serviceRestartInProgress(serviceId: string): boolean {
    return restartProgressBusyForService(
      serviceRestartProgress,
      serviceId,
      context.operations.has(`restart:${serviceId}`)
    );
  }

  function invalidateForContainerSelection(): void {
    stopServiceLogElapsedTimer();
    serviceLogRequestId += 1;
  }

  return {
    selectedServiceId,
    selectedConsoleTarget,
    currentService,
    logState: () => serviceLogState,
    restartProgress: () => serviceRestartProgress,
    serviceLogElapsedSeconds,
    selectService,
    syncSelectedService,
    remapServiceSelectionForRestart,
    clearServiceSelection,
    refreshSelectedServiceLogs,
    resumeServiceLogElapsedTimer,
    stopServiceLogElapsedTimer,
    beginServiceRestartProgress,
    updateServiceRestartProgress,
    finishServiceRestartProgress,
    serviceRestartInProgress,
    invalidateForContainerSelection
  };
}
