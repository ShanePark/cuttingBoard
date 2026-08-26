import type { ServiceSnapshot } from "./types";

export const FRESH_UPTIME_SECONDS = 300;

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

const TECH_LABELS: Readonly<Record<string, string>> = {
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

export function middleEllipsis(value: string, maximum: number): string {
  if (maximum < 5 || value.length <= maximum) return value;
  const left = Math.floor((maximum - 1) / 2);
  return `${value.slice(0, left)}…${value.slice(-(maximum - left - 1))}`;
}
