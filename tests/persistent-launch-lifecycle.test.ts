import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const nativeSource = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("normal app lifecycle handlers leave managed tasks running", () => {
  const windowEvents = nativeSource.slice(
    nativeSource.indexOf(".on_window_event"),
    nativeSource.indexOf(".invoke_handler")
  );
  const runEvents = nativeSource.slice(nativeSource.indexOf(".run(|app, event|"));

  assert.doesNotMatch(windowEvents, /stop_managed_tasks|\.stop_all\(\)/);
  assert.doesNotMatch(runEvents, /stop_managed_tasks|\.stop_all\(\)/);
});

test("the explicit shutdown command still stops managed tasks", () => {
  const shutdown = nativeSource.slice(
    nativeSource.indexOf("fn shutdown"),
    nativeSource.indexOf("fn terminate_discovered_service")
  );

  assert.match(shutdown, /lock\(&state\.0\.launch\)\?\.stop_all\(\)/);
});

test("startup scans the workspace before loading task snapshots", () => {
  const bootstrap = mainSource.slice(
    mainSource.indexOf("async function bootstrap"),
    mainSource.indexOf("function installTimers")
  );
  const scan = bootstrap.indexOf("await refreshWorkspace(true)");
  const snapshots = bootstrap.indexOf("taskSnapshots = await api.taskSnapshots()");

  assert.notEqual(scan, -1);
  assert.notEqual(snapshots, -1);
  assert.ok(scan < snapshots);
});
