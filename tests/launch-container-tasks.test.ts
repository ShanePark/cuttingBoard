import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { launchTasksEquivalent } from "../src/presentation-launch.ts";
import { containerLaunchTasks } from "../src/presentation-services.ts";
import type { ContainerInfo } from "../src/types.ts";

function container(name: string, workingDir: string | null): ContainerInfo {
  return {
    id: "b".repeat(64),
    name,
    image: "postgres:16",
    state: "running",
    status: "Up 5 hours (healthy)",
    ports: [45432],
    compose_project: "oasis26",
    compose_service: "postgres",
    compose_working_dir: workingDir
  };
}

test("saving a services group carries its Docker containers into the profile", () => {
  const tasks = containerLaunchTasks([container("oasis-dev-postgres", "/home/dev/oasis26")], ["backend"]);

  assert.deepEqual(tasks, [{
    name: "oasis-dev-postgres",
    cwd: ".",
    command: "",
    expected_port: 45432,
    container: "oasis-dev-postgres"
  }]);
});

test("a container task never shadows the name of a service task", () => {
  const tasks = containerLaunchTasks([container("backend", "/home/dev/oasis26")], ["Backend"]);

  assert.equal(tasks[0]!.name, "backend 2");
  assert.equal(tasks[0]!.container, "backend");
});

test("the services board hands its group containers to the generated profile", () => {
  const rendering = readFileSync(new URL("../src/services-rendering.ts", import.meta.url), "utf8");

  assert.match(rendering, /containerLaunchTasks\(group\.containers, services\.map\(\(task\) => task\.name\)\)/);
});

test("a container task is a different task from a command task of the same name", () => {
  const command = [{ name: "db", cwd: ".", command: "", expected_port: 5432, container: null }];
  const containerTask = [{ name: "db", cwd: ".", command: "", expected_port: 5432, container: "app-db" }];

  assert.equal(launchTasksEquivalent(command, containerTask), false);
  assert.equal(launchTasksEquivalent(containerTask, [...containerTask]), true);
});

test("container tasks are started and stopped through Docker, never as a shell command", () => {
  const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
  const external = readFileSync(new URL("../src-tauri/src/launch/external.rs", import.meta.url), "utf8");
  const forms = readFileSync(new URL("../src/modal-forms.ts", import.meta.url), "utf8");

  for (const action of ["start", "stop", "restart"]) {
    assert.match(lib, new RegExp(`launch_containers::${action}\\(&request, &container, state\\.0\\.demo\\)`));
  }
  assert.match(lib, /launch_containers::stop_profile\(&profiles, &profile_id, state\.0\.demo\)/);
  assert.match(external, /task\.container_name\(\)\.is_some\(\)/);
  // The editor keeps the container binding of a task it did not create.
  assert.match(forms, /data-task-field="container"/);
  assert.match(forms, /\(!task\.command && !task\.container\)/);
});

test("only Docker reports the state of a container task", () => {
  const manager = readFileSync(new URL("../src-tauri/src/launch.rs", import.meta.url), "utf8");

  // A stopped snapshot from the manager would win the lookup and hide the running container.
  assert.match(manager, /if task\.container_name\(\)\.is_some\(\) \{\n\s+continue;/);
});

test("a launch profile can be deleted while its tasks are running", () => {
  const actions = readFileSync(new URL("../src/launch-actions.ts", import.meta.url), "utf8");
  const forms = readFileSync(new URL("../src/modal-forms.ts", import.meta.url), "utf8");
  const lib = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");

  assert.doesNotMatch(actions, /before deleting it/);
  assert.match(actions, /Tasks Cutting Board started for it are stopped first/);
  assert.match(forms, /title="Delete profile" \$\{appInfo\?\.demo \? "disabled" : ""\}/);
  assert.match(lib, /lock\(&state\.0\.launch\)\?\.discard_profile\(&profile_id\)/);
});
