import type { LaunchState } from "./types";

export function launchTaskIsActive(state: LaunchState): boolean {
  return state === "starting" || state === "running" || state === "stopping";
}

export function launchTaskCanStart(state: LaunchState): boolean {
  return !launchTaskIsActive(state) && state !== "external";
}

export function launchTaskCanStop(state: LaunchState): boolean {
  return launchTaskIsActive(state);
}

export function launchStatePriority(state: LaunchState): number {
  return ({ running: 0, starting: 1, stopping: 2, external: 3, failed: 4, stopped: 5 })[state];
}

export function stateLabel(state: LaunchState): string {
  return ({ stopped: "Stopped", starting: "Starting", running: "Running", stopping: "Stopping", failed: "Failed", external: "Running externally" })[state];
}
