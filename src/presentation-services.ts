import type { ContainerInfo, LaunchProfile, LaunchTask, ServiceSnapshot } from "./types";

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

/** Whether two paths name the same directory or one lies inside the other. */
export function pathsMatch(left: string | null | undefined, right: string | null | undefined): boolean {
  const candidate = left?.trim();
  const parent = right?.trim();
  if (!candidate || !parent || candidate === "." || parent === ".") return false;
  return candidate === parent || pathIsEqualOrNested(candidate, parent) || pathIsEqualOrNested(parent, candidate);
}

/** The directory a task runs from: its own when absolute, otherwise a directory inside the profile's project. */
export function launchTaskRoot(profile: LaunchProfile, task: LaunchTask): string {
  const cwd = task.cwd.trim();
  return isAbsolutePath(cwd) ? cwd : `${profile.project_root}/${cwd}`;
}

/**
 * The running service that backs a task, by the rule the backend uses to report a task as running
 * outside Cutting Board (task_matches_service in src-tauri/src/launch/external.rs): the service
 * listens on the task's port and runs from the task's directory, one inside it or one containing
 * it, judged by the process's working directory or the project it was scanned under. The card's
 * metrics and the task's state then rest on the same evidence, so a service that merely shares
 * the port never shows a running uptime on a task the backend reports as stopped. Docker answers
 * for a container task, so no process backs one, and only the dev services the board shows count.
 */
export function matchedServiceForTask(profile: LaunchProfile, task: LaunchTask, services: readonly ServiceSnapshot[]): ServiceSnapshot | null {
  const port = task.expected_port;
  if (port === null || port === undefined || task.container?.trim()) return null;
  const taskRoot = launchTaskRoot(profile, task);
  const scored = services
    .filter((service) => service.relevance === "dev" && uniquePorts(service).includes(port))
    .map((service) => ({ service, score: serviceRootScore(service, taskRoot) }))
    .filter(({ score }) => score > 0)
    .sort((left, right) => right.score - left.score || left.service.id.localeCompare(right.service.id));
  return scored[0]?.service ?? null;
}

// A process's own directory is stronger evidence than the project it was scanned under.
function serviceRootScore(service: ServiceSnapshot, taskRoot: string): number {
  if (pathsMatch(service.process?.working_directory, taskRoot)) return 2;
  const project = service.project;
  return project && (pathsMatch(project.root_path, taskRoot) || pathsMatch(project.workspace_root_path, taskRoot)) ? 1 : 0;
}

