import type {
  LaunchBuildTool,
  LaunchPrepareSpec,
  ServiceSnapshot
} from "./types";

/**
 * Build the preparation metadata that is carried with a Spring service task.
 *
 * The scanner already exposes the project marker it found, so it is safe to use that as a
 * build-tool hint. When no marker is available we deliberately leave the tool unset: the native
 * runner can inspect the project itself, while the profile still has enough Spring metadata to
 * opt into preparation.
 */
export function springPrepareForService(
  service: ServiceSnapshot,
  groupPath: string | null,
  command = service.process?.launch_command ?? service.process?.command ?? ""
): LaunchPrepareSpec {
  return {
    kind: "spring_boot",
    build_tool: springBuildTool(service),
    module: relativeModulePath(service.project?.root_path, groupPath),
    profiles: springProfilesForService(service, command),
    main_class: springMainClass(command)
  };
}

/** Infer a build tool only from project information the scanner has already read. */
export function springBuildTool(service: ServiceSnapshot): LaunchBuildTool | null {
  const marker = service.project?.detection_source?.trim().toLowerCase() ?? "";
  if (
    marker === "pom.xml"
    || marker === "maven"
    || marker.endsWith("/pom.xml")
    || marker.endsWith("\\pom.xml")
  ) return "maven";
  if (
    marker === "build.gradle"
    || marker === "build.gradle.kts"
    || marker === "settings.gradle"
    || marker === "settings.gradle.kts"
    || marker === "gradle"
    || marker.endsWith("/build.gradle")
    || marker.endsWith("/build.gradle.kts")
    || marker.endsWith("/settings.gradle")
    || marker.endsWith("/settings.gradle.kts")
    || marker.endsWith("\\build.gradle")
    || marker.endsWith("\\build.gradle.kts")
    || marker.endsWith("\\settings.gradle")
    || marker.endsWith("\\settings.gradle.kts")
  ) return "gradle";
  return null;
}

/** Resolve a scanner project root to the path used by Maven/Gradle from the profile root. */
export function relativeModulePath(projectRoot: string | null | undefined, groupPath: string | null): string | null {
  const child = normalizedPath(projectRoot);
  const parent = normalizedPath(groupPath);
  if (!child || !parent || child === parent) return null;
  const comparableChild = child.toLowerCase();
  const comparableParent = parent.toLowerCase();
  if (!comparableChild.startsWith(`${comparableParent}/`)) return null;
  return child.slice(parent.length + 1) || null;
}

/** Prefer the scanner's resolved profiles, falling back to the launch command when needed. */
export function springProfilesForService(service: ServiceSnapshot, command: string): string[] {
  const fromSnapshot = uniqueProfiles(service.active_profiles);
  return fromSnapshot.length ? fromSnapshot : uniqueProfiles(parseSpringProfiles(command));
}

/** Extract a Java/Boot main class without mistaking classpath entries or option values for one. */
export function springMainClass(command: string): string | null {
  const explicit = command.match(/(?:--main-class|mainClass)(?:=|\s+)([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+)/);
  if (explicit?.[1]) return explicit[1];

  const tokens = shellTokens(command);
  const javaIndex = tokens.findIndex((token) => basename(token).toLowerCase() === "java" || basename(token).toLowerCase() === "java.exe");
  if (javaIndex < 0) return null;
  const optionsWithValue = new Set(["-cp", "-classpath", "--class-path", "-p", "--module-path", "--upgrade-module-path", "--add-exports", "--add-opens", "--add-reads", "--patch-module"]);
  let skipNext = false;
  for (const token of tokens.slice(javaIndex + 1)) {
    if (skipNext) {
      skipNext = false;
      continue;
    }
    if (optionsWithValue.has(token)) {
      skipNext = true;
      continue;
    }
    if (token === "-jar" || token === "--module" || token === "-m") {
      // A jar/module launch does not expose the application class in the command.
      return null;
    }
    if (token.startsWith("-")) continue;
    if (token.endsWith(".jar") || token.includes("/") || token.includes("\\")) continue;
    if (/^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+$/.test(token)) return token;
  }
  return null;
}

export function isSpringService(service: ServiceSnapshot): boolean {
  const tech = service.tech.trim().toLowerCase();
  return tech === "spring" || tech === "spring_boot" || tech === "spring boot" || tech.includes("spring");
}

function normalizedPath(value: string | null | undefined): string | null {
  const original = value?.trim() ?? "";
  if (!original) return null;
  const input = original.replaceAll("\\", "/").replace(/\/+$/, "");
  if (!input) return original.startsWith("/") ? "/" : null;
  const absolute = input.startsWith("/") || /^[A-Za-z]:\//.test(input);
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
  const normalized = `${absolute && input.startsWith("/") ? "/" : ""}${parts.join("/")}`;
  return normalized || (absolute ? "/" : null);
}

function uniqueProfiles(values: readonly string[]): string[] {
  return [...new Set(values.flatMap((value) => value.split(",")).map((value) => value.trim()).filter(Boolean))];
}

function parseSpringProfiles(command: string): string[] {
  const values: string[] = [];
  const pattern = /(?:spring\.profiles\.active|SPRING_PROFILES_ACTIVE)\s*(?:=|\s+)\s*["']?([^\s"']+)/gi;
  for (const match of command.matchAll(pattern)) values.push(...match[1]!.split(","));
  return values;
}

function shellTokens(command: string): string[] {
  return command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g)?.map((token) => token.replace(/^['"]|['"]$/g, "")) ?? [];
}

function basename(value: string): string {
  return value.split(/[\\/]/).pop() ?? value;
}
