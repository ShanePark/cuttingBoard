import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { createContainerConsoleData } from "../src/container-console-data.ts";
import {
  composeDownControl,
  composeDownOutputKind,
  composeDownStateClass,
  composeDownStatusText,
  groupContainers,
  startComposeDownLogPolling,
  STANDALONE_GROUP_NAME,
  type ComposeDownConsoleState
} from "../src/docker-groups.ts";
import type { ContainerInfo, ContainerListing } from "../src/types.ts";

function container(name: string, project: string | null, state = "running"): ContainerInfo {
  return {
    id: name,
    name,
    image: "postgres:16",
    state,
    status: state === "running" ? "Up 5 hours" : "Exited (0) 2 hours ago",
    ports: [],
    port_mappings: [],
    compose_project: project,
    compose_service: project ? name : null,
    compose_working_dir: null
  };
}

function groupNamed(groups: ReturnType<typeof groupContainers>, name: string) {
  const group = groups.find((item) => item.name === name);
  assert.ok(group, `missing group ${name}`);
  return group;
}

test("offers compose down for a compose project with several containers", () => {
  const groups = groupContainers([container("db", "oasis26"), container("cache", "oasis26", "exited")]);
  const control = composeDownControl(groupNamed(groups, "oasis26"), false);

  assert.deepEqual(control, {
    project: "oasis26",
    busy: false,
    disabled: false,
    title: "Stop compose project (docker compose down)",
    ariaLabel: "Stop all containers in oasis26"
  });
});

test("does not offer compose down for standalone containers", () => {
  const groups = groupContainers([container("one", null), container("two", null)]);
  const standalone = groupNamed(groups, STANDALONE_GROUP_NAME);

  assert.equal(standalone.project, null);
  assert.equal(composeDownControl(standalone, false), null);
});

test("keeps a project labelled like the standalone group apart from standalone containers", () => {
  const groups = groupContainers([
    container("labelled-one", STANDALONE_GROUP_NAME),
    container("labelled-two", STANDALONE_GROUP_NAME),
    container("plain-one", null),
    container("plain-two", null)
  ]);

  assert.equal(groups.length, 2);
  assert.deepEqual(groups.map((group) => group.project), [STANDALONE_GROUP_NAME, null]);
  assert.equal(composeDownControl(groups[1]!, false), null);
});

test("does not offer compose down for a single-container compose project", () => {
  const groups = groupContainers([container("mailpit", "kice-mailpit")]);

  assert.equal(composeDownControl(groupNamed(groups, "kice-mailpit"), false), null);
});

test("disables compose down when no container in the project is running", () => {
  const groups = groupContainers([container("db", "oasis26", "exited"), container("cache", "oasis26", "created")]);
  const control = composeDownControl(groupNamed(groups, "oasis26"), false);

  assert.equal(control?.disabled, true);
  assert.equal(control?.title, "No running containers");
});

test("disables compose down while the project is being stopped", () => {
  const groups = groupContainers([container("db", "oasis26"), container("cache", "oasis26")]);
  const control = composeDownControl(groupNamed(groups, "oasis26"), true);

  assert.equal(control?.disabled, true);
  assert.equal(control?.busy, true);
  assert.equal(control?.title, "Stopping compose project");
});

test("renders the compose down control as a group stop button carrying the project", () => {
  const dockerRendering = readFileSync(new URL("../src/docker-rendering.ts", import.meta.url), "utf8");

  assert.match(dockerRendering, /class="section-action icon-only-button stop-group-action" type="button" data-action="compose-down-group" data-compose-project="\$\{h\(control\.project\)\}"/);
  assert.match(dockerRendering, /uiIcon\(control\.busy \? "refresh" : "stop", 15\)/);
});

function composeState(overrides: Partial<ComposeDownConsoleState> = {}): ComposeDownConsoleState {
  return { project: "oasis26", log: "", status: "running", error: null, ...overrides };
}

test("shows a waiting placeholder until docker compose writes output", () => {
  const running = composeState();
  assert.equal(composeDownOutputKind(running), "loading");
  assert.equal(composeDownStatusText(running), "Running docker compose down");
  assert.equal(composeDownStateClass(running), "stopping");
  assert.equal(composeDownOutputKind(composeState({ log: "Container db  Stopping\n" })), "log");
});

