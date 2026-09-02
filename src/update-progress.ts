export const UPDATE_PROGRESS_STAGES = [
  {
    id: "validating",
    label: "Validate source",
    detail: "Checking the latest local commit"
  },
  {
    id: "building",
    label: "Build release",
    detail: "Compiling a fresh macOS release app"
  },
  {
    id: "preparing",
    label: "Prepare installation",
    detail: "Staging the new application bundle"
  },
  {
    id: "restarting",
    label: "Restart app",
    detail: "Launching the updated Cutting Board app"
  }
] as const;

export type UpdateProgressStage = typeof UPDATE_PROGRESS_STAGES[number]["id"];

export interface UpdateProgressEvent {
  stage: UpdateProgressStage;
  step: number;
  total: number;
  message: string;
}

export interface NormalizedUpdateProgress {
  stage: UpdateProgressStage;
  step: number;
  total: number;
  message: string;
}

export interface UpdateProgressView {
  start: () => void;
  update: (payload: unknown) => void;
  fail: () => void;
  isActive: () => boolean;
}

const DEFAULT_TOTAL = UPDATE_PROGRESS_STAGES.length;
const FIRST_UPDATE_PROGRESS_STAGE = UPDATE_PROGRESS_STAGES[0]!;

export function isUpdateProgressStage(value: string): value is UpdateProgressStage {
  return UPDATE_PROGRESS_STAGES.some((stage) => stage.id === value);
}

export function updateProgressStageIndex(stage: UpdateProgressStage): number {
  return Math.max(0, UPDATE_PROGRESS_STAGES.findIndex((item) => item.id === stage));
}

export function normaliseUpdateProgress(payload: unknown): NormalizedUpdateProgress {
  const value = isRecord(payload) ? payload : {};
  const stage = typeof value.stage === "string" && isUpdateProgressStage(value.stage)
    ? value.stage
    : FIRST_UPDATE_PROGRESS_STAGE.id;
  const total = positiveInteger(value.total, DEFAULT_TOTAL, DEFAULT_TOTAL);
  const step = clamp(positiveInteger(value.step, 1, 1), 1, total);
  const message = typeof value.message === "string" && value.message.trim()
    ? value.message.trim()
    : UPDATE_PROGRESS_STAGES[updateProgressStageIndex(stage)]!.detail;
  return { stage, step, total, message };
}

