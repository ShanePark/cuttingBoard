import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const rendering = readFileSync(new URL("../src/launch-rendering.ts", import.meta.url), "utf8");
const actions = readFileSync(new URL("../src/launch-actions.ts", import.meta.url), "utf8");
const api = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("running launch task cards place restart immediately before stop", () => {
  const restart = rendering.indexOf('data-action="restart-task"');
  const stop = rendering.indexOf('data-action="${externalCanStop ? "stop-service" : "stop-task"}"');

  assert.notEqual(restart, -1);
  assert.notEqual(stop, -1);
  assert.ok(restart < stop);
  assert.match(rendering, /const restartAction = canStop\s*\n\s*\? /);
  assert.match(rendering, /Boolean\(snapshot\?\.external_pid\)/);
});

test("launch restart routes through confirmation and the atomic task API", () => {
  assert.match(main, /action === "restart-task"\) launchActions\.requestLaunchAction\(\{ kind: "task", direction: "restart"/);
  assert.match(actions, /direction: "start" \| "stop" \| "restart"/);
  assert.match(actions, /direction === "restart"\s*\n\s*\? await context\.api\.restartTask\(profileId, taskName\)/);
  assert.match(api, /invoke<ManagedTaskSnapshot>\("restart_task", \{ request: \{ profile_id: profileId, task_name: taskName \} \}\)/);
});

test("launch start and restart reveal the console and stream logs before completion", () => {
  assert.match(actions, /import \{ startRestartProgressPolling \} from "\.\/restart-progress"/);
  assert.match(actions, /direction !== "stop" && context\.api\.taskLogTail && context\.updateTaskLogTail/);
  assert.match(actions, /startRestartProgressPolling\(/);
  assert.match(actions, /if \(context\.focusTaskConsole\) context\.focusTaskConsole\(profileId, taskName\)/);
  assert.match(main, /taskLogTail: api\.taskLogTail/);
  assert.match(main, /function focusTaskConsole\(profileId: string, taskName: string\)/);
  assert.match(main, /function updateTaskLogTail\(profileId: string, taskName: string, logTail: string\)/);
});

test("restart icon combines refresh and play affordances", () => {
  const icons = readFileSync(new URL("../src/icons.ts", import.meta.url), "utf8");
  assert.match(icons, /export function restartIcon\(/);
  assert.match(icons, /uiIcon\("refresh", edge\)/);
  assert.match(icons, /uiIcon\("play", playSize\)/);
});
