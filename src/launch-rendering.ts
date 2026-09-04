import { restartIcon, techIcon, uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import { launchTaskRestartOperationKey } from "./launch-actions";
import {
  FRESH_UPTIME_SECONDS,
  currentUptime,
  formatBytes,
  launchTaskTech,
  matchedServiceForTask
} from "./presentation";
import {
  launchProfileIsIdle,
  launchTaskCanStart,
  launchTaskCanStop,
  launchTaskIsActive,
  launchStatePriority,
  stateLabel
} from "./launch-state";
import { renderGroupCount, renderOpenServiceButton, renderSharedServiceCard } from "./tile-rendering";
import type {
  ContainerInfo,
  LaunchProfile,
  LaunchState,
  LaunchTask,
  ManagedTaskSnapshot,
  ServiceSnapshot
} from "./types";

export type LaunchTaskRef = { profile: LaunchProfile; task: LaunchTask };

export type LaunchRenderingContext = {
  appIsDemo: boolean;
  operations: ReadonlySet<string>;
  selectedTaskKey: string | null;
  services: readonly ServiceSnapshot[];
  containers: readonly ContainerInfo[];
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined;
  launchProfileOperationKey: (profileId: string) => string;
  launchProfileHasTaskOperation: (profile: LaunchProfile) => boolean;
  renderGroupTitle: (title: string, itemCount: number, action: string, attributes: string, displayTitle?: string, actionTitle?: string) => string;
  uptimeText: (service: ServiceSnapshot, stopping: boolean, restarting?: boolean) => string;
};

export type LaunchConsoleRenderingContext = {
  consoleOpen: boolean;
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined;
  renderConsoleResizer: () => string;
  renderConsoleJumpButton: () => string;
};

export function renderLaunchAddCard(appIsDemo: boolean): string {
  return `<article class="launch-add-card" aria-labelledby="launch-add-title">
    <button class="launch-add-action" type="button" data-action="add-profile" aria-label="Add launch profile" title="Add launch profile" ${appIsDemo ? "disabled" : ""}>
      <span class="launch-add-icon" aria-hidden="true">${uiIcon("plus", 24)}</span>
      <strong id="launch-add-title">Add profile</strong>
    </button>
    <button class="info-button icon-only-button launch-add-info" type="button" data-action="show-info" data-info-kind="launch" aria-label="About launch profiles" title="About launch profiles">${uiIcon("info", 15)}</button>
  </article>`;
}

export function bulkActionIcon(name: "play" | "stop"): string {
  return `<span class="bulk-action-icon bulk-action-${name}" aria-hidden="true">${uiIcon(name, 15, "bulk-action-icon-back")}${uiIcon(name, 15, "bulk-action-icon-front")}</span>`;
}

export function renderProfile(profile: LaunchProfile, context: LaunchRenderingContext): string {
  const snapshots = profile.tasks.map((task) => context.snapshotFor(profile.id, task.name));
  const canStop = snapshots.some((snapshot) => snapshot && launchTaskCanStop(snapshot.state));
  const canStart = profile.tasks.some((task) => launchTaskCanStart(context.snapshotFor(profile.id, task.name)?.state ?? "stopped"));
  // Nothing in the profile is running, so the whole group recedes and says so in its header.
  const idle = launchProfileIsIdle(snapshots.map((snapshot) => snapshot?.state ?? "stopped"));
  const profileBusy = context.operations.has(context.launchProfileOperationKey(profile.id)) || context.launchProfileHasTaskOperation(profile);
  const runAll = profile.tasks.length > 1 && canStart
    ? `<button class="primary-button icon-only-button bulk-action-button" type="button" data-action="start-profile" data-profile-id="${h(profile.id)}" aria-label="${canStop ? "Start remaining tasks" : "Run all tasks"}" title="${profileBusy ? "Starting tasks" : canStop ? "Start remaining tasks" : "Run all tasks"}" ${profileBusy ? "disabled" : ""}>${bulkActionIcon("play")}</button>`
    : "";
  const stopAll = profile.tasks.length > 1 && canStop
    ? `<button class="secondary-button danger-button icon-only-button bulk-action-button" type="button" data-action="stop-profile" data-profile-id="${h(profile.id)}" aria-label="Stop all tasks" title="${profileBusy ? "Stopping tasks" : "Stop all tasks"}" ${profileBusy ? "disabled" : ""}>${bulkActionIcon("stop")}</button>`
    : "";
  return `<section class="launch-profile service-section${idle ? " is-idle" : ""}" data-tiles="${profile.tasks.length}" aria-labelledby="launch-profile-${h(profile.id)}">
    <header class="section-header launch-profile-header">
      <span class="section-accent accent-runtime" aria-hidden="true"></span>
      <div class="launch-profile-heading"><h2 id="launch-profile-${h(profile.id)}">${context.renderGroupTitle(profile.name, profile.tasks.length, "profile-details", `data-profile-id="${h(profile.id)}"`, profile.name.toUpperCase(), "View profile details")}${renderGroupCount(profile.tasks.length)}${idle ? `<span class="section-state">Stopped</span>` : ""}</h2></div>
      <div class="section-actions launch-profile-actions">
        ${runAll}${stopAll}
        <button class="section-action icon-only-button" type="button" data-action="edit-profile" data-profile-id="${h(profile.id)}" aria-label="Edit ${h(profile.name)}" title="Edit profile" ${context.appIsDemo ? "disabled" : ""}>${uiIcon("settings", 15)}</button>
      </div>
    </header>
    <div class="task-list" role="list" aria-label="Tasks in ${h(profile.name)}">${profile.tasks.map((task) => renderTask(profile, task, context)).join("")}</div>
  </section>`;
}

export function renderTask(profile: LaunchProfile, task: LaunchTask, context: LaunchRenderingContext): string {
  const snapshot = context.snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  const external = state === "external";
  const externallyManaged = external || Boolean(snapshot?.external_pid);
  const active = launchTaskIsActive(state) || externallyManaged;
  const matchedService = matchedServiceForTask(profile, task, context.services);
  const tech = launchTaskTech(task, matchedService, context.containers);
  const taskOperation = context.operations.has(`task:${profile.id}:${task.name}`);
  const serviceStopping = matchedService ? context.operations.has(`stop:${matchedService.id}`) : false;
  const serviceRestarting = matchedService ? context.operations.has(`restart:${matchedService.id}`) : false;
  const taskRestarting = context.operations.has(launchTaskRestartOperationKey(profile.id, task.name));
  const serviceOperation = serviceStopping || serviceRestarting;
  const profileOperation = context.operations.has(context.launchProfileOperationKey(profile.id));
  const busy = taskOperation || serviceOperation || profileOperation;
  const selected = context.selectedTaskKey === launchTaskKey(profile.id, task.name);
  const externalCanStop = external && Boolean(matchedService?.can_terminate);
  const canStop = external ? externalCanStop : launchTaskCanStop(state);
  const stopUnavailable = !canStop;
  const detailsAction = `<button type="button" class="info-button icon-only-button service-card-control task-card-action" data-tile-action data-action="task-details" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" aria-label="View ${h(task.name)} details" title="View task details">${uiIcon("info", 15)}</button>`;
  const startAction = !active && !external
    ? `<button class="quiet-button start-action icon-only-button service-card-control task-card-action" type="button" data-tile-action data-action="start-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" aria-label="Start ${h(task.name)}" title="Start task" ${busy ? "disabled" : ""}>${uiIcon(busy ? "refresh" : "play", 15)}</button>`
    : "";
  const restartAction = canStop
    ? `<button class="restart-action icon-only-button service-card-control task-card-action" type="button" data-tile-action data-action="restart-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" aria-label="${busy ? "Restarting" : "Restart"} ${h(task.name)}" title="${busy ? "Restarting task" : "Restart task"}" ${busy ? "disabled" : ""}>${restartIcon(15)}</button>`
    : "";
  const stopAction = `<button class="stop-button service-card-control task-card-action${stopUnavailable ? " is-unavailable" : ""}" type="button" data-tile-action data-action="${externalCanStop ? "stop-service" : "stop-task"}"${externalCanStop ? ` data-service-id="${h(matchedService?.id ?? "")}"` : ` data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}"`} aria-label="${externalCanStop ? "Stop externally managed service" : `Stop ${task.name}`}" title="${active ? busy ? "Stopping task" : "Stop task" : externalCanStop ? "Stop externally managed service" : external ? "Cannot stop an externally managed task" : "Task is not running"}" ${stopUnavailable || busy ? "disabled" : ""}>${uiIcon("stop", 15)}</button>`;
  const matchedUptime = matchedService ? currentUptime(matchedService) : null;
  const metricsMarkup = matchedService
    ? `<span class="metric metric-uptime${matchedUptime !== null && matchedUptime < FRESH_UPTIME_SECONDS ? " is-fresh" : ""}" data-metric="uptime" title="Uptime">${uiIcon("clock", 13)}<span class="sr-only">Uptime </span><span data-metric-text>${h(context.uptimeText(matchedService, serviceStopping, serviceRestarting || taskRestarting))}</span></span><span class="metric metric-memory" data-metric="memory" title="Memory used">${uiIcon("memory", 13)}<span class="sr-only">Memory </span><span data-metric-text>${h(formatBytes(matchedService.process?.memory_bytes ?? null))}</span></span>`
    : state === "external"
      ? `<span class="metric metric-state is-external" title="Running externally">${uiIcon("terminal", 13)}<span class="sr-only">Running externally</span></span>`
      : `<span class="metric metric-state ${busy ? "is-busy" : `state-${state}`}" title="Task state">${uiIcon(state === "running" ? "play" : "terminal", 13)}<span class="sr-only">State </span>${h(stateLabel(state))}</span>`;
  const cardAttributes = ` data-action="select-task" data-profile-id="${h(profile.id)}" data-task-name="${h(task.name)}" tabindex="0" role="listitem" aria-current="${selected ? "true" : "false"}"`;
  // A stopped task has no port bound yet, so the shortcut only appears once a service backs it.
  const trailingMarkup = matchedService ? renderOpenServiceButton(matchedService, task.name) : "";
  return renderSharedServiceCard({
    category: matchedService?.category ?? "runtime",
    cardClass: `task-card state-${state}`,
    metricsId: matchedService?.id,
    cardAttributes,
    ariaLabel: `${profile.name} · ${task.name}, ${stateLabel(state)}${task.expected_port ? `, port ${task.expected_port}` : ""}`,
    selected,
    busy,
    iconMarkup: tech ? techIcon(tech, 44) : uiIcon("terminal", 25),
    pipClass: busy ? "busy" : state === "external" ? "external" : state === "stopped" || state === "failed" ? "idle" : "running",
    title: task.name,
    controlsMarkup: `${detailsAction}${startAction}${restartAction}${stopAction}`,
    metricsMarkup,
    ports: task.expected_port ? [task.expected_port] : [],
    emptyPortLabel: "No expected port",
    trailingMarkup
  });
}

