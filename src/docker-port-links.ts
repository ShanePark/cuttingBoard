import type { ContainerInfo, PublishedPortMapping } from "./types";

const HTTP_CONTAINER_PORTS = new Set([
  80, 3000, 3001, 4173, 4200, 5000, 5173, 8000, 8008, 8025, 8080, 8081, 8082, 8083,
  8088, 8180, 8888, 8983, 9090, 9001, 15672, 5601, 16686, 18080
]);
const HTTPS_CONTAINER_PORTS = new Set([443, 8443, 9443]);

export function containerBrowserUrl(container: ContainerInfo, hostPort: number): string | null {
  if (container.state.trim().toLowerCase() !== "running") return null;
  const mappings = (container.port_mappings ?? [])
    .filter((mapping) => mapping.host_port === hostPort && mapping.protocol.trim().toLowerCase() === "tcp")
    .sort((left, right) => Number(schemeFor(right.container_port) === "https") - Number(schemeFor(left.container_port) === "https"));
  for (const mapping of mappings) {
    const scheme = schemeFor(mapping.container_port);
    if (!scheme) continue;
    const host = browserHost(mapping.host_ip);
    if (!host) continue;
    return `${scheme}://${host}:${mapping.host_port}/`;
  }
  return null;
}

export function dockerTilePorts(container: ContainerInfo): number[] {
  const ports = [...container.ports];
  if (ports.length <= 2) return ports;
  const browserPort = ports.find((port) => containerBrowserUrl(container, port) !== null);
  return browserPort === undefined || ports[0] === browserPort
    ? ports
    : [browserPort, ...ports.filter((port) => port !== browserPort)];
}

function schemeFor(containerPort: number): "http" | "https" | null {
  if (HTTPS_CONTAINER_PORTS.has(containerPort)) return "https";
  return HTTP_CONTAINER_PORTS.has(containerPort) ? "http" : null;
}

function browserHost(value: PublishedPortMapping["host_ip"]): string | null {
  const address = value?.trim();
  if (!address || address === "*" || address === "localhost" || address === "0.0.0.0" || address === "::" || /^127\./.test(address) || isIpv6Loopback(address) || isIpv6Unspecified(address)) {
    return "localhost";
  }
  if (address.includes(":")) {
    try {
      new URL(`http://[${address}]/`);
      return `[${address}]`;
    } catch {
      return null;
    }
  }
  const octets = address.split(".");
  if (octets.length !== 4 || octets.some((octet) => !/^(0|[1-9]\d{0,2})$/.test(octet) || Number(octet) > 255)) return null;
  return address;
}

function isIpv6Loopback(address: string): boolean {
  const groups = address.toLowerCase().split(":");
  if (groups.length === 8) return groups.slice(0, 7).every((group) => /^0*$/.test(group)) && groups[7] === "1";
  return address.toLowerCase() === "::1";
}

function isIpv6Unspecified(address: string): boolean {
  if (!address.includes(":")) return false;
  try {
    return new URL(`http://[${address}]/`).hostname === "[::]";
  } catch {
    return false;
  }
}
