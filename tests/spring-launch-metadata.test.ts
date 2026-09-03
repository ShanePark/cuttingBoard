import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { register } from "node:module";
import test from "node:test";
import {
  initialRestartProgress,
  progressFromTaskLog,
  restartProgressBusyForService,
  remapRestartProgress,
  shouldClearCompletedRestartProgress,
  startRestartProgressPolling
} from "../src/restart-progress.ts";
import { springPrepareForService } from "../src/spring-prepare.ts";
import type { LaunchProfile, ServiceSnapshot } from "../src/types.ts";

// Source files use extensionless imports for Vite. Register a test-only resolver so the pure
// restart/metadata modules can be imported and exercised directly by Node's built-in test runner.
const extensionlessResolver = `export async function resolve(specifier, context, nextResolve) {
  if ((specifier.startsWith("./") || specifier.startsWith("../")) && !/[.]\\w+$/.test(specifier)) {
    try { return await nextResolve(specifier + ".ts", context); } catch {}
  }
  return nextResolve(specifier, context);
}`;
register(`data:text/javascript,${encodeURIComponent(extensionlessResolver)}`, import.meta.url);
const { restartServiceWithPreparation, savedSpringTaskForService } = await import("../src/service-restart.ts");

const servicesRendering = readFileSync(new URL("../src/services-rendering.ts", import.meta.url), "utf8");
const serviceActions = readFileSync(new URL("../src/service-actions.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise; });
  return { promise, resolve };
}

function springService(overrides: Partial<ServiceSnapshot> = {}): ServiceSnapshot {
  return {
    id: "adm-service",
    display_name: "kice · Spring Boot",
    tech: "spring",
    category: "api",
    relevance: "dev",
    endpoints: [{ family: "ipv4", address: "127.0.0.1", port: 8060, scope: "loopback", protocol: "TCP" }],
    process: {
      pid: 1234,
      parent_pid: null,
      name: "java",
      executable: "/usr/lib/jvm/bin/java",
      working_directory: "/home/dev/kice/adm",
      command: "/usr/lib/jvm/bin/java -Dspring.profiles.active=dev,test -classpath /home/dev/kice/adm/target/classes kr.re.kisti.idr.AdminApplication --server.port=8060",
      launch_command: null,
      create_time: 1,
      uptime_seconds: 60,
      cpu_percent: null,
      memory_bytes: null,
      uid: 1000
    },
    project: {
      id: "kice-adm",
      name: "adm",
      root_path: "/home/dev/kice/adm",
      workspace_root_path: "/home/dev/kice",
      workspace_name: "kice",
      detection_source: "pom.xml"
    },
    status: "healthy",
    warnings: [],
    origin_kind: "terminal",
    origin_label: null,
    can_terminate: true,
    browser_url: null,
    active_profiles: [],
    ...overrides
  };
}

function profileFor(service: ServiceSnapshot, taskOverrides: Partial<LaunchProfile["tasks"][number]> = {}): LaunchProfile {
  return {
    id: "kice",
    name: "kice",
    project_root: "/home/dev/kice",
    tasks: [{
      name: "adm",
      cwd: "/home/dev/kice/adm",
      command: service.process?.launch_command ?? service.process?.command ?? "java AdminApplication",
      expected_port: 8060,
      container: null,
      ...taskOverrides
    }]
  };
}

test("generated Maven Spring metadata serializes module, profiles, and main class", () => {
  const service = springService();
  const prepare = springPrepareForService(service, "/home/dev/kice");

  assert.deepEqual(JSON.parse(JSON.stringify(prepare)), {
    kind: "spring_boot",
    build_tool: "maven",
    module: "adm",
    profiles: ["dev", "test"],
    main_class: "kr.re.kisti.idr.AdminApplication"
  });
  assert.match(servicesRendering, /if \(isSpringService\(service\)\) task\.prepare = springPrepareForService\(service, group\.path, command\)/);
});

test("generated Gradle Spring metadata keeps the backend-detectable shape", () => {
  const service = springService({
    project: {
      ...springService().project!,
      detection_source: "build.gradle.kts"
    },
    active_profiles: ["local", "dev"]
  });
  const prepare = springPrepareForService(service, "/home/dev/kice");

  assert.deepEqual(JSON.parse(JSON.stringify(prepare)), {
    kind: "spring_boot",
    build_tool: "gradle",
    module: "adm",
    profiles: ["local", "dev"],
    main_class: "kr.re.kisti.idr.AdminApplication"
  });
});

test("Spring metadata falls back to null build tool when filesystem evidence is unavailable", () => {
  const service = springService({
    project: null,
    active_profiles: [],
    process: {
      ...springService().process!,
      command: "java -Dspring.profiles.active=dev -classpath target/classes kr.re.kisti.idr.AdminApplication"
    }
  });

  assert.deepEqual(springPrepareForService(service, null), {
    kind: "spring_boot",
    build_tool: null,
    module: null,
    profiles: ["dev"],
    main_class: "kr.re.kisti.idr.AdminApplication"
  });
});

test("legacy Spring task without prepare invokes restartTask instead of restartService", async () => {
  const service = springService();
  const profile = profileFor(service);
  const calls: string[] = [];

  const result = await restartServiceWithPreparation(service, [profile], {
    restartTask: async (profileId, taskName) => {
      calls.push(`task:${profileId}:${taskName}`);
      return {
        profile_id: profileId,
        task_name: taskName,
        state: "running",
        main_pid: 2,
        started_at: 1,
        message: null,
        log_tail: ""
      };
    },
    restartService: async (serviceId) => {
      calls.push(`service:${serviceId}`);
    }
  });

  assert.equal(result?.state, "running");
  assert.deepEqual(calls, ["task:kice:adm"]);
  assert.deepEqual(savedSpringTaskForService(service, [profile]), { profile, task: profile.tasks[0] });
});

test("unmatched or non-Spring services retain raw restart behavior", async () => {
  const service = springService();
  const unrelated = profileFor(service, { expected_port: 8061 });
  const calls: string[] = [];
  await restartServiceWithPreparation(service, [unrelated], {
    restartTask: async () => {
      calls.push("task");
      throw new Error("unexpected prepared restart");
    },
    restartService: async () => {
      calls.push("service");
    }
  });
  assert.deepEqual(calls, ["service"]);

  const nonSpring = springService({ tech: "nodejs" });
  const legacySpringShape = profileFor(nonSpring);
  await restartServiceWithPreparation(nonSpring, [legacySpringShape], {
    restartTask: async () => {
      calls.push("task");
      throw new Error("unexpected non-Spring task restart");
    },
    restartService: async () => {
      calls.push("service");
    }
  });
  assert.deepEqual(calls, ["service", "service"]);
});

test("service actions use the extracted prepared restart selector and API wiring", () => {
  assert.match(serviceActions, /restartServiceWithPreparation\(service, context\.getProfiles\(\), context\.api\)/);
  assert.match(main, /restartTask: api\.restartTask/);
});

test("service restart wires a lock-free task log reader and focuses the console", () => {
  assert.match(apiSource, /taskLogTail: \(profileId: string, taskName: string\) =>[\s\S]*invoke<string>\("task_log_tail"/);
  assert.match(main, /focusServiceConsole: \(serviceId\) => focusServiceConsole\(serviceId\)/);
  assert.match(main, /updateRestartProgress: updateServiceRestartProgress/);
  assert.match(serviceActions, /startRestartProgressPolling\(/);
});

test("service console renders restart progress while the replacement scan is temporarily empty", () => {
  // services-rendering imports browser-only icon assets, so keep this as a focused source
  // assertion rather than requiring a DOM shim in the pure metadata test suite.
  assert.match(servicesRendering, /renderServiceLogOutput\(service: ServiceSnapshot \| null/);
  assert.match(servicesRendering, /context\.restartProgress && \(!service \|\| context\.restartProgress\.serviceId === service\.id\)/);
  assert.match(servicesRendering, /if \(restartProgress\) \{[\s\S]*console-progress[\s\S]*console-log/);
  assert.match(main, /if \(!service\) \{\s*if \(serviceRestartProgress\?\.serviceId === selectedServiceId\) return;/);
  assert.match(main, /remapServiceSelectionForRestart\(\);\s*syncSelectedService\(\)/);
});

test("task log polling exposes an intermediate prepare line before restart completion", async () => {
  const restart = deferred<void>();
  let restartCompleted = false;
  const restartPromise = restart.promise.then(() => { restartCompleted = true; });
  const observed: string[] = [];
  let nextPoll: (() => void) | null = null;
  let stopped = false;
  let reads = 0;
  const stop = startRestartProgressPolling(
    async () => {
      reads += 1;
      if (reads === 1) return "=== Cutting Board new process started · task=adm · pid=10 ===\n";
      if (reads === 2) return "=== Cutting Board restart requested · task=adm ===\n=== Cutting Board prepare detection started · task=adm ===\n";
      await restart.promise;
      return "=== Cutting Board new process started · task=adm · pid=42 ===\n";
    },
    (logTail) => observed.push(logTail),
    {
      setInterval: (callback) => {
        nextPoll = callback;
        return 1;
      },
      clearInterval: () => { stopped = true; }
    }
  );

  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(restartCompleted, false);
  assert.equal(observed.length, 0, "the previous append-only log is used only as the baseline");

  nextPoll!();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(restartCompleted, false);
  assert.match(observed.at(-1)!, /prepare detection started/);

  nextPoll!();
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  restart.resolve();
  await restartPromise;
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
  assert.equal(restartCompleted, true);
  assert.match(observed.at(-1)!, /new process started/);
  stop();
  assert.equal(stopped, true);
});

test("progress phase ignores a previous completed restart section", () => {
  const initial = initialRestartProgress("service", "kice", "adm", 1);
  const log = [
    "=== Cutting Board restart requested · task=adm ===",
    "=== Cutting Board new process started · task=adm · pid=10 ===",
    "=== Cutting Board restart requested · task=adm ===",
    "=== Cutting Board prepare detection started · task=adm · cwd=/tmp/kice/adm ===",
    "=== Cutting Board prepare command · mvnw install ==="
  ].join("\n");

  const current = progressFromTaskLog(initial, log);
  assert.equal(current.phase, "building");
  assert.notEqual(current.phase, "completed");
  assert.match(current.logTail, /prepare command/);
});

test("completed progress stays pending until a delayed replacement service is remapped", () => {
  const initial = initialRestartProgress("old-service", "kice", "adm", 1);
  const completed = progressFromTaskLog(initial, [
    "=== Cutting Board restart requested · task=adm ===",
    "=== Cutting Board prepare completed · tool=maven · module=adm ===",
    "=== Cutting Board previous process stopped · task=adm ===",
    "=== Cutting Board starting new process · task=adm ===",
    "=== Cutting Board new process started · task=adm · pid=42 ==="
  ].join("\n"));

  assert.equal(completed.phase, "completed");
  assert.equal(completed.remapped, false, "a completed restart may still be waiting for its port to bind");
  assert.equal(shouldClearCompletedRestartProgress(completed), false);
  const remapped = remapRestartProgress(completed, "new-service");
  assert.equal(remapped.serviceId, "new-service");
  assert.equal(remapped.remapped, true);
  assert.equal(shouldClearCompletedRestartProgress(remapped), true);
  const afterPortBind = progressFromTaskLog(remapped, [
    "=== Cutting Board restart requested · task=adm ===",
    "=== Cutting Board new process started · task=adm · pid=42 ==="
  ].join("\n"));
  assert.equal(afterPortBind.serviceId, "new-service", "later log polls keep targeting the remapped service");
  assert.equal(afterPortBind.phase, "completed");
});

test("a PID-replaced service stays busy until its restart settles", () => {
  const initial = initialRestartProgress("old-service", "kice", "adm", 1);
  assert.equal(restartProgressBusyForService(initial, "old-service", true), true);

  const replacement = remapRestartProgress(initial, "new-service");
  assert.equal(restartProgressBusyForService(replacement, "new-service", false), true);
  assert.equal(restartProgressBusyForService(replacement, "old-service", true), false);

  const failed = { ...replacement, phase: "failed" as const };
  assert.equal(restartProgressBusyForService(failed, "new-service", true), false);
});
