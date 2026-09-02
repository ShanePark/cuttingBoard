import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { launchProfileIsActive, orderLaunchProfiles } from "../src/launch-state.ts";
import type { LaunchProfile, LaunchState, ManagedTaskSnapshot } from "../src/types.ts";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

function profile(id: string, taskCount = 1): LaunchProfile {
  return {
    id,
    name: id,
    project_root: ".",
    tasks: Array.from({ length: taskCount }, (_, index) => ({
      name: `task-${index}`,
      cwd: ".",
      command: "run",
      expected_port: null
    }))
  };
}

function snapshot(profileId: string, taskName: string, state: LaunchState): ManagedTaskSnapshot {
  return {
    profile_id: profileId,
    task_name: taskName,
    state,
    main_pid: null,
    started_at: null,
    message: null,
    log_tail: ""
  };
}

test("active launch states identify a profile while an empty profile stays inactive", () => {
  assert.equal(launchProfileIsActive([]), false);
  assert.equal(launchProfileIsActive(["stopped", "failed"]), false);
  for (const state of ["running", "starting", "stopping", "external"] as const) {
    assert.equal(launchProfileIsActive(["stopped", state]), true, state);
  }
});

test("launch profiles render active groups first without changing either partition order", () => {
  const profiles = [profile("stopped"), profile("empty", 0), profile("starting"), profile("failed"), profile("external"), profile("running")];
  const snapshots = new Map<string, LaunchState>([
    ["stopped", "stopped"],
    ["starting", "starting"],
    ["failed", "failed"],
    ["external", "external"],
    ["running", "running"]
  ]);
  const snapshotFor = (profileId: string, taskName: string): ManagedTaskSnapshot | undefined => {
    const state = snapshots.get(profileId);
    return state ? snapshot(profileId, taskName, state) : undefined;
  };
  const ordered = orderLaunchProfiles(profiles, snapshotFor);

  assert.deepEqual(ordered.map(({ id }) => id), ["starting", "external", "running", "stopped", "empty", "failed"]);
  assert.deepEqual(profiles.map(({ id }) => id), ["stopped", "empty", "starting", "failed", "external", "running"]);
  assert.notStrictEqual(ordered, profiles);
});

test("the Launch view keeps the Add profile card after the ordered groups", () => {
  assert.match(main, /const orderedProfiles = orderLaunchProfiles\(profiles, snapshotFor\)/);
  assert.match(main, /\$\{orderedProfiles\.map\(\(profile\) => renderProfile\(profile, selectedRenderingContext\)\)\.join\(""\)\}\$\{renderLaunchAddCard/);
});