export function formatUpdateElapsed(seconds: number): string {
  const elapsed = Math.max(0, Math.floor(Number.isFinite(seconds) ? seconds : 0));
  if (elapsed < 60) return `${elapsed}s`;
  const minutes = Math.floor(elapsed / 60);
  const remainder = elapsed % 60;
  if (minutes < 60) return `${minutes}m ${remainder}s`;
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${minutes % 60}m`;
}

export function renderUpdateProgressOverlay(): string {
  const stages = UPDATE_PROGRESS_STAGES.map((stage, index) => {
    const current = index === 0;
    return `<li class="update-progress-stage" data-update-stage="${stage.id}" data-state="${current ? "current" : "pending"}"${current ? " aria-current=\"step\"" : ""} aria-label="${escapeProgressText(`${stage.label}: ${current ? "in progress" : "pending"}`)}"><span class="update-progress-stage-mark" aria-hidden="true">${current ? "" : index + 1}</span><span class="update-progress-stage-copy"><strong>${escapeProgressText(stage.label)}</strong><small>${escapeProgressText(stage.detail)}</small></span></li>`;
  }).join("");

  return `<div class="update-progress-backdrop" data-update-progress-overlay role="presentation"><section class="update-progress-dialog" role="dialog" aria-modal="true" aria-busy="true" aria-labelledby="update-progress-title" aria-describedby="update-progress-message" tabindex="-1"><div class="update-progress-intro"><div class="update-progress-spinner" aria-hidden="true"><span></span></div><div class="update-progress-heading"><p class="update-progress-kicker">APPLICATION UPDATE</p><h2 id="update-progress-title">Updating Cutting Board</h2><p id="update-progress-message" role="status" aria-live="polite" aria-atomic="true">Checking the latest local commit…</p></div></div><div class="update-progress-summary"><span class="update-progress-step-label" data-update-step>Step 1 of ${DEFAULT_TOTAL}</span><span class="update-progress-elapsed"><span>Elapsed</span><time data-update-elapsed datetime="PT0S">0s</time></span></div><div class="update-progress-track" role="progressbar" aria-label="Update progress" aria-valuemin="1" aria-valuemax="${DEFAULT_TOTAL}" aria-valuenow="1" aria-valuetext="Step 1 of ${DEFAULT_TOTAL} — Validate source"><span class="update-progress-fill" data-update-progress-fill></span><span class="update-progress-shimmer" aria-hidden="true"></span></div><ol class="update-progress-stages" aria-label="Update steps">${stages}</ol><p class="update-progress-footnote"><span class="update-progress-footnote-dot" aria-hidden="true"></span>Keep this window open while the release build completes.</p></section></div>`;
}

export function createUpdateProgressView(): UpdateProgressView {
  let active = false;
  let startedAt = 0;
  let elapsedTimer: number | null = null;
  let focusReturn: HTMLElement | null = null;

  const stopElapsedTimer = (): void => {
    if (elapsedTimer === null) return;
    window.clearInterval(elapsedTimer);
    elapsedTimer = null;
  };

  const updateElapsed = (): void => {
    if (!active) return;
    const elapsed = Math.max(0, Math.floor((Date.now() - startedAt) / 1000));
    const time = document.querySelector<HTMLTimeElement>("[data-update-elapsed]");
    if (!time) return;
    const formatted = formatUpdateElapsed(elapsed);
    time.textContent = formatted;
    time.dateTime = `PT${elapsed}S`;
    time.setAttribute("aria-label", `${formatted} elapsed`);
  };

  const restoreInteraction = (): void => {
    document.querySelector<HTMLElement>(".app-shell")?.removeAttribute("inert");
    document.querySelector<HTMLElement>(".app-shell")?.removeAttribute("aria-hidden");
    document.body.classList.remove("is-update-progress");
    const returnFocus = focusReturn;
    focusReturn = null;
    if (returnFocus?.isConnected) returnFocus.focus();
  };

  const start = (): void => {
    if (active) return;
    const modalRoot = document.getElementById("modal-root");
    if (!modalRoot) return;
    const activeElement = document.activeElement;
    focusReturn = activeElement instanceof HTMLElement && activeElement !== document.body ? activeElement : null;
    active = true;
    startedAt = Date.now();
    modalRoot.innerHTML = renderUpdateProgressOverlay();
    document.querySelector<HTMLElement>(".app-shell")?.setAttribute("inert", "");
    document.querySelector<HTMLElement>(".app-shell")?.setAttribute("aria-hidden", "true");
    document.body.classList.add("is-update-progress");
    document.querySelector<HTMLElement>(".update-progress-dialog")?.focus();
    updateElapsed();
    elapsedTimer = window.setInterval(updateElapsed, 1000);
  };

  const update = (payload: unknown): void => {
    if (!active) return;
    const progress = normaliseUpdateProgress(payload);
    const dialog = document.querySelector<HTMLElement>(".update-progress-dialog");
    if (!dialog) return;
    const currentIndex = updateProgressStageIndex(progress.stage);
    const stage = UPDATE_PROGRESS_STAGES[currentIndex]!;
    const message = dialog.querySelector<HTMLElement>("#update-progress-message");
    if (message) message.textContent = progress.message;
    const stepLabel = dialog.querySelector<HTMLElement>("[data-update-step]");
    if (stepLabel) stepLabel.textContent = `Step ${progress.step} of ${progress.total}`;
    const track = dialog.querySelector<HTMLElement>(".update-progress-track");
    if (track) {
      const percent = Math.round((progress.step / progress.total) * 100);
      track.setAttribute("aria-valuemax", String(progress.total));
      track.setAttribute("aria-valuenow", String(progress.step));
      track.setAttribute("aria-valuetext", `Step ${progress.step} of ${progress.total} — ${stage.label}`);
      dialog.querySelector<HTMLElement>("[data-update-progress-fill]")?.style.setProperty("width", `${percent}%`);
    }
    dialog.querySelectorAll<HTMLElement>("[data-update-stage]").forEach((item, index) => {
      const state = index < currentIndex ? "complete" : index === currentIndex ? "current" : "pending";
      const itemStage = UPDATE_PROGRESS_STAGES[index];
      if (!itemStage) return;
      item.dataset.state = state;
      item.toggleAttribute("aria-current", state === "current");
      if (state === "current") item.setAttribute("aria-current", "step");
      item.setAttribute("aria-label", `${itemStage.label}: ${state === "complete" ? "complete" : state === "current" ? "in progress" : "pending"}`);
      const mark = item.querySelector<HTMLElement>(".update-progress-stage-mark");
      if (mark) mark.textContent = state === "complete" ? "✓" : state === "current" ? "" : String(index + 1);
    });
    dialog.dataset.stage = progress.stage;
  };

  const fail = (): void => {
    if (!active) return;
    active = false;
    stopElapsedTimer();
    document.getElementById("modal-root")?.replaceChildren();
    restoreInteraction();
  };

  return { start, update, fail, isActive: () => active };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function positiveInteger(value: unknown, fallback: number, minimum: number): number {
  return typeof value === "number" && Number.isFinite(value) && value >= minimum
    ? Math.max(minimum, Math.floor(value))
    : fallback;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function escapeProgressText(value: string): string {
  return value.replace(/[&<>'"]/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "'": "&#39;",
    '"': "&quot;"
  })[character] ?? character);
}
