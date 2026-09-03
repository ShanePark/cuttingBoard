import { restartIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import { launchProfileIsActive, launchTaskCanStart, launchTaskCanStop } from "./launch-state";
import { openModal } from "./modal";
import { startRestartProgressPolling } from "./restart-progress";
import type {
  LaunchProfile,
  ManagedTaskSnapshot
} from "./types";

export type PendingLaunchAction =
  | { kind: "task"; direction: "start" | "stop" | "restart"; profileId: string; taskName: string }
  | { kind: "profile"; direction: "start" | "stop"; profileId: string };

export type LaunchActionsApi = {
  saveProfile: (profile: LaunchProfile) => Promise<LaunchProfile[]>;
  deleteProfile: (profileId: string) => Promise<LaunchProfile[]>;
  startTask: (profileId: string, taskName: string) => Promise<ManagedTaskSnapshot>;
  stopTask: (profileId: string, taskName: string) => Promise<ManagedTaskSnapshot>;
  restartTask: (profileId: string, taskName: string) => Promise<ManagedTaskSnapshot>;
  taskLogTail?: (profileId: string, taskName: string) => Promise<string>;
  stopProfile: (profileId: string) => Promise<ManagedTaskSnapshot[]>;
};

export type LaunchActionsContext = {
  api: LaunchActionsApi;
  operations: Set<string>;
  getProfiles: () => readonly LaunchProfile[];
  setProfiles: (profiles: LaunchProfile[]) => void;
  getSnapshots: () => readonly ManagedTaskSnapshot[];
  setSnapshots: (snapshots: ManagedTaskSnapshot[]) => void;
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined;
  setSelectedTask: (profileId: string, taskName: string) => void;
  focusTaskConsole?: (profileId: string, taskName: string) => void;
  updateTaskLogTail?: (profileId: string, taskName: string, logTail: string) => void;
  renderLaunch: (force?: boolean) => void;
  refreshLaunch: (force?: boolean) => Promise<void>;
  renderHeaderCounts: () => void;
  hasProfileForm: () => boolean;
  readProfileForm: (id: string | null) => LaunchProfile | null;
  openModal: typeof openModal;
  closeModal: () => void;
  toast: (message: string, error?: boolean) => void;
};

export function launchProfileOperationKey(profileId: string): string {
  return `profile:${profileId}`;
}

export function launchProfileHasTaskOperation(profile: LaunchProfile, operations: ReadonlySet<string>): boolean {
  return profile.tasks.some((task) => operations.has(`task:${profile.id}:${task.name}`));
}

export function launchProfileBlocksEditing(
  profile: LaunchProfile,
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined
): boolean {
  return launchProfileIsActive(profile.tasks.map((task) => snapshotFor(profile.id, task.name)?.state ?? "stopped"));
}

export function mergeSnapshots(current: readonly ManagedTaskSnapshot[], next: readonly ManagedTaskSnapshot[]): ManagedTaskSnapshot[] {
  const map = new Map(current.map((snapshot) => [`${snapshot.profile_id}\0${snapshot.task_name}`, snapshot]));
  for (const snapshot of next) map.set(`${snapshot.profile_id}\0${snapshot.task_name}`, snapshot);
  return [...map.values()];
}

export function createLaunchActions(context: LaunchActionsContext) {
  let pendingLaunchAction: PendingLaunchAction | null = null;
  let pendingDeleteProfileId: string | null = null;

  const findProfile = (id: string): LaunchProfile => {
    const profile = context.getProfiles().find((item) => item.id === id);
    if (!profile) throw new Error("The launch profile no longer exists.");
    return profile;
  };

  const hasTaskOperation = (profile: LaunchProfile): boolean => launchProfileHasTaskOperation(profile, context.operations);
  const blocksEditing = (profile: LaunchProfile): boolean => launchProfileBlocksEditing(profile, context.snapshotFor);
  const focusTask = (profileId: string, taskName: string): void => {
    if (context.focusTaskConsole) context.focusTaskConsole(profileId, taskName);
    else context.setSelectedTask(profileId, taskName);
  };

  function resetPending(): void {
    pendingLaunchAction = null;
    pendingDeleteProfileId = null;
  }

  function requestLaunchAction(action: PendingLaunchAction): void {
    if (pendingLaunchAction !== null) return;
    const profile = findProfile(action.profileId);
    if (context.operations.has(launchProfileOperationKey(profile.id))) return;
    if (action.kind === "task") {
      const task = profile.tasks.find((item) => item.name === action.taskName);
      if (!task) throw new Error("The launch task no longer exists.");
      const state = context.snapshotFor(profile.id, task.name)?.state ?? "stopped";
      if (context.operations.has(`task:${profile.id}:${task.name}`)) return;
      if (action.direction === "start" && !launchTaskCanStart(state)) return;
      if ((action.direction === "stop" || action.direction === "restart") && !launchTaskCanStop(state) && state !== "external") return;
      pendingLaunchAction = action;
      const title = action.direction === "start" ? "Start task?" : action.direction === "restart" ? "Restart task?" : "Stop task?";
      // A container task is handed to Docker, so its copy talks about the container.
      const description = action.direction === "start"
        ? `Start <strong>${h(task.name)}</strong> in <strong>${h(profile.name)}</strong>? This will ${task.container ? "start its Docker container" : "launch the task process"}.`
        : action.direction === "restart"
          ? `Restart <strong>${h(task.name)}</strong> in <strong>${h(profile.name)}</strong>? This will stop and start ${task.container ? "its Docker container" : "the task process"}.`
          : `Stop <strong>${h(task.name)}</strong> in <strong>${h(profile.name)}</strong>? This will ${task.container ? "stop its Docker container" : "terminate the task process"}.`;
      const button = action.direction === "start"
        ? `<button class="primary-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("play", 13)} Start</button>`
        : action.direction === "restart"
          ? `<button class="primary-button icon-button-label" type="button" data-action="confirm-launch-action">${restartIcon(13)} Restart</button>`
          : `<button class="primary-button danger-confirm-button icon-button-label" type="button" data-action="confirm-launch-action">${uiIcon("stop", 13)} Stop</button>`;
      context.openModal(title, `<p class="confirm-copy">${description}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button>${button}</div>`);
      return;
    }

    if (hasTaskOperation(profile)) return;
    const canStart = profile.tasks.some((task) => {
      const state = context.snapshotFor(profile.id, task.name)?.state ?? "stopped";
      return launchTaskCanStart(state);
    });
    const canStop = profile.tasks.some((task) => launchTaskCanStop(context.snapshotFor(profile.id, task.name)?.state ?? "stopped"));
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
    context.openModal(title, `<p class="confirm-copy">${description}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button>${button}</div>`);
  }

  async function confirmLaunchAction(): Promise<void> {
    const action = pendingLaunchAction;
    if (!action) return;
    resetPending();
    context.closeModal();
    const profile = findProfile(action.profileId);
    if (action.kind === "task") {
      const task = profile.tasks.find((item) => item.name === action.taskName);
      if (!task) throw new Error("The launch task is no longer available.");
      const state = context.snapshotFor(profile.id, task.name)?.state ?? "stopped";
      if (context.operations.has(launchProfileOperationKey(profile.id)) || context.operations.has(`task:${profile.id}:${task.name}`) || (action.direction === "start" && !launchTaskCanStart(state)) || ((action.direction === "stop" || action.direction === "restart") && !launchTaskCanStop(state) && state !== "external")) {
        return;
      }
      await runTask(profile.id, task.name, action.direction);
      return;
    }

    if (context.operations.has(launchProfileOperationKey(profile.id)) || hasTaskOperation(profile)) return;
    const canStart = profile.tasks.some((task) => {
      const state = context.snapshotFor(profile.id, task.name)?.state ?? "stopped";
      return launchTaskCanStart(state);
    });
    const canStop = profile.tasks.some((task) => launchTaskCanStop(context.snapshotFor(profile.id, task.name)?.state ?? "stopped"));
    if ((action.direction === "start" && !canStart) || (action.direction === "stop" && !canStop)) return;
    if (action.direction === "start") await startProfile(profile.id);
    else await stopProfile(profile.id);
  }

  async function startProfile(id: string): Promise<void> {
    const profile = findProfile(id);
    const operationKey = launchProfileOperationKey(id);
    if (context.operations.has(operationKey) || hasTaskOperation(profile)) return;
    const firstStartable = profile.tasks.find((task) => launchTaskCanStart(context.snapshotFor(id, task.name)?.state ?? "stopped"));
    if (!firstStartable) return;
    context.operations.add(operationKey);
    context.renderLaunch(true);
    try {
      focusTask(id, firstStartable.name);
      for (const task of profile.tasks) {
        const state = context.snapshotFor(id, task.name)?.state ?? "stopped";
        if (launchTaskCanStart(state)) await runTask(id, task.name, "start", false, false);
      }
      await context.refreshLaunch(true);
    } finally {
      context.operations.delete(operationKey);
      context.renderLaunch(true);
    }
  }

  async function stopProfile(id: string): Promise<void> {
    const profile = findProfile(id);
    const operationKey = launchProfileOperationKey(id);
    if (context.operations.has(operationKey) || hasTaskOperation(profile)) return;
    const canStop = profile.tasks.some((task) => launchTaskCanStop(context.snapshotFor(id, task.name)?.state ?? "stopped"));
    if (!canStop) return;
    context.operations.add(operationKey);
    context.renderLaunch(true);
    try {
      context.setSnapshots(mergeSnapshots(context.getSnapshots(), await context.api.stopProfile(id)));
      await context.refreshLaunch(true);
    } finally {
      context.operations.delete(operationKey);
      context.renderLaunch(true);
    }
  }

  async function runTask(profileId: string, taskName: string, direction: "start" | "stop" | "restart", refresh = true, select = true): Promise<void> {
    const key = `task:${profileId}:${taskName}`;
    if (context.operations.has(key)) return;
    if (select) focusTask(profileId, taskName);
    context.operations.add(key);
    context.renderLaunch(true);
    const streamLogs = direction !== "stop" && context.api.taskLogTail && context.updateTaskLogTail;
    const readLatestLog = async (): Promise<void> => {
      if (!streamLogs) return;
      try {
        context.updateTaskLogTail!(profileId, taskName, await context.api.taskLogTail!(profileId, taskName));
      } catch {
        // The returned snapshot or normal refresh remains authoritative if the log is briefly
        // unavailable while the task process is being created.
      }
    };
    const stopLogPolling = streamLogs
      ? startRestartProgressPolling(
          () => context.api.taskLogTail!(profileId, taskName),
          (logTail) => context.updateTaskLogTail!(profileId, taskName, logTail),
          window
        )
      : null;
    try {
      const snapshot = direction === "start"
        ? await context.api.startTask(profileId, taskName)
        : direction === "restart"
          ? await context.api.restartTask(profileId, taskName)
          : await context.api.stopTask(profileId, taskName);
      context.setSnapshots(mergeSnapshots(context.getSnapshots(), [snapshot]));
      if (snapshot.message) context.toast(snapshot.message, snapshot.state === "failed");
    } catch (error) {
      await readLatestLog();
      throw error;
    } finally {
      stopLogPolling?.();
      context.operations.delete(key);
      if (refresh) await context.refreshLaunch(true);
    }
  }

  async function saveProfileFromModal(id: string | null): Promise<void> {
    if (!context.hasProfileForm()) return;
    if (id && blocksEditing(findProfile(id))) throw new Error("Stop every task in this profile before editing it.");
    const profile = context.readProfileForm(id);
    if (!profile) return;
    context.setProfiles(await context.api.saveProfile(profile));
    context.closeModal();
    context.renderHeaderCounts();
    context.renderLaunch(true);
    context.toast("Launch profile saved.");
  }

  function requestDeleteProfile(id: string): void {
    if (pendingDeleteProfileId !== null) return;
    const profile = findProfile(id);
    pendingDeleteProfileId = id;
    // A running profile can still be deleted; the tasks Cutting Board started are stopped with it.
    const runningNote = blocksEditing(profile) ? " Tasks Cutting Board started for it are stopped first." : "";
    context.openModal("Delete launch profile?", `<p class="confirm-copy">Delete <strong>${h(profile.name)}</strong>? This removes its saved commands.${runningNote}</p><div class="modal-actions"><button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button danger-confirm-button" type="button" data-action="confirm-delete-profile" data-profile-id="${h(id)}">Delete</button></div>`);
  }

  async function confirmDeleteProfile(id: string): Promise<void> {
    if (pendingDeleteProfileId !== id) return;
    resetPending();
    context.closeModal();
    context.setProfiles(await context.api.deleteProfile(id));
    context.renderHeaderCounts();
    context.renderLaunch(true);
    context.toast("Launch profile deleted.");
  }

  return {
    blocksEditing,
    hasTaskOperation,
    requestLaunchAction,
    confirmLaunchAction,
    startProfile,
    stopProfile,
    runTask,
    saveProfileFromModal,
    requestDeleteProfile,
    confirmDeleteProfile,
    resetPending
  };
}