export function launchTaskKey(profileId: string, taskName: string): string {
  return `${profileId}\0${taskName}`;
}

// Keep the NUL-delimited key for in-memory identity, but never put it in HTML:
// HTML parsers normalize NUL characters in attribute values.
export function launchTaskDomKey(profileId: string, taskName: string): string {
  return `${encodeURIComponent(profileId)}:${encodeURIComponent(taskName)}`;
}

export function launchTaskRefs(profiles: readonly LaunchProfile[]): LaunchTaskRef[] {
  return profiles.flatMap((profile) => profile.tasks.map((task) => ({ profile, task })));
}

export function launchTaskRefForKey(profiles: readonly LaunchProfile[], key: string | null): LaunchTaskRef | null {
  if (!key) return null;
  return launchTaskRefs(profiles).find(({ profile, task }) => launchTaskKey(profile.id, task.name) === key) ?? null;
}

export function selectedTaskDomKey(profiles: readonly LaunchProfile[], key: string | null): string | null {
  const ref = launchTaskRefForKey(profiles, key);
  return ref ? launchTaskDomKey(ref.profile.id, ref.task.name) : null;
}

export function ensureSelectedTask(
  profiles: readonly LaunchProfile[],
  key: string | null,
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined
): LaunchTaskRef | null {
  const existing = launchTaskRefForKey(profiles, key);
  if (existing) return existing;
  const refs = launchTaskRefs(profiles);
  return refs
    .map((ref, index) => ({ ref, index, state: snapshotFor(ref.profile.id, ref.task.name)?.state ?? "stopped" as LaunchState }))
    .filter(({ state }) => ["running", "starting", "stopping", "external"].includes(state))
    .sort((a, b) => launchStatePriority(a.state) - launchStatePriority(b.state) || a.index - b.index)[0]?.ref
    ?? refs[0]
    ?? null;
}

