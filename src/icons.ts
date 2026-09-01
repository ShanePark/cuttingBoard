import "./devicon.generated.css";
import iconManifest from "./icon-manifest.json";
import { escapeHtml } from "./html";

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

export function uiIcon(name: UiIconName, size = 20, className = ""): string {
  const icon = iconManifest.ui[name];
  const safeClass = className ? ` ${escapeHtml(className)}` : "";
  const style = `--ui-icon-size:${safeSize(size)}px;--ui-icon-image:url('/${escapeHtml(icon.file)}')`;
  return `<span class="ui-icon ui-icon-${escapeHtml(name)}${safeClass}" style="${style}" aria-hidden="true"></span>`;
}

export function restartIcon(size = 20, className = ""): string {
  const edge = safeSize(size);
  const safeClass = className ? ` ${escapeHtml(className)}` : "";
  const playSize = Math.max(7, Math.round(edge * 0.5));
  return `<span class="restart-icon${safeClass}" style="--restart-icon-size:${edge}px" aria-hidden="true"><span class="restart-icon-ring">${uiIcon("refresh", edge)}</span><span class="restart-icon-play">${uiIcon("play", playSize)}</span></span>`;
}

export function techIcon(tech: string, size = 48, className = ""): string {
  const requested = normaliseTech(tech);
  const edge = safeSize(size);
  const safeClass = className ? ` ${escapeHtml(className)}` : "";

  if (Object.hasOwn(iconManifest.devicon, requested)) {
    const icon = iconManifest.devicon[requested as keyof typeof iconManifest.devicon];
    // The requested size is the fallback: a container such as a board tile sets --tech-icon-size
    // to scale its icons, and the inline font-size is the only lever a devicon glyph has.
    return `<i class="tech-icon devicon devicon-${escapeHtml(icon.name)}-${escapeHtml(icon.variant)} colored${safeClass}" style="font-size:var(--tech-icon-size, ${edge}px)" aria-hidden="true"></i>`;
  }

  const id = Object.hasOwn(iconManifest.tech, requested) ? requested : "service";
  const icon = iconManifest.tech[id as keyof typeof iconManifest.tech];
  return `<img class="tech-icon tech-${escapeHtml(id)}${safeClass}" src="/${escapeHtml(icon.file)}" width="${edge}" height="${edge}" alt="" aria-hidden="true" draggable="false">`;
}

function normaliseTech(tech: string): string {
  const id = tech.trim().toLowerCase().replaceAll("_", "-").replace(/\s+/g, "-");
  return TECH_ALIASES[id] ?? id;
}

function safeSize(value: number): number {
  return Number.isFinite(value) ? Math.min(512, Math.max(8, Math.round(value))) : 20;
}
