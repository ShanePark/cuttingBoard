import type { ContainerInfo, PublishedPortMapping } from "./types";

export function containerBrowserUrl(container: ContainerInfo, hostPort: number): string | null {
  if (container.state.trim().toLowerCase() !== "running") return null;
  const mappings = (container.port_mappings ?? [])
    .filter((mapping) => mapping.host_port === hostPort && mapping.protocol.trim().toLowerCase() === "tcp")
    .sort((left, right) => Number(right.web_scheme === "https") - Number(left.web_scheme === "https"));
  for (const mapping of mappings) {
    // Only the backend's network probe decides whether a port serves a web page.
    const scheme = mapping.web_scheme;
    if (scheme !== "http" && scheme !== "https") continue;
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
