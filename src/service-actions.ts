import { restartIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import { openModal } from "./modal";
import { canRestartService, generatedTasksForGroup } from "./services-rendering";
import {
  launchTasksEquivalent,
  serviceBoardGroups,
  serviceTitle,
  type ServiceBoardGroup
} from "./presentation";
import type {
  ContainerInfo,
  LaunchProfile,
  LaunchTask,
  ServiceSnapshot,
  WorkspaceSnapshot
} from "./types";

export type ServiceActionsApi = {
  terminate: (serviceId: string) => Promise<{ success: boolean; message: string }>;
  restartService: (serviceId: string) => Promise<void>;
  saveProfile: (profile: LaunchProfile) => Promise<LaunchProfile[]>;
};

export type ServiceActionsContext = {
  api: ServiceActionsApi;
  operations: Set<string>;
  getWorkspace: () => WorkspaceSnapshot | null;
  getContainers: () => readonly ContainerInfo[];
  getProfiles: () => readonly LaunchProfile[];
  setProfiles: (profiles: LaunchProfile[]) => void;
  refreshWorkspace: (force?: boolean) => Promise<void>;
  renderServices: (force?: boolean) => void;
  renderHeaderCounts: () => void;
  openUrl: (url: string) => Promise<void>;
  openModal: typeof openModal;
  closeModal: () => void;
  toast: (message: string, error?: boolean) => void;
};

export function createServiceActions(context: ServiceActionsContext) {
  let pendingServiceStopId: string | null = null;
  let pendingServiceRestartId: string | null = null;
  let pendingGroupSaveId: string | null = null;
  let pendingGroupStopId: string | null = null;

  const findService = (id: string): ServiceSnapshot => {
    const service = context.getWorkspace()?.services.find((item) => item.id === id);
    if (!service) throw new Error("The service is no longer available.");
    return service;
  };

  function groupForId(id: string): ServiceBoardGroup {
    // The same grouping the board renders, so a saved profile holds exactly the cards on screen.
    const group = serviceBoardGroups(
      context.getWorkspace()?.services.filter((service) => service.relevance === "dev") ?? [],
      context.getContainers()
    ).find((item) => item.id === id);
    if (!group) throw new Error("The workspace group is no longer available.");
    return group;
  }

  function profileForGroup(group: ServiceBoardGroup): LaunchProfile | undefined {
    if (!group.path) return undefined;
    return context.getProfiles().find((profile) => profile.project_root === group.path && launchTasksEquivalent(profile.tasks, generatedTasksForGroup(group)));
  }

  function validateGroupProfile(group: ServiceBoardGroup): LaunchTask[] {
    if (!group.path) throw new Error("This workspace group has no project root and cannot be saved.");
    const incomplete = group.services.filter((service) => !service.process?.command.trim() || !service.process?.working_directory?.trim());
    if (incomplete.length) throw new Error(`Cannot save ${group.name}: ${incomplete.map((service) => serviceTitle(service) || service.display_name).join(", ")} ${incomplete.length === 1 ? "is missing" : "are missing"} a process command or working directory.`);
    return generatedTasksForGroup(group);
  }

  function resetPending(): void {
    pendingServiceStopId = null;
    pendingServiceRestartId = null;
    pendingGroupSaveId = null;
    pendingGroupStopId = null;
  }

  async function openService(id: string): Promise<void> {
    const service = findService(id);
    if (!service.browser_url) throw new Error("This service has no browser destination.");
    await context.openUrl(service.browser_url);
  }

  function requestStopService(id: string): void {
    if (pendingServiceStopId !== null || pendingServiceRestartId !== null || context.operations.has(`stop:${id}`) || context.operations.has(`restart:${id}`)) return;
    const service = findService(id);
    if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
    pendingServiceStopId = id;
    context.openModal("Stop service?", `<p class="confirm-copy">Stop <strong>${h(service.display_name)}</strong>? This will terminate process PID <span class="mono">${service.process?.pid ?? "unknown"}</span>.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service" data-service-id="${h(id)}">${uiIcon("stop", 13)} Stop</button></div>`);
  }

  async function confirmStopService(id: string): Promise<void> {
    if (pendingServiceStopId !== id) return;
    resetPending();
    context.closeModal();
    await stopService(id);
  }

  async function stopService(id: string): Promise<void> {
    const service = findService(id);
    if (!service.can_terminate) throw new Error("This process cannot be stopped safely.");
    const key = `stop:${id}`;
    if (context.operations.has(key) || context.operations.has(`restart:${id}`)) return;
    context.operations.add(key);
    context.renderServices(true);
    try {
      const result = await context.api.terminate(id);
      context.toast(result.message, !result.success);
      await context.refreshWorkspace(true);
    } finally {
      context.operations.delete(key);
      context.renderServices(true);
    }
  }

  function requestRestartService(id: string): void {
    if (pendingServiceStopId !== null || pendingServiceRestartId !== null || context.operations.has(`stop:${id}`) || context.operations.has(`restart:${id}`)) return;
    const service = findService(id);
    if (!canRestartService(service)) throw new Error("This process does not expose enough launch information to restart safely.");
    pendingServiceRestartId = id;
    context.openModal("Restart service?", `<p class="confirm-copy">Restart <strong>${h(service.display_name)}</strong>? Process PID <span class="mono">${service.process?.pid ?? "unknown"}</span> will be stopped and started again with its current launch settings.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button icon-button-label" type="button" data-action="confirm-restart-service" data-service-id="${h(id)}">${restartIcon(13)} Restart</button></div>`);
  }

  async function confirmRestartService(id: string): Promise<void> {
    if (pendingServiceRestartId !== id) return;
    resetPending();
    context.closeModal();
    await restartService(id);
  }

  async function restartService(id: string): Promise<void> {
    const service = findService(id);
    if (!canRestartService(service)) throw new Error("This process does not expose enough launch information to restart safely.");
    const key = `restart:${id}`;
    if (context.operations.has(key) || context.operations.has(`stop:${id}`)) return;
    context.operations.add(key);
    context.renderServices(true);
    try {
      await context.api.restartService(id);
      context.toast(`Restarted ${service.display_name}.`);
    } finally {
      try {
        await context.refreshWorkspace(true);
      } finally {
        context.operations.delete(key);
        context.renderServices(true);
      }
    }
  }

  function requestSaveGroup(id: string): void {
    if (pendingGroupSaveId !== null || context.operations.has(`group-save:${id}`)) return;
    const group = groupForId(id);
    const tasks = validateGroupProfile(group);
    if (profileForGroup(group)) return;
    pendingGroupSaveId = id;
    context.openModal("Save launch profile?", `<p class="confirm-copy">Save <strong>${h(group.name)}</strong> as a launch profile with ${tasks.length} ${tasks.length === 1 ? "task" : "tasks"}?</p><p class="form-note mono">${h(group.path)}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button icon-button-label" type="button" data-action="confirm-save-service-group" data-group-id="${h(id)}">${uiIcon("save", 15)} Save Profile</button></div>`);
  }

  async function confirmSaveGroup(id: string): Promise<void> {
    if (pendingGroupSaveId !== id) return;
    resetPending();
    context.closeModal();
    await saveGroup(id);
  }

  async function saveGroup(id: string): Promise<void> {
    const group = groupForId(id);
    const key = `group-save:${id}`;
    if (context.operations.has(key)) return;
    const tasks = validateGroupProfile(group);
    if (profileForGroup(group)) return;
    context.operations.add(key);
    context.renderServices(true);
    try {
      context.setProfiles(await context.api.saveProfile({
        id: crypto.randomUUID().replaceAll("-", ""),
        name: group.name,
        project_root: group.path!,
        tasks
      }));
      context.renderHeaderCounts();
      context.toast(`Launch profile saved for ${group.name}.`);
    } finally {
      context.operations.delete(key);
      context.renderServices(true);
    }
  }

  function requestStopGroup(id: string): void {
    if (pendingGroupStopId !== null || context.operations.has(`group-stop:${id}`)) return;
    const group = groupForId(id);
    const candidates = group.services.filter((service) => service.can_terminate);
    if (!candidates.length) throw new Error("No services in this group can be stopped safely.");
    pendingGroupStopId = id;
    context.openModal("Stop all services?", `<p class="confirm-copy">Stop <strong>${candidates.length} stoppable ${candidates.length === 1 ? "service" : "services"}</strong> in ${h(group.name)}? Services that cannot be terminated safely will remain untouched.</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-stop-service-group" data-group-id="${h(id)}">${uiIcon("stop", 13)} Stop All</button></div>`);
  }

  async function confirmStopGroup(id: string): Promise<void> {
    if (pendingGroupStopId !== id) return;
    resetPending();
    context.closeModal();
    await stopGroup(id);
  }

  async function stopGroup(id: string): Promise<void> {
    const group = groupForId(id);
    const candidates = group.services.filter((service) => service.can_terminate);
    if (!candidates.length) throw new Error("No services in this group can be stopped safely.");
    const key = `group-stop:${id}`;
    if (context.operations.has(key)) return;
    const pending = candidates.filter((service) => !context.operations.has(`stop:${service.id}`));
    if (!pending.length) {
      context.toast(`All stoppable services in ${group.name} are already stopping.`);
      return;
    }
    pending.forEach((service) => context.operations.add(`stop:${service.id}`));
    context.operations.add(key);
    context.renderServices(true);
    try {
      const results = await Promise.allSettled(pending.map((service) => context.api.terminate(service.id)));
      const successes = results.filter((result) => result.status === "fulfilled" && result.value.success).length;
      const failures = results.length - successes;
      context.toast(`${successes} of ${results.length} services in ${group.name} stopped${failures ? `; ${failures} could not be stopped.` : "."}`, failures > 0);
      await context.refreshWorkspace(true);
    } finally {
      pending.forEach((service) => context.operations.delete(`stop:${service.id}`));
      context.operations.delete(key);
      context.renderServices(true);
    }
  }

  return {
    findService,
    groupForId,
    profileForGroup,
    validateGroupProfile,
    openService,
    requestStopService,
    confirmStopService,
    stopService,
    requestRestartService,
    confirmRestartService,
    restartService,
    requestSaveGroup,
    confirmSaveGroup,
    saveGroup,
    requestStopGroup,
    confirmStopGroup,
    stopGroup,
    resetPending
  };
}