function isAbsolutePath(path: string): boolean {
  return /^(?:[\\/]|[A-Za-z]:[\\/])/.test(path);
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

/**
 * Launch tasks for the containers of a board group. A container task carries the container it
 * stands for instead of a command, so a saved profile starts and stops a project's containers
 * alongside its services. Names that a service task already took are suffixed.
 */
export function containerLaunchTasks(containers: readonly ContainerInfo[], takenNames: readonly string[]): LaunchTask[] {
  const used = new Set(takenNames.map((name) => name.toLowerCase()));
  return containers.map((container) => {
    let name = container.name;
    let suffix = 2;
    while (used.has(name.toLowerCase())) name = `${container.name} ${suffix++}`;
    used.add(name.toLowerCase());
    return { name, cwd: ".", command: "", expected_port: container.ports[0] ?? null, container: container.name };
  });
}

/**
 * Framework evidence anywhere in a command, checked before the runtime that hosts it: a Spring
 * Boot classpath reads as Spring rather than Java, and `node …/vite dev` as Vite rather than Node.
 * Needles match whole terms only, so "vite" is found in ".bin/vite" but not in "invited".
 */
const COMMAND_FRAMEWORK_TESTS: ReadonlyArray<readonly [string, string]> = [
  ["spring-boot", "spring"], ["springframework", "spring"], ["bootrun", "spring"],
  ["quarkus", "java"], ["micronaut", "java"], ["catalina", "tomcat"],
  ["next dev", "nextjs"], ["next-server", "nextjs"], ["nuxt", "nuxt"], ["astro", "astro"], ["remix", "remix"],
  ["storybook", "storybook"], ["svelte-kit", "svelte"], ["sveltekit", "svelte"], ["ng serve", "angular"], ["angular", "angular"],
  ["vite", "vite"], ["webpack-dev-server", "nodejs"], ["react-scripts", "nodejs"],
  ["django", "django"], ["manage.py runserver", "django"], ["fastapi", "fastapi"], ["uvicorn", "fastapi"],
  ["gunicorn", "python"], ["hypercorn", "python"], ["daphne", "python"], ["flask", "flask"],
  ["rails", "rails"], ["puma", "rails"], ["artisan", "laravel"],
  ["jupyter-lab", "jupyter"], ["jupyter-notebook", "jupyter"],
  ["cargo run", "rust"], ["target/debug", "rust"], ["target/release", "rust"], ["go run", "go"]
];

/** The technology a command's executable, or one of its first arguments, stands for by name. */
const RUNTIME_TECH: Readonly<Record<string, string>> = {
  java: "java", "java.exe": "java", gradlew: "gradle", gradle: "gradle", mvn: "maven", mvnw: "maven",
  node: "nodejs", nodejs: "nodejs", npm: "nodejs", npx: "nodejs", pnpm: "nodejs", yarn: "nodejs", nodemon: "nodejs", "ts-node": "nodejs", tsx: "nodejs",
  python: "python", python3: "python", poetry: "python", pipenv: "python", uv: "python",
  ruby: "ruby", bundle: "ruby", rake: "ruby", php: "php", composer: "php", dotnet: "dotnet",
  cargo: "rust", go: "go", deno: "deno", bun: "bun", bunx: "bun",
  docker: "docker", "docker-compose": "docker", podman: "docker", ssh: "ssh", autossh: "ssh",
  solr: "solr", "kafka-server-start.sh": "kafka", "redis-server": "redis", postgres: "postgresql", pg_ctl: "postgresql",
  mysqld: "mysql", mongod: "mongodb", nginx: "nginx", caddy: "caddy", ollama: "ollama"
};

const COMMAND_PREFIXES = new Set(["sudo", "env", "nohup", "exec"]);

/** The technology a saved command stands for, or null when nothing recognisable is in it. */
export function commandTech(command: string): string | null {
  const text = command.trim().toLowerCase();
  if (!text) return null;
  for (const [needle, tech] of COMMAND_FRAMEWORK_TESTS) if (hasTerm(text, needle)) return tech;
  for (const name of commandIdentity(text)) {
    const tech = RUNTIME_TECH[name];
    if (tech) return tech;
  }
  return null;
}

/**
 * The technology a task card shows. A running task takes it from the service backing it, a
 * container task from its image, and anything else from the saved command, so a stopped task
 * still shows what it is instead of a bare terminal.
 */
export function launchTaskTech(task: LaunchTask, matchedService: ServiceSnapshot | null, containers: readonly ContainerInfo[]): string | null {
  if (matchedService) return matchedService.tech;
  const containerName = task.container?.trim();
  if (containerName) {
    const container = containers.find((item) => item.name === containerName);
    return container ? imageTech(container.image) : "docker";
  }
  return commandTech(task.command);
}

// Mirrors the scanner's process identity: the executable plus the first two non-flag arguments,
// each reduced to its basename, after skipping environment assignments and wrappers.
function commandIdentity(command: string): string[] {
  const tokens = command.split(/\s+/).filter(Boolean);
  let start = 0;
  while (start < tokens.length && (/^[a-z_][a-z0-9_]*=/.test(tokens[start]!) || COMMAND_PREFIXES.has(tokens[start]!))) start += 1;
  const names = [tokens[start], ...tokens.slice(start + 1).filter((token) => !token.startsWith("-")).slice(0, 2)];
  return names.filter((token): token is string => Boolean(token)).map((token) => token.split(/[\\/]/).pop() ?? token);
}

function hasTerm(text: string, needle: string): boolean {
  let from = 0;
  while (from <= text.length) {
    const index = text.indexOf(needle, from);
    if (index === -1) return false;
    const before = text[index - 1];
    const after = text[index + needle.length];
    if (!(before && isWordCharacter(before)) && !(after && isWordCharacter(after))) return true;
    from = index + 1;
  }
  return false;
}

function isWordCharacter(character: string): boolean {
  return /[a-z0-9]/.test(character);
}
