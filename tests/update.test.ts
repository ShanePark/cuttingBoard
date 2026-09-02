import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createUpdateController } from "../src/update-controller.ts";
import type { UpdateCheckResult } from "../src/types.ts";

const apiSource = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const shellSource = readFileSync(new URL("../src/app-shell.ts", import.meta.url), "utf8");
const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

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
