/**
 * Progress derived from the native launch manager's task log while a prepared task is being
 * restarted.
 *
 * This state is intentionally independent from ManagedTaskSnapshot: restart preparation happens
 * before the old process is replaced, so a snapshot can be blocked behind the launch manager's
 * mutex while these updates are still useful to the UI.
 */
export type RestartProgressPhase = "preparing" | "building" | "starting" | "completed" | "failed";

export interface RestartProgressEvent {
  profile_id: string;
  task_name: string;
  phase: RestartProgressPhase;
  message: string;
  detail?: string;
  log_tail?: string;
}

export interface ServiceRestartProgress {
  serviceId: string;
  profileId: string;
  taskName: string;
  phase: RestartProgressPhase;
  message: string;
  detail: string | null;
  logTail: string;
  startedAt: number;
  /** Becomes true only after a post-restart scan observes and selects the replacement service. */
  remapped: boolean;
}

export interface RestartProgressScheduler {
  setInterval: (callback: () => void, delayMs: number) => number;
  clearInterval: (id: number) => void;
}

export const RESTART_PROGRESS_POLL_INTERVAL_MS = 250;

const PHASES: readonly RestartProgressPhase[] = [
  "preparing",
  "building",
  "starting",
  "completed",
  "failed"
];

const DEFAULT_MESSAGES: Record<RestartProgressPhase, string> = {
  preparing: "Detecting project build settings…",
  building: "Building changed project modules…",
  starting: "Starting the refreshed service…",
  completed: "Restart completed.",
  failed: "Restart preparation failed."
};

export function normaliseRestartProgress(payload: unknown): RestartProgressEvent | null {
  if (!isRecord(payload)) return null;
  const profileId = stringValue(payload.profile_id) ?? stringValue(payload.profileId);
  const taskName = stringValue(payload.task_name) ?? stringValue(payload.taskName);
  if (!profileId || !taskName) return null;
  const phase = normalisePhase(payload.phase);
  const message = stringValue(payload.message) ?? DEFAULT_MESSAGES[phase];
  const detail = stringValue(payload.detail) ?? stringValue(payload.line) ?? stringValue(payload.output);
  const logTail = stringValue(payload.log_tail) ?? stringValue(payload.logTail);
  return {
    profile_id: profileId,
    task_name: taskName,
    phase,
    message,
    ...(detail ? { detail } : {}),
    ...(logTail !== null ? { log_tail: logTail } : {})
  };
}

export function restartProgressMatches(
  progress: Pick<ServiceRestartProgress, "profileId" | "taskName">,
  event: Pick<RestartProgressEvent, "profile_id" | "task_name">
): boolean {
  return progress.profileId === event.profile_id && progress.taskName === event.task_name;
}

export function progressStatusText(progress: Pick<RestartProgressEvent, "message" | "detail">): string {
  return [progress.message, progress.detail].filter((value): value is string => Boolean(value?.trim())).join(" · ");
}

export function appendProgressLine(logTail: string, event: Pick<RestartProgressEvent, "message" | "detail">): string {
  const line = progressStatusText(event);
  if (!line) return logTail;
  if (logTail.trimEnd().endsWith(line)) return logTail;
  return `${logTail}${logTail && !logTail.endsWith("\n") ? "\n" : ""}${line}\n`;
}

export function initialRestartProgress(
  serviceId: string,
  profileId: string,
  taskName: string,
  startedAt = Date.now()
): ServiceRestartProgress {
  const event: RestartProgressEvent = {
    profile_id: profileId,
    task_name: taskName,
    phase: "preparing",
    message: DEFAULT_MESSAGES.preparing
  };
  return {
    serviceId,
    profileId,
    taskName,
    phase: event.phase,
    message: event.message,
    detail: null,
    logTail: `=== Cutting Board restart · ${taskName} ===\n${progressStatusText(event)}\n`,
    startedAt,
    remapped: false
  };
}

