import type { LaunchTask, ServiceSnapshot } from "./types";

export const FRESH_UPTIME_SECONDS = 300;

export function uniquePorts(service: ServiceSnapshot): number[] {
  return [...new Set(service.endpoints.map((endpoint) => endpoint.port))].sort((a, b) => a - b);
}

export function launchTasksEquivalent(left: LaunchTask[], right: LaunchTask[]): boolean {
  if (left.length !== right.length) return false;
  const normalise = (task: LaunchTask): string => JSON.stringify([task.name, task.cwd, task.command, task.expected_port]);
  const leftDefinitions = left.map(normalise).sort();
  const rightDefinitions = right.map(normalise).sort();
  return leftDefinitions.every((definition, index) => definition === rightDefinitions[index]);
}

export function currentUptime(service: ServiceSnapshot): number | null {
  const process = service.process;
  if (!process) return null;
  const now = Date.now() / 1000;
  return Math.max(process.uptime_seconds, Math.floor(now - process.create_time), 0);
}

export function formatUptimeCompact(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return "";
  const whole = Math.max(0, Math.floor(seconds));
  if (whole < 60) return `${whole}s`;
  const minutes = Math.floor(whole / 60);
  const secondsLeft = whole % 60;
  if (minutes < 60) return `${minutes}m ${secondsLeft}s`;
  const hours = Math.floor(minutes / 60);
  const minutesLeft = minutes % 60;
  if (hours < 24) return `${hours}h ${minutesLeft}m ${secondsLeft}s`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h ${minutesLeft}m ${secondsLeft}s`;
}

const TECH_LABELS: Record<string, string> = {
  dotnet: ".NET", elasticsearch: "Elasticsearch", fastapi: "FastAPI", graphql: "GraphQL",
  mariadb: "MariaDB", minio: "MinIO", mongodb: "MongoDB", mysql: "MySQL", nextjs: "Next.js",
  node: "Node.js", nodejs: "Node.js", php: "PHP", postgresql: "PostgreSQL", rabbitmq: "RabbitMQ",
  spring: "Spring Boot", sqlite: "SQLite", ssh: "SSH", vuejs: "Vue"
};

export function techLabel(tech: string): string {
  const id = tech.trim().toLowerCase();
  if (!id) return "Service";
  return TECH_LABELS[id] ?? id.replace(/(^|[-_ ])(\w)/g, (_match, separator: string, character: string) => `${separator ? " " : ""}${character.toUpperCase()}`);
}

// The backend names a service "<project> · <tech>", so the trailing segment repeats the
// technology label the card already shows. Drop it and keep the identity part.
export function serviceTitle(service: ServiceSnapshot): string {
  const name = service.display_name.trim();
  const parts = name.split(" · ");
  if (parts.length < 2) return name;
  const last = (parts[parts.length - 1] ?? "").trim().toLowerCase();
  if (last !== techLabel(service.tech).toLowerCase() && last !== service.tech.trim().toLowerCase()) return name;
  return parts.slice(0, -1).join(" · ") || name;
}

export function formatBytes(value: number | null): string {
  if (value === null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let amount = value;
  let index = 0;
  while (amount >= 1024 && index < units.length - 1) {
    amount /= 1024;
    index += 1;
  }
  return index < 2 ? `${Math.round(amount)} ${units[index]}` : `${amount.toFixed(1)} ${units[index]}`;
}

export function portBadgeLabels(ports: number[]): string[] {
  return ports.length > 2 ? [String(ports[0]), `+${ports.length - 1}`] : ports.map(String);
}

export function browserLinkLabel(value: string): string {
  try {
    const url = new URL(value);
    const normalized = url.hostname.toLowerCase().replace(/\.$/, "");
    let host = url.hostname;
    if (
      normalized === "localhost" || normalized === "localhost.localdomain" || normalized === "*" || normalized === "+" ||
      normalized.endsWith(".localhost") || normalized === "127.0.0.1" || normalized === "0.0.0.0" ||
      normalized === "::1" || normalized === "::"
    ) host = "localhost";
    else if (host.includes(":")) host = `[${host}]`;
    return `${host}${url.port ? `:${url.port}` : ""}${url.pathname === "/" ? "" : url.pathname}${url.search}${url.hash}`;
  } catch {
    return value;
  }
}

export function groupServices(services: ServiceSnapshot[]): Array<{
  id: string;
  name: string;
  path: string | null;
  accent: string;
  services: ServiceSnapshot[];
}> {
  const groups = new Map<string, { id: string; name: string; path: string | null; accent: string; services: ServiceSnapshot[] }>();
  for (const service of services) {
    const project = service.project;
    const workspacePath = project?.workspace_root_path || null;
    const id = workspacePath ?? project?.id ?? "other";
    const group = groups.get(id) ?? {
      id,
      name: project?.workspace_name || project?.name || "Other",
      path: workspacePath ?? project?.root_path ?? null,
      accent: service.category,
      services: []
    };
    group.services.push(service);
    groups.set(id, group);
  }
  for (const group of groups.values()) {
    group.services.sort((a, b) => (uniquePorts(a)[0] ?? 65536) - (uniquePorts(b)[0] ?? 65536) || a.display_name.localeCompare(b.display_name));
  }
  return [...groups.values()].sort((a, b) => (a.id === "other" ? 1 : b.id === "other" ? -1 : a.name.localeCompare(b.name)));
}

export function middleEllipsis(value: string, maximum: number): string {
  if (maximum < 5 || value.length <= maximum) return value;
  const left = Math.floor((maximum - 1) / 2);
  return `${value.slice(0, left)}…${value.slice(-(maximum - left - 1))}`;
}

export function shortenPath(value: string, maximum = 44): string {
  const home = navigator.userAgent.includes("Windows") ? null : value.match(/^(\/Users\/[^/]+|\/home\/[^/]+)/)?.[1];
  const text = home ? `~${value.slice(home.length)}` : value;
  if (text.length <= maximum) return text;
  const parts = text.split(/[\\/]/).filter(Boolean);
  return parts.length > 2 ? `.../${parts.slice(-2).join("/")}` : text.slice(-maximum);
}

export function imageTech(image: string): string {
  const value = image.toLowerCase();
  const tests: Array<[string, string]> = [
    ["postgres", "postgresql"], ["mysql", "mysql"], ["mariadb", "mariadb"], ["redis", "redis"],
    ["mongo", "mongodb"], ["elastic", "elasticsearch"], ["nginx", "nginx"], ["caddy", "caddy"],
    ["node", "nodejs"], ["python", "python"], ["java", "java"], ["rabbit", "rabbitmq"]
  ];
  return tests.find(([needle]) => value.includes(needle))?.[1] ?? "docker";
}