test("keeps the output and reports the failure once docker compose down fails", () => {
  const failed = composeState({ log: "Error response from daemon\n", status: "failed", error: "Could not stop compose project oasis26: Error response from daemon" });
  assert.equal(composeDownOutputKind(failed), "log-alert");
  assert.equal(composeDownStatusText(failed), "Failed");
  assert.equal(composeDownStateClass(failed), "failed");
  assert.equal(composeDownOutputKind(composeState({ status: "failed", error: "Actions are disabled in demonstration mode." })), "error");
  assert.equal(composeDownOutputKind(composeState({ status: "succeeded" })), "empty");
  assert.equal(composeDownStatusText(composeState({ status: "succeeded" })), "Completed");
});

function consoleData(activeTab: () => "services" | "docker") {
  const listing: ContainerListing = { available: true, containers: [container("db", "oasis26")], message: null };
  let dockerDomUpdates = 0;
  const data = createContainerConsoleData({
    api: { containerLogs: async () => ({ logs: "" }) },
    getActiveTab: activeTab,
    getContainerListing: () => listing,
    getServicesConsoleTarget: () => null,
    setServicesConsoleTarget: () => undefined,
    captureServicesConsoleState: () => undefined,
    resetContainerSelection: () => undefined,
    invalidateServiceSelection: () => undefined,
    updateDockerConsoleDom: () => { dockerDomUpdates += 1; },
    updateServiceConsoleDom: () => undefined,
    renderDocker: () => undefined,
    renderServices: () => undefined,
    messageOf: String
  });
  return { data, dockerDomUpdates: () => dockerDomUpdates };
}

test("keeps the compose down output after the run until a Docker container is selected", () => {
  let tab: "services" | "docker" = "docker";
  const { data, dockerDomUpdates } = consoleData(() => tab);

  data.beginComposeDown("oasis26");
  data.updateComposeDownLog("oasis26", "Container db  Stopping\n");
  data.updateComposeDownLog("other", "Container other  Stopping\n");
  assert.equal(dockerDomUpdates(), 1);
  data.finishComposeDown("oasis26", true, null);
  assert.deepEqual(data.composeDownState(), { project: "oasis26", log: "Container db  Stopping\n", status: "succeeded", error: null });

  tab = "services";
  data.selectContainer("db");
  assert.equal(data.composeDownState()?.project, "oasis26");

  tab = "docker";
  data.selectContainer("db");
  assert.equal(data.composeDownState(), null);
});

test("a new compose down run replaces the previous one and ignores the old run's output", () => {
  const { data } = consoleData(() => "docker");

  data.beginComposeDown("oasis26");
  data.beginComposeDown("kice-mailpit");
  data.updateComposeDownLog("oasis26", "stale\n");
  data.finishComposeDown("oasis26", true, null);
  assert.deepEqual(data.composeDownState(), { project: "kice-mailpit", log: "", status: "running", error: null });

  data.finishComposeDown("kice-mailpit", false, "Actions are disabled in demonstration mode.");
  assert.equal(data.composeDownState()?.status, "failed");
  assert.equal(data.composeDownState()?.error, "Actions are disabled in demonstration mode.");
});

test("polls compose down output and ends with a final read after the read in flight", async () => {
  let tick: (() => void) | null = null;
  let cleared = false;
  const reads: Array<(log: string) => void> = [];
  const logs: string[] = [];
  const stop = startComposeDownLogPolling(
    () => new Promise<string>((resolve) => { reads.push(resolve); }),
    (log) => logs.push(log),
    {
      setInterval: (callback) => { tick = callback; return 1; },
      clearInterval: () => { cleared = true; }
    }
  );

  assert.equal(reads.length, 0, "the first read waits one interval");
  tick!();
  tick!();
  assert.equal(reads.length, 1, "reads do not overlap");
  const stopped = stop();
  assert.equal(cleared, true);
  reads[0]!("partial\n");
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(reads.length, 2, "the final read starts after the read in flight");
  reads[1]!("partial\ncomplete\n");
  await stopped;
  assert.deepEqual(logs, ["partial\n", "partial\ncomplete\n"]);
});
