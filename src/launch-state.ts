import type { LaunchProfile, LaunchState, ManagedTaskSnapshot } from "./types";

export function launchTaskIsActive(state: LaunchState): boolean {
  return state === "starting" || state === "running" || state === "stopping";
}

export function launchTaskCanStart(state: LaunchState): boolean {
  return !launchTaskIsActive(state) && state !== "external";
}

export function launchTaskCanStop(state: LaunchState): boolean {
  return launchTaskIsActive(state);
}

export function launchProfileIsActive(states: readonly LaunchState[]): boolean {
  return states.some((state) => launchTaskIsActive(state) || state === "external");
}

/** A profile is idle when it has tasks and none of them is running, starting, stopping, or external. */
export function launchProfileIsIdle(states: readonly LaunchState[]): boolean {
  return states.length > 0 && states.every(launchTaskCanStart);
}

/** A profile is fully stopped only when every task reports the stopped state. */
export function launchProfileIsFullyStopped(states: readonly LaunchState[]): boolean {
  return states.length > 0 && states.every((state) => state === "stopped");
}

/** Active and failed profiles stay expanded; fully stopped profiles expand only after the user chooses them. */
export function launchProfileIsExpanded(states: readonly LaunchState[], userExpanded: boolean): boolean {
  return !launchProfileIsFullyStopped(states) || userExpanded;
}

export function orderLaunchProfiles(
  profiles: readonly LaunchProfile[],
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined
): LaunchProfile[] {
  const active: LaunchProfile[] = [];
  const inactive: LaunchProfile[] = [];
  for (const profile of profiles) {
    const states = profile.tasks.map((task) => snapshotFor(profile.id, task.name)?.state ?? "stopped");
    (launchProfileIsActive(states) ? active : inactive).push(profile);
  }
  return [...active, ...inactive];
}

export function launchStatePriority(state: LaunchState): number {
  return ({ running: 0, starting: 1, stopping: 2, external: 3, failed: 4, stopped: 5 })[state];
}

export function stateLabel(state: LaunchState): string {
  return ({ stopped: "Stopped", starting: "Starting", running: "Running", stopping: "Stopping", failed: "Failed", external: "Running externally" })[state];
}
