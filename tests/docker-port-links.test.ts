import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { containerBrowserUrl, dockerTilePorts } from "../src/docker-port-links.ts";
import type { ContainerInfo } from "../src/types.ts";

function container(overrides: Partial<ContainerInfo> = {}): ContainerInfo {
  return {
    id: "c".repeat(64),
    name: "mailpit",
    image: "axllent/mailpit:latest",
    state: "running",
    status: "Up",
    ports: [1025, 8025],
    port_mappings: [
      { host_ip: "0.0.0.0", host_port: 1025, container_port: 1025, protocol: "tcp", web_scheme: null },
      { host_ip: "0.0.0.0", host_port: 8025, container_port: 8025, protocol: "tcp", web_scheme: "http" }
    ],
    compose_project: null,
    compose_service: null,
    compose_working_dir: null,
    ...overrides
  };
}

test("opens Mailpit's probed web port while leaving its SMTP port alone", () => {
  const mailpit = container();

  assert.equal(containerBrowserUrl(mailpit, 8025), "http://localhost:8025/");
  assert.equal(containerBrowserUrl(mailpit, 1025), null);
});

test("links only ports the backend probe confirmed, whatever their container port", () => {
  const unprobed = container({
    name: "nginx",
    image: "nginx:latest",
    ports: [18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "tcp" }]
  });
  const notWeb = container({
    name: "ryuk",
    image: "testcontainers/ryuk:0.11.0",
    ports: [32768],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 32768, container_port: 8080, protocol: "tcp", web_scheme: null }]
  });
  const unusualPort = container({
    name: "dataworks-dev-front-1",
    image: "nginx:latest",
    ports: [28080],
    port_mappings: [{ host_ip: "127.0.0.1", host_port: 28080, container_port: 28080, protocol: "tcp", web_scheme: "http" }]
  });

  assert.equal(containerBrowserUrl(unprobed, 18080), null);
  assert.equal(containerBrowserUrl(notWeb, 32768), null);
  assert.equal(containerBrowserUrl(unusualPort, 28080), "http://localhost:28080/");
});

test("uses the probed scheme rather than the container port", () => {
  const tls = container({
    ports: [29502],
    port_mappings: [{ host_ip: "::", host_port: 29502, container_port: 29443, protocol: "tcp", web_scheme: "https" }]
  });
  const plainOn443 = container({
    ports: [29503],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 29503, container_port: 443, protocol: "tcp", web_scheme: "http" }]
  });

  assert.equal(containerBrowserUrl(tls, 29502), "https://localhost:29502/");
  assert.equal(containerBrowserUrl(plainOn443, 29503), "http://localhost:29503/");
});

test("prefers an HTTPS mapping when a host port lists several", () => {
  const mixed = container({
    ports: [18443],
    port_mappings: [
      { host_ip: "0.0.0.0", host_port: 18443, container_port: 8080, protocol: "tcp", web_scheme: "http" },
      { host_ip: "::", host_port: 18443, container_port: 8443, protocol: "tcp", web_scheme: "https" }
    ]
  });

  assert.equal(containerBrowserUrl(mixed, 18443), "https://localhost:18443/");
});

test("keeps a web port visible when a Docker tile has more than two published ports", () => {
  const nginx = container({
    ports: [5432, 6379, 18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "tcp", web_scheme: "http" }]
  });

  assert.deepEqual(dockerTilePorts(nginx), [18080, 5432, 6379]);
});

test("opens web mappings only for running TCP containers", () => {
  const udp = container({
    ports: [18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "udp", web_scheme: "http" }]
  });

  assert.equal(containerBrowserUrl(udp, 18080), null);
  assert.equal(containerBrowserUrl(container({ state: "exited" }), 8025), null);
});

test("keeps explicit host bindings and rejects malformed binding addresses", () => {
  const explicit = container({
    ports: [18080],
    port_mappings: [{ host_ip: "192.168.1.20", host_port: 18080, container_port: 80, protocol: "tcp", web_scheme: "http" }]
  });
  const malformed = container({
    ports: [18080],
    port_mappings: [{ host_ip: "http://example.test", host_port: 18080, container_port: 80, protocol: "tcp", web_scheme: "http" }]
  });

  assert.equal(containerBrowserUrl(explicit, 18080), "http://192.168.1.20:18080/");
  assert.equal(containerBrowserUrl(malformed, 18080), null);
});

test("renders a labeled action on Docker port chips and routes it through the shared URL opener", () => {
  const rendering = readFileSync(new URL("../src/docker-rendering.ts", import.meta.url), "utf8");
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

  assert.match(rendering, /port_mappings: container\.port_mappings/);
  assert.match(rendering, /renderTileFoot\(dockerTilePorts\(container\),/);
  assert.match(rendering, /data-action="open-container-port" data-container-id="\$\{h\(container\.id\)\}" data-port="\$\{port\}" aria-label="Open/);
  assert.match(main, /action === "open-container-port"[\s\S]+await openUrl\(url\)/);
});
