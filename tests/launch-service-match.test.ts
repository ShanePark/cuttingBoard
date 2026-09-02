import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { launchTaskRoot, matchedServiceForTask } from "../src/presentation-services.ts";
import type { LaunchProfile, LaunchTask, Relevance, ServiceSnapshot } from "../src/types.ts";

const profile: LaunchProfile = { id: "kice", name: "kice", project_root: "/home/dev/kice", tasks: [] };

function task(cwd: string, port: number | null, container: string | null = null): LaunchTask {
  return { name: "serv", cwd, command: "java -jar serv.jar", expected_port: port, container };
}

type ServiceOptions = {
  workingDirectory?: string | null;
  project?: { root: string; workspace?: string } | null;
  relevance?: Relevance;
};

function service(id: string, port: number, options: ServiceOptions = {}): ServiceSnapshot {
  const project = options.project ?? null;
  return {
    id,
    display_name: id,
    tech: "spring",
    category: "runtime",
    relevance: options.relevance ?? "dev",
    endpoints: [{ family: "ipv4", address: "0.0.0.0", port, scope: "all", protocol: "TCP" }],
    process: options.workingDirectory === undefined ? null : {
      pid: 1,
      parent_pid: null,
      name: "java",
      executable: null,
      working_directory: options.workingDirectory,
      command: "java -jar serv.jar",
      create_time: 0,
      uptime_seconds: 0,
      cpu_percent: null,
      memory_bytes: null,
      uid: null
    },
    project: project ? {
      id: `${id}-project`,
      name: "kice",
      root_path: project.root,
      workspace_root_path: project.workspace ?? project.root,
      workspace_name: "kice",
      detection_source: "maven"
    } : null,
    status: "healthy",
    warnings: [],
    origin_kind: "terminal",
    origin_label: null,
    can_terminate: true,
    browser_url: null,
    active_profiles: []
  };
}

test("a service listening on the task's port from the task's directory backs the task", () => {
  const serv = service("serv", 8060, { workingDirectory: "/home/dev/kice/serv" });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [serv]), serv);
});

test("a service that merely shares the port does not back the task", () => {
  const stranger = service("stranger", 8060, { workingDirectory: "/home/dev/other", project: { root: "/home/dev/other" } });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [stranger]), null);
});

test("a service started from elsewhere is matched through the project it was scanned under", () => {
  const fromIde = service("serv", 8060, { workingDirectory: "/", project: { root: "/home/dev/kice/serv", workspace: "/home/dev/kice" } });
  const siblingModule = service("user", 8060, { workingDirectory: "/home/dev/kice/user", project: { root: "/home/dev/kice/user", workspace: "/home/dev/kice" } });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [fromIde]), fromIde);
  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [siblingModule]), siblingModule);
});

test("a relative task directory is anchored to the profile's project", () => {
  const serv = service("serv", 8060, { workingDirectory: "/home/dev/kice/serv" });

  assert.equal(launchTaskRoot(profile, task("serv", 8060)), "/home/dev/kice/serv");
  assert.equal(launchTaskRoot(profile, task("C:\\dev\\kice\\serv", 8060)), "C:\\dev\\kice\\serv");
  assert.equal(matchedServiceForTask(profile, task("serv", 8060), [serv]), serv);
  assert.equal(matchedServiceForTask(profile, task(".", 8060), [serv]), serv);
});

test("a task without an expected port has no backing service", () => {
  const serv = service("serv", 8060, { workingDirectory: "/home/dev/kice/serv" });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", null), [serv]), null);
});

test("a container task is answered by Docker, not by a process on its port", () => {
  const proxy = service("proxy", 45432, { workingDirectory: "/home/dev/kice" });

  assert.equal(matchedServiceForTask(profile, task(".", 45432, "kice-dev-postgres"), [proxy]), null);
});

test("only the dev services the board shows can back a task", () => {
  const noise = service("noise", 8060, { workingDirectory: "/home/dev/kice/serv", relevance: "noise" });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [noise]), null);
});

test("the service running from the task's own directory wins over one matched by project", () => {
  const byProject = service("a-by-project", 8060, { workingDirectory: "/tmp", project: { root: "/home/dev/kice" } });
  const byDirectory = service("b-by-directory", 8060, { workingDirectory: "/home/dev/kice/serv" });

  assert.equal(matchedServiceForTask(profile, task("/home/dev/kice/serv", 8060), [byProject, byDirectory]), byDirectory);
});

test("the task card and the task details take the backing service from the shared rule", () => {
  for (const file of ["src/launch-rendering.ts", "src/modal-forms.ts"]) {
    const source = readFileSync(new URL(`../${file}`, import.meta.url), "utf8");
    assert.match(source, /matchedServiceForTask[\s\S]*?\} from "\.\/presentation";|import \{ matchedServiceForTask \} from "\.\/presentation";/, file);
    assert.doesNotMatch(source, /function matchedServiceForTask/, file);
  }
});
