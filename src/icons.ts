import "./devicon.generated.css";
import iconManifest from "./icon-manifest.json";

export type UiIconName = keyof typeof iconManifest.ui;

const TECH_FALLBACK_URL = "/icons/tech/service-192.png";
const TECH_ALIASES: Record<string, string> = {
  "apache-kafka": "kafka", "apache-maven": "maven", "apache-solr": "solr", "apache-tomcat": "tomcat",
  "c#": "dotnet", csharp: "dotnet", generic: "service", golang: "go", mongo: "mongodb",
  "node.js": "node", "node-js": "node", nodejs: "node", "next.js": "nextjs", "next-js": "nextjs",
  openjdk: "java", postgres: "postgresql", "postgresql-server": "postgresql", "ruby-on-rails": "rails",
  "spring-boot": "spring", traefikproxy: "traefik", "vue.js": "vue", vuejs: "vue",
};

document.addEventListener("error", (event) => {
  const image = event.target;
  if (!(image instanceof HTMLImageElement) || !image.classList.contains("tech-icon") || image.dataset.fallbackApplied) return;
  image.dataset.fallbackApplied = "true";
  image.src = TECH_FALLBACK_URL;
}, true);

export const TECH_ICON_IDS = Object.freeze([
  ...Object.keys(iconManifest.devicon),
  ...Object.keys(iconManifest.tech),
].sort());

export function uiIcon(name: UiIconName, size = 20, className = ""): string {
  const icon = iconManifest.ui[name];
  const safeClass = className ? ` ${escapeAttribute(className)}` : "";
  const style = `--ui-icon-size:${safeSize(size)}px;--ui-icon-image:url('/${escapeAttribute(icon.file)}')`;
  return `<span class="ui-icon ui-icon-${escapeAttribute(name)}${safeClass}" style="${style}" aria-hidden="true"></span>`;
}

export function techIcon(tech: string, size = 48, className = ""): string {
  const requested = normaliseTech(tech);
  const edge = safeSize(size);
  const safeClass = className ? ` ${escapeAttribute(className)}` : "";

  if (Object.hasOwn(iconManifest.devicon, requested)) {
    const icon = iconManifest.devicon[requested as keyof typeof iconManifest.devicon];
    return `<i class="tech-icon devicon devicon-${escapeAttribute(icon.name)}-${escapeAttribute(icon.variant)} colored${safeClass}" style="font-size:${edge}px" aria-hidden="true"></i>`;
  }

  const id = Object.hasOwn(iconManifest.tech, requested) ? requested : "service";
  const icon = iconManifest.tech[id as keyof typeof iconManifest.tech];
  return `<img class="tech-icon tech-${escapeAttribute(id)}${safeClass}" src="/${escapeAttribute(icon.file)}" width="${edge}" height="${edge}" alt="" aria-hidden="true" draggable="false">`;
}

function normaliseTech(tech: string): string {
  const id = tech.trim().toLowerCase().replaceAll("_", "-").replace(/\s+/g, "-");
  return TECH_ALIASES[id] ?? id;
}

function safeSize(value: number): number {
  return Number.isFinite(value) ? Math.min(512, Math.max(8, Math.round(value))) : 20;
}

function escapeAttribute(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '\"': "&quot;", "'": "&#39;",
  })[character] ?? character);
}
