import type { ContainerInfo, ServiceSnapshot } from "./types";

export type ServiceGroup = {
  id: string;
  name: string;
  path: string | null;
  accent: string;
  services: ServiceSnapshot[];
};

export type ServiceBoardGroup = ServiceGroup & { containers: ContainerInfo[] };

export function uniquePorts(service: ServiceSnapshot): number[] {
  return [...new Set(service.endpoints.map((endpoint) => endpoint.port))].sort((a, b) => a - b);
}

export function groupServices(services: ServiceSnapshot[]): ServiceGroup[] {
  const groups = new Map<string, ServiceGroup>();
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

function normalisePath(value: string): string {
  const input = value.trim().replaceAll("\\", "/");
  const absolute = input.startsWith("/");
  const parts: string[] = [];
  for (const part of input.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length && parts[parts.length - 1] !== "..") parts.pop();
      else if (!absolute) parts.push(part);
      continue;
    }
    parts.push(part);
  }
  const normalized = `${absolute ? "/" : ""}${parts.join("/")}`;
  return (normalized || (absolute ? "/" : "")).replace(/\/+$/, "") || "/";
}

export function pathIsEqualOrNested(path: string, root: string): boolean {
  const candidate = normalisePath(path);
  const parent = normalisePath(root);
  if (!candidate || !parent) return false;
  return parent === "/" ? candidate.startsWith("/") : candidate === parent || candidate.startsWith(`${parent}/`);
}

/**
 * The groups the services board shows, in the order it shows them: every project with its own
 * services and the containers that belong to the same project. The tab count measures the same
 * set, so the badge cannot drift from the cards on screen.
 */
export function serviceBoardGroups(services: readonly ServiceSnapshot[], containers: readonly ContainerInfo[]): ServiceBoardGroup[] {
  return groupServices([...services])
    .map((group) => ({ ...group, containers: relatedContainersForGroup(group.services, [...containers]) }))
    .sort((left, right) => boardGroupCards(right) - boardGroupCards(left) || left.name.localeCompare(right.name));
}

export function boardGroupCards(group: ServiceBoardGroup): number {
  return group.services.length + group.containers.length;
}

export function relatedContainersForGroup(services: ServiceSnapshot[], containers: ContainerInfo[]): ContainerInfo[] {
  const roots = services
    .map((service) => service.project?.root_path?.trim() || (!service.project ? service.process?.working_directory?.trim() : ""))
    .filter((root): root is string => Boolean(root));
  if (roots.length === 0) return [];
  return containers
    .filter((container) => container.state.trim().toLowerCase() === "running")
    .filter((container) => {
      const workingDir = container.compose_working_dir?.trim();
      return Boolean(workingDir && roots.some((root) => pathIsEqualOrNested(workingDir, root)));
    })
    .sort((left, right) => left.name.localeCompare(right.name));
}

const IMAGE_TECH_TESTS: ReadonlyArray<readonly [string, string]> = [
  ["postgres", "postgresql"], ["mysql", "mysql"], ["mariadb", "mariadb"], ["redis", "redis"],
  ["mongo", "mongodb"], ["elastic", "elasticsearch"], ["nginx", "nginx"], ["caddy", "caddy"],
  ["node", "nodejs"], ["python", "python"], ["java", "java"], ["rabbit", "rabbitmq"]
];

export function imageTech(image: string): string {
  const value = image.toLowerCase();
  return IMAGE_TECH_TESTS.find(([needle]) => value.includes(needle))?.[1] ?? "docker";
}

