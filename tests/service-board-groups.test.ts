import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { boardGroupCards, serviceBoardGroups } from "../src/presentation-services.ts";
import type { ContainerInfo, ServiceSnapshot } from "../src/types.ts";

function service(id: string, root: string, port: number): ServiceSnapshot {
  return {
    id,
    display_name: id,
    tech: "java",
    category: "api",
    relevance: "dev",
    endpoints: [{ family: "IPv4", address: "127.0.0.1", port, scope: "local", protocol: "TCP" }],
    process: null,
    project: { id: root, name: id, root_path: root, workspace_root_path: root, workspace_name: "OASIS26", detection_source: "git" },
    status: "healthy",
    warnings: [],
    origin_kind: "terminal",
    origin_label: null,
    can_terminate: true,
    browser_url: null,
    active_profiles: []
  };
}

function container(name: string, workingDir: string | null): ContainerInfo {
  return {
    id: name,
    name,
    image: "postgres:16",
    state: "running",
    status: "Up 5 hours (healthy)",
    ports: [45432],
    compose_project: "oasis26",
    compose_service: name,
    compose_working_dir: workingDir
  };
}

test("counts the container grouped with a project's services as a card of that group", () => {
  const groups = serviceBoardGroups(
    [service("backend", "/home/dev/oasis26", 48080), service("oasis26", "/home/dev/oasis26", 48983)],
    [container("oasis-dev-postgres", "/home/dev/oasis26")]
  );

  assert.equal(groups.length, 1);
  assert.equal(groups[0]!.services.length, 2);
  assert.equal(groups[0]!.containers.length, 1);
  assert.equal(boardGroupCards(groups[0]!), 3);
});

test("leaves out containers that belong to another project", () => {
  const groups = serviceBoardGroups(
    [service("backend", "/home/dev/oasis26", 48080)],
    [container("other-db", "/home/dev/somewhere-else"), container("loose-db", null)]
  );

  assert.equal(boardGroupCards(groups[0]!), 1);
});

test("the tab badge counts the same cards the board renders", () => {
  const uiSupport = readFileSync(new URL("../src/ui-support.ts", import.meta.url), "utf8");
  const servicesRendering = readFileSync(new URL("../src/services-rendering.ts", import.meta.url), "utf8");

  assert.match(uiSupport, /serviceBoardGroups\(services, listing\?\.available \? listing\.containers : \[\]\)/);
  assert.match(uiSupport, /boardGroupCards\(group\)/);
  assert.match(servicesRendering, /serviceBoardGroups\(context\.services, context\.containers\)/);
});

test("every grouped view shows how many cards its header stands for", () => {
  const headers: Record<string, RegExp> = {
    "services-rendering.ts": /renderGroupCount\(boardGroupCards\(group\)\)/,
    "docker-rendering.ts": /renderGroupCount\(group\.containers\.length\)/,
    "launch-rendering.ts": /renderGroupCount\(profile\.tasks\.length\)/,
  };

  for (const [file, pattern] of Object.entries(headers)) {
    assert.match(readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8"), pattern, file);
  }
});