export function renderLaunchConsole(ref: LaunchTaskRef | null, context: LaunchConsoleRenderingContext): string {
  if (!context.consoleOpen) return "";
  if (!ref) {
    return `<section class="launch-console launch-console-empty" aria-labelledby="launch-console-title">${context.renderConsoleResizer()}<header class="console-header"><div class="console-title"><span class="console-icon" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="launch-console-title">Task console</h2></div></div><div class="console-meta" aria-label="Task status"><span class="console-state"><span class="task-state-dot" aria-hidden="true"></span>No task selected</span></div></header><div class="console-output-shell"><div class="console-output console-empty-output" data-console-output-kind="empty" role="status"><div class="console-message"><strong>No tasks available</strong><span>Add at least one task to this launch profile.</span></div></div>${context.renderConsoleJumpButton()}</div></section>`;
  }
  const { profile, task } = ref;
  const snapshot = context.snapshotFor(profile.id, task.name);
  const state: LaunchState = snapshot?.state ?? "stopped";
  return `<section class="launch-console state-${state}" aria-labelledby="launch-console-title" data-console-task-key="${h(launchTaskDomKey(profile.id, task.name))}">
    ${context.renderConsoleResizer()}
    <header class="console-header">
      <div class="console-title"><span class="console-icon state-${state}" aria-hidden="true">${uiIcon("terminal", 16)}</span><div><h2 id="launch-console-title">${h(task.name)}</h2></div></div>
      <div class="console-meta" aria-label="Task status"><span class="console-state state-${state}"><span class="task-state-dot" aria-hidden="true"></span>${h(stateLabel(state))}</span></div>
    </header>
    <div class="console-output-shell"><div class="console-output" data-console-output-kind="${launchConsoleOutputKind(snapshot, state)}" tabindex="0" role="log" aria-live="polite" aria-label="Output for ${h(task.name)}">${renderConsoleOutput(snapshot, state)}</div>${context.renderConsoleJumpButton()}</div>
  </section>`;
}