export function remapRestartProgress(progress: ServiceRestartProgress, serviceId: string): ServiceRestartProgress {
  return { ...progress, serviceId, remapped: true };
}

/** Keep both the original card and its PID-replaced successor guarded during one restart. */
export function restartProgressBusyForService(
  progress: ServiceRestartProgress | null,
  serviceId: string,
  nativeOperationPending: boolean
): boolean {
  if (!progress || progress.serviceId !== serviceId || progress.phase === "failed") return false;
  if (progress.phase === "completed" && !progress.remapped) return nativeOperationPending;
  return true;
}

export function shouldClearCompletedRestartProgress(progress: Pick<ServiceRestartProgress, "phase" | "remapped">): boolean {
  return progress.phase === "completed" && progress.remapped;
}

/** Poll the lock-free task log while restart_task is preparing the replacement process. */
export function startRestartProgressPolling(
  readLogTail: () => Promise<string>,
  onLogTail: (logTail: string) => void,
  scheduler: RestartProgressScheduler,
  intervalMs = RESTART_PROGRESS_POLL_INTERVAL_MS
): () => void {
  let stopped = false;
  let busy = false;
  let lastLogTail: string | null = null;
  const poll = async (): Promise<void> => {
    if (stopped || busy) return;
    busy = true;
    try {
      const logTail = await readLogTail();
      if (stopped || lastLogTail === logTail) return;
      // The first read may still contain the previous restart's append-only output. Establish it
      // as a baseline; only a later change can be attributed to the restart currently in flight.
      if (lastLogTail === null) {
        lastLogTail = logTail;
        return;
      }
      lastLogTail = logTail;
      onLogTail(logTail);
    } catch {
      // A transient read failure must not stop polling; the task log can be created a moment
      // after the restart request and the next poll will pick it up.
    } finally {
      busy = false;
    }
  };
  void poll();
  const timer = scheduler.setInterval(() => void poll(), intervalMs);
  return () => {
    stopped = true;
    scheduler.clearInterval(timer);
  };
}

/** Infer a human-readable phase from the native lifecycle markers written to the task log. */
export function progressFromTaskLog(
  progress: ServiceRestartProgress,
  logTail: string
): ServiceRestartProgress {
  const lower = logTail.toLowerCase();
  // Task logs are append-only across restarts. Restrict lifecycle detection to the newest
  // restart section so a previous "new process started" marker cannot complete a current build.
  const marker = "=== cutting board restart requested";
  const markerIndex = lower.lastIndexOf(marker);
  const current = markerIndex >= 0 ? lower.slice(markerIndex) : "";
  const phase = current.includes("prepare failed") || current.includes("process start failed")
    ? "failed"
    : current.includes("new process started")
      ? "completed"
      : current.includes("previous process stopped") || current.includes("starting new process")
        ? "starting"
        : current.includes("prepare")
          ? "building"
          : progress.phase;
  const message = phase === "failed"
    ? "Restart preparation failed."
    : phase === "completed"
      ? "Restart completed."
      : phase === "starting"
        ? "Starting the refreshed service…"
        : phase === "building"
          ? "Preparing changed project modules…"
          : progress.message;
  const detail = lastProgressLine(current) ?? progress.detail;
  return { ...progress, phase, message, detail, logTail };
}

function normalisePhase(value: unknown): RestartProgressPhase {
  return typeof value === "string" && PHASES.includes(value as RestartProgressPhase)
    ? value as RestartProgressPhase
    : "preparing";
}

function lastProgressLine(logTail: string): string | null {
  const lines = logTail
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const line = lines.at(-1);
  if (!line) return null;
  // Keep tool output visible in the console while removing the decorative marker wrapper from
  // the status pill, which is easier to scan as a single sentence.
  return line.replace(/^===\s*/, "").replace(/\s*===\s*$/, "").trim() || null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
