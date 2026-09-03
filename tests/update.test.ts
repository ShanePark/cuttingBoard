import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createUpdateController } from "../src/update-controller.ts";
import {
  formatUpdateElapsed,
  normaliseUpdateProgress,
  renderUpdateProgressOverlay,
  updateProgressStageIndex
} from "../src/update-progress.ts";
import type { UpdateCheckResult } from "../src/types.ts";

const apiSource = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const shellSource = readFileSync(new URL("../src/app-shell.ts", import.meta.url), "utf8");
const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const progressSource = readFileSync(new URL("../src/update-progress.ts", import.meta.url), "utf8");

function updateResult(available: boolean): UpdateCheckResult {
  return {
    available,
    current_commit: "current",
    latest_commit: available ? "latest" : "current"
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void } {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

test("the shell keeps the update action hidden beside Settings", () => {
  assert.match(shellSource, /id="update-button"[^>]*data-action="update"[^>]*hidden/);
  assert.match(shellSource, /class="toolbar-actions"[\s\S]*id="update-button"[\s\S]*class="gear-button"/);
});

test("the frontend exposes native update commands and polls every 30 seconds", () => {
  assert.match(apiSource, /invoke<UpdateCheckResult>\("check_for_update"\)/);
  assert.match(apiSource, /invoke<void>\("update_and_restart"\)/);
  assert.match(mainSource, /void updateController\.checkForUpdate\(\)/);
  assert.match(mainSource, /window\.setInterval\(\(\) => void updateController\.checkForUpdate\(\), UPDATE_CHECK_INTERVAL_MS\)/);
  assert.match(mainSource, /appInfo\?\.update_supported && !appInfo\.demo/);
});

test("the update progress surface is an accessible blocking dialog with all four stages", () => {
  const overlay = renderUpdateProgressOverlay();
  assert.match(overlay, /class="update-progress-backdrop"[^>]*role="presentation"/);
  assert.match(overlay, /class="update-progress-dialog"[^>]*role="dialog"[^>]*aria-modal="true"/);
  assert.match(overlay, /id="update-progress-message" role="status" aria-live="polite"/);
  assert.match(overlay, /data-update-build-log aria-hidden="true"/);
  assert.match(overlay, /data-update-stage="validating"/);
  assert.match(overlay, /data-update-stage="building"/);
  assert.match(overlay, /data-update-stage="preparing"/);
  assert.match(overlay, /data-update-stage="restarting"/);
  assert.match(progressSource, /document\.body\.classList\.add\("is-update-progress"\)/);
});

test("progress payloads are normalized to a safe stage, step, and message", () => {
  assert.deepEqual(normaliseUpdateProgress({ stage: "preparing", step: 3, total: 4, message: " Staging… " }), {
    stage: "preparing",
    step: 3,
    total: 4,
    message: "Staging…"
  });
  assert.deepEqual(normaliseUpdateProgress({ stage: "building", step: 2, total: 4, message: "Building", detail: " Compiling cutting-board " }), {
    stage: "building",
    step: 2,
    total: 4,
    message: "Building",
    detail: "Compiling cutting-board"
  });
  assert.deepEqual(normaliseUpdateProgress({ stage: "unknown", step: 99, total: 0, message: "" }), {
    stage: "validating",
    step: 4,
    total: 4,
    message: "Checking the latest local commit"
  });
  assert.equal(updateProgressStageIndex("restarting"), 3);
  assert.equal(formatUpdateElapsed(0), "0s");
  assert.equal(formatUpdateElapsed(65), "1m 5s");
  assert.equal(formatUpdateElapsed(3665), "1h 1m");
});

test("main subscribes to progress events and blocks input while the update surface is active", () => {
  assert.match(mainSource, /listen<UpdateProgressEvent>\("update-progress", \(event\) => updateProgressView\.update\(event\.payload\)\)/);
  assert.match(mainSource, /if \(updateProgressView\.isActive\(\)\) \{[\s\S]*event\.preventDefault\(\);[\s\S]*event\.stopPropagation\(\);/);
  assert.match(mainSource, /showUpdateStarted: \(\) => updateProgressView\.start\(\)/);
  assert.match(mainSource, /updateProgressView\.fail\(\);[\s\S]*toast\(`Update failed:/);
  assert.match(progressSource, /progress\.stage === "building" && progress\.detail !== undefined/);
  assert.match(progressSource, /buildLog\.textContent = progress\.detail/);
  assert.match(progressSource, /progress\.stage !== "building"/);
  assert.match(progressSource, /message\.textContent !== progress\.message/);
});

test("bootstrap waits for progress listener setup without making listener errors fatal", () => {
  assert.match(mainSource, /const updateProgressListenerReady = listen<UpdateProgressEvent>\([\s\S]*\.catch\(\(\) => undefined\)/);
  assert.match(mainSource, /await updateProgressListenerReady;/);
  assert.ok(mainSource.indexOf("await updateProgressListenerReady;") < mainSource.indexOf("installTimers();"));
});

test("an update announces progress before starting the native build", async () => {
  const pending = deferred<void>();
  const sequence: string[] = [];
  const controller = createUpdateController(
    {
      checkForUpdate: async () => updateResult(true),
      updateAndRestart: () => {
        sequence.push("native");
        return pending.promise;
      }
    },
    {
      setUpdateAvailable: () => {},
      setUpdateBusy: (busy) => { if (busy) sequence.push("busy"); },
      showUpdateStarted: () => sequence.push("progress"),
      showError: () => {}
    }
  );

  await controller.checkForUpdate();
  const update = controller.updateAndRestart();
  assert.deepEqual(sequence, ["busy", "progress", "native"]);
  pending.resolve();
  await update;
});

test("update checks do not overlap and expose a matching commit as available", async () => {
  const pending = deferred<UpdateCheckResult>();
  let checks = 0;
  const availableStates: boolean[] = [];
  const controller = createUpdateController(
    {
      checkForUpdate: () => {
        checks += 1;
        return pending.promise;
      },
      updateAndRestart: async () => {}
    },
    {
      setUpdateAvailable: (available) => availableStates.push(available),
      setUpdateBusy: () => {},
      showUpdateStarted: () => {},
      showError: () => {}
    }
  );

  const firstCheck = controller.checkForUpdate();
  const secondCheck = controller.checkForUpdate();
  assert.equal(checks, 1);
  pending.resolve(updateResult(true));
  await Promise.all([firstCheck, secondCheck]);
  assert.deepEqual(availableStates, [true]);
});

test("a check failure hides the update control without reporting an app error", async () => {
  const errors: string[] = [];
  const availableStates: boolean[] = [];
  const controller = createUpdateController(
    {
      checkForUpdate: async () => {
        throw new Error("git unavailable");
      },
      updateAndRestart: async () => {}
    },
    {
      setUpdateAvailable: (available) => availableStates.push(available),
      setUpdateBusy: () => {},
      showUpdateStarted: () => {},
      showError: (message) => errors.push(message)
    }
  );

  await controller.checkForUpdate();
  assert.deepEqual(availableStates, [false]);
  assert.deepEqual(errors, []);
});

test("an update failure restores the control and reports the native error", async () => {
  const pending = deferred<void>();
  const busyStates: boolean[] = [];
  const errors: string[] = [];
  const controller = createUpdateController(
    {
      checkForUpdate: async () => updateResult(true),
      updateAndRestart: () => pending.promise
    },
    {
      setUpdateAvailable: () => {},
      setUpdateBusy: (busy) => busyStates.push(busy),
      showUpdateStarted: () => {},
      showError: (message) => errors.push(message)
    }
  );

  await controller.checkForUpdate();
  const update = controller.updateAndRestart();
  pending.reject(new Error("release build failed"));
  await update;
  assert.deepEqual(busyStates, [true, false]);
  assert.deepEqual(errors, ["release build failed"]);
});

test("a poll started during an update is ignored", async () => {
  const updatePending = deferred<void>();
  let checks = 0;
  const controller = createUpdateController(
    {
      checkForUpdate: async () => {
        checks += 1;
        return updateResult(true);
      },
      updateAndRestart: () => updatePending.promise
    },
    {
      setUpdateAvailable: () => {},
      setUpdateBusy: () => {},
      showUpdateStarted: () => {},
      showError: () => {}
    }
  );

  await controller.checkForUpdate();
  const update = controller.updateAndRestart();
  await controller.checkForUpdate();
  assert.equal(checks, 1);
  updatePending.resolve();
  await update;
});

test("a stale check cannot hide availability while a failed update restores the button", async () => {
  const staleCheck = deferred<UpdateCheckResult>();
  const updatePending = deferred<void>();
  const availableStates: boolean[] = [];
  const busyStates: boolean[] = [];
  const errors: string[] = [];
  let checks = 0;
  const controller = createUpdateController(
    {
      checkForUpdate: () => {
        checks += 1;
        return checks === 1 ? Promise.resolve(updateResult(true)) : staleCheck.promise;
      },
      updateAndRestart: () => updatePending.promise
    },
    {
      setUpdateAvailable: (available) => availableStates.push(available),
      setUpdateBusy: (busy) => busyStates.push(busy),
      showUpdateStarted: () => {},
      showError: (message) => errors.push(message)
    }
  );

  await controller.checkForUpdate();
  const stale = controller.checkForUpdate();
  const update = controller.updateAndRestart();
  staleCheck.reject(new Error("stale git check"));
  await stale;
  updatePending.reject(new Error("release build failed"));
  await update;

  assert.deepEqual(availableStates, [true, true]);
  assert.deepEqual(busyStates, [true, false]);
  assert.deepEqual(errors, ["release build failed"]);
});