export function launchConsoleOutputKind(snapshot: ManagedTaskSnapshot | undefined, state: LaunchState): string {
  const log = snapshot?.log_tail ?? "";
  if (state === "external") return log.length > 0 ? "log" : "external";
  if (state === "failed") return log.length > 0 ? "log-alert" : "failed";
  return log.length > 0 ? "log" : "empty";
}

export function renderConsoleOutput(snapshot: ManagedTaskSnapshot | undefined, state: LaunchState): string {
  const log = snapshot?.log_tail ?? "";
  if (state === "external") {
    if (log.length > 0) return `<pre class="console-log">${h(log)}</pre>`;
    const message = snapshot?.external_log_path
      ? "No output is available from the configured external log source yet."
      : "This process was started outside Cutting Board, so its output goes to whatever started it, such as an IDE or a terminal. Output appears here once the process writes to a log file (for a Spring Boot app, set logging.file.name) or when the task is started from this profile.";
    return `<div class="console-message is-external"><span class="console-message-icon">${uiIcon("terminal", 18)}</span><strong>${snapshot?.external_log_path ? "Waiting for external output" : "Output unavailable"}</strong><span>${message}</span></div>`;
  }
  const message = snapshot?.message?.trim() ?? "";
  const notice = state === "failed"
    ? `<div class="console-alert">${uiIcon("warning", 14)}<span>${h(message || "The task exited before completing successfully.")}</span></div>`
    : "";
  if (log.length > 0) return `${notice}<pre class="console-log">${h(log)}</pre>`;
  if (state === "failed") {
    return `<div class="console-message is-failed"><span class="console-message-icon">${uiIcon("warning", 18)}</span><strong>Task failed before producing output</strong><span>${h(message || "The task exited before completing successfully.")}</span></div>`;
  }
  const copy = state === "starting"
    ? "Waiting for the task to produce output…"
    : state === "running"
      ? "No output has been captured yet."
      : state === "stopping"
        ? "Stopping task…"
        : "No output has been captured for this task.";
  return `${notice}<div class="console-message"><span class="console-message-icon">${uiIcon("log", 18)}</span><strong>${h(copy)}</strong><span>${state === "stopped" ? "Start the task to stream its output here." : "New output will appear here automatically."}</span></div>`;
}
