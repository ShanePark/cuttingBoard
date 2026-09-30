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
      { host_ip: "0.0.0.0", host_port: 1025, container_port: 1025, protocol: "tcp" },
      { host_ip: "0.0.0.0", host_port: 8025, container_port: 8025, protocol: "tcp" }
    ],
    compose_project: null,
    compose_service: null,
    compose_working_dir: null,
    ...overrides
  };
}

test("opens Mailpit's published web port while leaving its SMTP port alone", () => {
  const mailpit = container();

  assert.equal(containerBrowserUrl(mailpit, 8025), "http://localhost:8025/");
  assert.equal(containerBrowserUrl(mailpit, 1025), null);
});

test("uses the host port and localhost for wildcard bindings", () => {
  const nginx = container({
    name: "nginx",
    image: "nginx:latest",
    ports: [18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "tcp" }]
  });

  assert.equal(containerBrowserUrl(nginx, 18080), "http://localhost:18080/");
});

test("uses localhost for the loopback binding reported by the running Mailpit container", () => {
  const mailpit = container({
    ports: [8025],
    port_mappings: [{ host_ip: "127.0.0.1", host_port: 8025, container_port: 8025, protocol: "tcp" }]
  });

  assert.equal(containerBrowserUrl(mailpit, 8025), "http://localhost:8025/");
});

test("recognizes the running nginx and Solr ports from their actual container targets", () => {
  const nginx = container({
    name: "idr_dev-front",
    image: "nginx:latest",
    ports: [18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 18080, protocol: "tcp" }]
  });
  const solr = container({
    name: "idr_dev-solr",
    image: "docker-compose-dev-solr",
    ports: [8983],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 8983, container_port: 8983, protocol: "tcp" }]
  });

  assert.equal(containerBrowserUrl(nginx, 18080), "http://localhost:18080/");
  assert.equal(containerBrowserUrl(solr, 8983), "http://localhost:8983/");
});

test("uses HTTPS when the container target is 443, 8443, or 9443", () => {
  const https = container({
    ports: [18443],
    port_mappings: [{ host_ip: "::", host_port: 18443, container_port: 443, protocol: "tcp" }]
  });

  assert.equal(containerBrowserUrl(https, 18443), "https://localhost:18443/");
  assert.equal(containerBrowserUrl(container({
    ports: [19443],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 19443, container_port: 9443, protocol: "tcp" }]
  }), 19443), "https://localhost:19443/");
});

test("keeps a web port visible when a Docker tile has more than two published ports", () => {
  const nginx = container({
    ports: [5432, 6379, 18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "tcp" }]
  });

  assert.deepEqual(dockerTilePorts(nginx), [18080, 5432, 6379]);
});

test("opens supported web mappings only for running TCP containers", () => {
  const database = container({
    ports: [15432],
    port_mappings: [{ host_ip: "127.0.0.1", host_port: 15432, container_port: 5432, protocol: "tcp" }]
  });
  const udp = container({
    ports: [18080],
    port_mappings: [{ host_ip: "0.0.0.0", host_port: 18080, container_port: 80, protocol: "udp" }]
  });

  assert.equal(containerBrowserUrl(database, 15432), null);
  assert.equal(containerBrowserUrl(udp, 18080), null);
  assert.equal(containerBrowserUrl(container({ state: "exited" }), 8025), null);
});

test("keeps explicit host bindings and rejects malformed binding addresses", () => {
  const explicit = container({
    ports: [18080],
    port_mappings: [{ host_ip: "192.168.1.20", host_port: 18080, container_port: 80, protocol: "tcp" }]
  });
  const malformed = container({
    ports: [18080],
    port_mappings: [{ host_ip: "http://example.test", host_port: 18080, container_port: 80, protocol: "tcp" }]
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
