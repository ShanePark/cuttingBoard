import assert from "node:assert/strict";
import { register } from "node:module";
import test from "node:test";
import type {
  ContainerInfo,
  ContainerListing,
  ContainerLogSnapshot,
  ServiceLogSnapshot,
  ServiceSnapshot,
  WorkspaceSnapshot
} from "../src/types.ts";

// The Vite source uses extensionless imports. Keep these controller tests on Node's built-in
// runner without changing the application imports just for test execution.
const extensionlessResolver = `export async function resolve(specifier, context, nextResolve) {
  if ((specifier.startsWith("./") || specifier.startsWith("../")) && !/[.]\\w+$/.test(specifier)) {
    try { return await nextResolve(specifier + ".ts", context); } catch {}
  }
  return nextResolve(specifier, context);
}`;
register(`data:text/javascript,${encodeURIComponent(extensionlessResolver)}`, import.meta.url);

const { createContainerConsoleData } = await import("../src/container-console-data.ts");
const { createServiceConsoleData } = await import("../src/service-console-data.ts");

type Deferred<T> = {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
};

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

function service(id: string): ServiceSnapshot {
  return {
    id,
    display_name: id,
    tech: "node",
    category: "api",
    relevance: "dev",
    endpoints: [],
    process: null,
    project: null,
    status: "healthy",
    warnings: [],
    origin_kind: "terminal",
    origin_label: null,
    can_terminate: true,
    browser_url: null,
    active_profiles: []
  };
}

function workspace(services: ServiceSnapshot[]): WorkspaceSnapshot {
  return {
    services,
    scanned_at: 0,
    scan_duration_ms: 0,
    endpoint_count: 0,
    errors: []
  };
}

function container(id: string): ContainerInfo {
  return {
    id,
    name: id,
    image: "postgres:16",
    state: "running",
    status: "Up",
    ports: [],
    compose_project: null,
    compose_service: null,
    compose_working_dir: null
  };
}

function listing(containers: ContainerInfo[]): ContainerListing {
  return { available: true, containers, message: null };
}

function workspaceElement(): HTMLElement {
  return { querySelectorAll: () => [] } as unknown as HTMLElement;
}

function serviceLog(logs: string): ServiceLogSnapshot {
  return { logs, source_path: null, available: true, message: null };
}

function containerLog(logs: string): ContainerLogSnapshot {
  return { logs };
}

test("service logs ignore a late response from a previous selection", async () => {
  const first = deferred<ServiceLogSnapshot>();
  const second = deferred<ServiceLogSnapshot>();
  const requests = new Map<string, Deferred<ServiceLogSnapshot>>([
    ["first", first],
    ["second", second]
  ]);
  let currentWorkspace = workspace([service("first"), service("second")]);
  let selection = { serviceId: null as string | null, consoleTarget: null as { kind: "service"; id: string } | null };
  let nextTimer = 0;
  const controller = createServiceConsoleData({
    api: { serviceLogs: (serviceId) => requests.get(serviceId)!.promise },
    getActiveTab: () => "services",
    getWorkspace: () => currentWorkspace,
    getProfiles: () => [],
    getSelection: () => selection,
    setSelection: (next) => { selection = next as typeof selection; },
    resetServiceSelection: () => undefined,
    renderServices: () => undefined,
    updateServiceConsoleDom: () => undefined,
    messageOf: String,
    operations: new Set<string>(),
    workspaceElement: workspaceElement(),
    scheduler: {
      setInterval: () => ++nextTimer,
      clearInterval: () => undefined
    }
  });

  controller.selectService("first");
  controller.selectService("second");
  second.resolve(serviceLog("second logs"));
  await settle();
  assert.equal(controller.logState().serviceId, "second");
  assert.equal(controller.logState().logs, "second logs");

  first.resolve(serviceLog("stale first logs"));
  await settle();
  assert.equal(controller.logState().serviceId, "second");
  assert.equal(controller.logState().logs, "second logs");
  assert.equal(selection.serviceId, "second");
  assert.ok(nextTimer > 0);
  currentWorkspace = workspace([service("second")]);
});

test("container console state and responses stay isolated per tab", async () => {
  const serviceContainer = deferred<ContainerLogSnapshot>();
  const dockerContainer = deferred<ContainerLogSnapshot>();
  const latestDocker = deferred<ContainerLogSnapshot>();
  const staleDocker = deferred<ContainerLogSnapshot>();
  let dockerRequestCount = 0;
  let activeTab: "services" | "docker" = "services";
  let currentListing = listing([container("service-container"), container("docker-container"), container("stale-docker")]);
  let serviceTarget: { kind: "container"; id: string } | null = null;
  let invalidatedServiceSelection = 0;
  const controller = createContainerConsoleData({
    api: {
      containerLogs: (containerId) => {
        if (containerId === "service-container") return serviceContainer.promise;
        if (containerId === "stale-docker") return staleDocker.promise;
        dockerRequestCount += 1;
        return (dockerRequestCount === 1 ? dockerContainer : latestDocker).promise;
      }
    },
    getActiveTab: () => activeTab,
    getContainerListing: () => currentListing,
    getServicesConsoleTarget: () => serviceTarget,
    setServicesConsoleTarget: (target) => { serviceTarget = target?.kind === "container" ? target : null; },
    captureServicesConsoleState: () => undefined,
    resetContainerSelection: () => undefined,
    invalidateServiceSelection: () => { invalidatedServiceSelection += 1; },
    updateDockerConsoleDom: () => undefined,
    updateServiceConsoleDom: () => undefined,
    renderDocker: () => undefined,
    renderServices: () => undefined,
    messageOf: String
  });

  controller.selectContainer("service-container");
  activeTab = "docker";
  controller.selectContainer("docker-container");
  dockerContainer.resolve(containerLog("docker logs"));
  await settle();
  serviceContainer.resolve(containerLog("service logs"));
  await settle();

  assert.equal(controller.state("services").selectedContainerId, "service-container");
  assert.equal(controller.state("services").logState.logs, "service logs");
  assert.equal(controller.state("docker").selectedContainerId, "docker-container");
  assert.equal(controller.state("docker").logState.logs, "docker logs");
  assert.equal(invalidatedServiceSelection, 1);

  controller.selectContainer("stale-docker");
  controller.selectContainer("docker-container");
  latestDocker.resolve(containerLog("latest docker logs"));
  await settle();
  staleDocker.resolve(containerLog("stale docker logs"));
  await settle();
  assert.equal(controller.state("docker").selectedContainerId, "docker-container");
  assert.equal(controller.state("docker").logState.logs, "latest docker logs");

  currentListing = listing([container("service-container"), container("docker-container")]);
  controller.syncSelectedContainer();
  assert.equal(controller.state("services").selectedContainerId, "service-container");
});
