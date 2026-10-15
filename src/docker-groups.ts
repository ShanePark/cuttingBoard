import type { ContainerInfo } from "./types";

export const STANDALONE_GROUP_NAME = "Standalone containers";

export type ContainerGroup = {
  name: string;
  /** The real Compose project label; null for the standalone group. */
  project: string | null;
  containers: ContainerInfo[];
};

export type ComposeDownControl = {
  project: string;
  busy: boolean;
  disabled: boolean;
  title: string;
  ariaLabel: string;
};

export function composeDownOperationKey(project: string): string {
  return `compose-down:${project}`;
}

export function groupContainers(containers: readonly ContainerInfo[]): ContainerGroup[] {
  const map = new Map<string | null, ContainerInfo[]>();
  for (const container of containers) {
    const project = container.compose_project ?? null;
    const group = map.get(project) ?? [];
    group.push(container);
    map.set(project, group);
  }
  return [...map].map(([project, items]) => ({
    name: project ?? STANDALONE_GROUP_NAME,
    project,
    containers: items.sort((a, b) => Number(b.state === "running") - Number(a.state === "running") || a.name.localeCompare(b.name))
  })).sort((a, b) => a.project === null ? 1 : b.project === null ? -1 : a.name.localeCompare(b.name));
}

/** Describes the group-level `docker compose down` button, or null when the group gets none. */
export function composeDownControl(group: ContainerGroup, busy: boolean): ComposeDownControl | null {
  if (group.project === null || group.containers.length <= 1) return null;
  const running = group.containers.some((container) => container.state === "running");
  return {
    project: group.project,
    busy,
    disabled: busy || !running,
    title: busy ? "Stopping compose project" : running ? "Stop compose project (docker compose down)" : "No running containers",
    ariaLabel: `Stop all containers in ${group.project}`
  };
}

/** Output of the latest `docker compose down`, shown in the Docker console in place of container logs. */
export type ComposeDownConsoleState = {
  project: string;
  log: string;
  status: "running" | "succeeded" | "failed";
  error: string | null;
};

export function composeDownOutputKind(state: ComposeDownConsoleState): "log" | "log-alert" | "loading" | "error" | "empty" {
  if (state.log.trim()) return state.error ? "log-alert" : "log";
  if (state.status === "running") return "loading";
  return state.error ? "error" : "empty";
}

export function composeDownStatusText(state: ComposeDownConsoleState): string {
  return state.status === "running" ? "Running docker compose down" : state.status === "succeeded" ? "Completed" : "Failed";
}

export function composeDownStateClass(state: ComposeDownConsoleState): "stopping" | "stopped" | "failed" {
  return state.status === "running" ? "stopping" : state.status === "succeeded" ? "stopped" : "failed";
}

export type ComposeDownLogScheduler = {
  setInterval: (callback: () => void, delayMs: number) => number;
  clearInterval: (id: number) => void;
};

export const COMPOSE_DOWN_LOG_POLL_INTERVAL_MS = 250;

/**
 * Polls the native `docker compose down` output while the run is in flight. The returned function
 * stops polling and performs one final read after any read still in flight, so the last output
 * always wins.
 */
export function startComposeDownLogPolling(
  readLog: () => Promise<string>,
  onLog: (log: string) => void,
  scheduler: ComposeDownLogScheduler,
  intervalMs = COMPOSE_DOWN_LOG_POLL_INTERVAL_MS
): () => Promise<void> {
  let stopped = false;
  let inFlight: Promise<void> | null = null;
  const read = async (): Promise<void> => {
    try {
      onLog(await readLog());
    } catch {
      // A failed read keeps the last output; the run's result reports the failure.
    }
  };
  // The first read waits one interval so the native run has replaced the previous run's output.
  const timer = scheduler.setInterval(() => {
    if (stopped || inFlight) return;
    inFlight = read().finally(() => { inFlight = null; });
  }, intervalMs);
  return async () => {
    stopped = true;
    scheduler.clearInterval(timer);
    await inFlight;
    await read();
  };
}
