#!/usr/bin/env node

import { access, mkdir, mkdtemp, readFile, readdir, rename, rm, stat, writeFile } from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import properLockfile from "proper-lockfile";
import sharp from "sharp";
import * as simpleIcons from "simple-icons";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const PUBLIC = join(ROOT, "public");
const OUTPUT = join(ROOT, "public", "icons");
const PUBLISH_LOCK = join(PUBLIC, ".icons-publish.lock");
const STAGING_STALE_MS = 60 * 60 * 1_000;
const TECH_SIZE = 192;
const UI_SIZE = 64;
const EXPECTED_VERSIONS = { devicon: "2.17.0", "lucide-static": "1.33.0", "simple-icons": "16.28.0" };

const DEVICON_CATALOG = {
  android: "android", angular: "angularjs", astro: "astro", bun: "bun", deno: "denojs", django: "django",
  docker: "docker", dotnet: "dotnetcore", elasticsearch: "elasticsearch", electron: "electron", elixir: "elixir",
  fastapi: "fastapi", firebase: "firebase", flask: "flask", flutter: "flutter", go: "go", gradle: "gradle",
  grafana: "grafana", graphql: "graphql", java: "java", jupyter: "jupyter", kafka: "apachekafka",
  kotlin: "kotlin", kubernetes: "kubernetes", laravel: "laravel", mariadb: "mariadb", maven: "maven",
  mongodb: "mongodb", mysql: "mysql", nextjs: "nextjs", nginx: "nginx", node: "nodejs", nuxt: "nuxtjs",
  php: "php", postgresql: "postgresql", prometheus: "prometheus", python: "python", rabbitmq: "rabbitmq",
  rails: "rails", react: "react", redis: "redis", remix: "remix", ruby: "ruby", rust: "rust",
  spring: "spring", sqlite: "sqlite", ssh: "ssh", storybook: "storybook", supabase: "supabase", svelte: "svelte",
  tomcat: "tomcat", traefik: "traefikproxy", vite: "vite", vue: "vuejs", webpack: "webpack",
};

const FALLBACK_TECH = {
  caddy: "caddy", keycloak: "keycloak", minio: "minio", ollama: "ollama", solr: "apachesolr",
};

const UI_CATALOG = {
  github: ["simple-icons", "github"], plus: ["lucide", "plus"], close: ["lucide", "x"],
  settings: ["lucide", "settings"], info: ["lucide", "info"], details: ["lucide", "maximize-2"],
  play: ["lucide", "play"], stop: ["lucide", "square"], power: ["lucide", "power"],
  log: ["lucide", "file-text"], theme: ["lucide", "palette"], scan: ["lucide", "scan"],
  scanning: ["lucide", "scan"], link: ["lucide", "link"], external: ["lucide", "external-link"],
  ide: ["lucide", "square-terminal"], terminal: ["lucide", "terminal"], docker: ["lucide", "box"],
  refresh: ["lucide", "refresh-cw"], chevronDown: ["lucide", "chevron-down"], folder: ["lucide", "folder"],
  clock: ["lucide", "clock"], cpu: ["lucide", "cpu"], memory: ["lucide", "memory-stick"],
  port: ["lucide", "ethernet-port"], warning: ["lucide", "triangle-alert"], check: ["lucide", "check"],
};

const SIMPLE_ICONS = new Map(Object.values(simpleIcons)
  .filter((value) => value && typeof value === "object" && "slug" in value && "path" in value)
  .map((icon) => [icon.slug, icon]));

async function main() {
  await verifyPackageVersions();
  await mkdir(PUBLIC, { recursive: true });
  await cleanupStaleStagingDirectories();
  const staging = await mkdtemp(join(PUBLIC, ".icons-build-"));
  await mkdir(join(staging, "tech"), { recursive: true });
  await mkdir(join(staging, "ui"), { recursive: true });
  await mkdir(join(staging, "source"), { recursive: true });

  const deviconData = JSON.parse(await readFile(join(ROOT, "node_modules", "devicon", "devicon.json"), "utf8"));
  const deviconCss = await readFile(join(ROOT, "node_modules", "devicon", "devicon.min.css"), "utf8");
  const manifest = {
    schema: 2,
    sizes: { fallbackTech: TECH_SIZE, ui: UI_SIZE },
    sources: {
      devicon: { version: EXPECTED_VERSIONS.devicon, license: "MIT", url: "https://github.com/devicons/devicon" },
      lucide: {
        version: EXPECTED_VERSIONS["lucide-static"],
        license: "ISC; Feather-derived icons are MIT",
        url: "https://github.com/lucide-icons/lucide",
      },
      simpleIcons: { version: EXPECTED_VERSIONS["simple-icons"], license: "CC0-1.0", url: "https://github.com/simple-icons/simple-icons" },
    },
    devicon: {}, tech: {}, ui: {},
  };

  try {
    for (const [id, name] of Object.entries(DEVICON_CATALOG)) {
      const entry = deviconData.find((icon) => icon.name === name);
      const variant = ["original", "plain", "line"].find((candidate) => entry?.versions?.font?.includes(candidate));
      if (!variant) throw new Error(`Devicon font variant not found: ${name}`);
      manifest.devicon[id] = { name, variant };
    }
    await writeDeviconCss(deviconCss, manifest.devicon, join(staging, "source", "devicon.generated.css"));

    for (const [id, slug] of Object.entries(FALLBACK_TECH)) {
      const relative = `icons/tech/${id}-${TECH_SIZE}.png`;
      await rasterise(simpleIconSvg(slug), join(staging, "tech", `${id}-${TECH_SIZE}.png`), TECH_SIZE);
      manifest.tech[id] = { file: relative, library: "simple-icons", icon: slug };
    }
    await rasterise(serviceSvg(), join(staging, "tech", `service-${TECH_SIZE}.png`), TECH_SIZE);
    manifest.tech.service = { file: `icons/tech/service-${TECH_SIZE}.png`, library: "cutting-board", icon: "service" };

    for (const [id, [library, sourceId]] of Object.entries(UI_CATALOG)) {
      const svg = library === "lucide" ? await lucideSvg(sourceId) : simpleIconSvg(sourceId);
      const relative = `icons/ui/${id}-${UI_SIZE}.png`;
      await rasterise(asWhiteMask(svg), join(staging, "ui", `${id}-${UI_SIZE}.png`), UI_SIZE);
      manifest.ui[id] = { file: relative, library, icon: sourceId };
    }

    const manifestJson = `${JSON.stringify(manifest, null, 2)}\n`;
    await writeFile(join(staging, "manifest.json"), manifestJson);
    await writeFile(join(staging, "source", "icon-manifest.json"), manifestJson);
    await verifyManifest(staging, manifest);

    await withPublishLock(async () => {
      const expectedRuntimeFiles = await publishRuntimeAssets(staging, manifest);
      await atomicReplaceIfChanged(
        join(ROOT, "src", "devicon.generated.css"),
        await readFile(join(staging, "source", "devicon.generated.css")),
      );
      await atomicReplaceIfChanged(
        join(OUTPUT, "manifest.json"),
        await readFile(join(staging, "manifest.json")),
      );
      await atomicReplaceIfChanged(
        join(ROOT, "src", "icon-manifest.json"),
        await readFile(join(staging, "source", "icon-manifest.json")),
      );
      await removeStaleRuntimeFiles(expectedRuntimeFiles);
    });
    console.log(`Bundled ${Object.keys(manifest.devicon).length} Devicon font mappings, ${Object.keys(manifest.tech).length} technology PNG fallbacks, and ${Object.keys(manifest.ui).length} UI PNG icons.`);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}

async function writeDeviconCss(source, mappings, destination) {
  const rules = new Map();
  for (const match of source.matchAll(/([^{}]+)\{([^{}]+)\}/g)) {
    const selectors = match[1].split(",").map((selector) => selector.trim());
    for (const selector of selectors) rules.set(selector, match[2]);
  }
  const output = [
    "/* Generated by scripts/build-icons.mjs from devicon 2.17.0. */",
    '@font-face{font-family:"devicon";src:url("../node_modules/devicon/fonts/devicon.woff") format("woff");font-weight:normal;font-style:normal;font-display:block}',
    '.devicon{font-family:"devicon"!important;font-style:normal;font-weight:normal;font-variant:normal;text-transform:none;line-height:1;-webkit-font-smoothing:antialiased;-moz-osx-font-smoothing:grayscale}',
  ];
  for (const { name, variant } of Object.values(mappings)) {
    const base = `.devicon-${name}-${variant}`;
    const glyph = rules.get(`${base}:before`);
    const colour = rules.get(`${base}.colored`);
    if (!glyph) throw new Error(`Devicon glyph CSS missing: ${base}`);
    output.push(`${base}:before{${glyph}}`);
    if (colour) output.push(`${base}.colored{${colour}}`);
  }
  // Devicon's Bun font color nearly matches the light icon plate, so preserve
  // the glyph while using a darker Bun-inspired brown with accessible contrast.
  output.push(".devicon-bun-plain.colored{color:#6b4423}");
  await writeFile(destination, `${output.join("\n")}\n`);
}

async function withPublishLock(task) {
  const release = await properLockfile.lock(PUBLIC, {
    lockfilePath: PUBLISH_LOCK,
    stale: 10_000,
    update: 2_000,
    retries: {
      retries: 48,
      factor: 1.2,
      minTimeout: 10,
      maxTimeout: 250,
      randomize: true,
    },
  });
  let result;
  let taskError;
  try {
    result = await task();
  } catch (error) {
    taskError = error;
  } finally {
    await release();
  }
  if (taskError) throw taskError;
  return result;
}

async function publishRuntimeAssets(staging, manifest) {
  const assets = [
    ...Object.values(manifest.tech).map((entry) => entry.file.replace(/^icons\//, "")),
    ...Object.values(manifest.ui).map((entry) => entry.file.replace(/^icons\//, "")),
  ];
  const expected = new Set(["manifest.json", ...assets]);

  for (const relative of assets) {
    await atomicReplaceIfChanged(join(OUTPUT, relative), await readFile(join(staging, relative)));
  }
  return expected;
}

async function atomicReplaceIfChanged(destination, contents) {
  try {
    const current = await readFile(destination);
    if (current.equals(contents)) return;
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }

  await atomicReplace(destination, contents);
}

async function atomicReplace(destination, contents) {
  await mkdir(dirname(destination), { recursive: true });
  const temporary = join(dirname(destination), `.${basename(destination)}.${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, contents);
    await rename(temporary, destination);
  } finally {
    await rm(temporary, { force: true });
  }
}

async function cleanupStaleStagingDirectories() {
  const entries = await readdir(PUBLIC, { withFileTypes: true });
  const now = Date.now();
  for (const entry of entries) {
    if (!entry.isDirectory() || !/^\.icons-build-[a-zA-Z0-9_-]+$/.test(entry.name)) continue;
    const path = join(PUBLIC, entry.name);
    try {
      const metadata = await stat(path);
      if (now - metadata.mtimeMs > STAGING_STALE_MS) {
        await rm(path, { recursive: true, force: true });
      }
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
  }
}

async function removeStaleRuntimeFiles(expected) {
  for (const directory of ["tech", "ui"]) {
    const path = join(OUTPUT, directory);
    let entries;
    try {
      entries = await readdir(path, { withFileTypes: true });
    } catch (error) {
      if (error?.code === "ENOENT") continue;
      throw error;
    }
    for (const entry of entries) {
      const relative = `${directory}/${entry.name}`;
      if (!entry.isFile() || !expected.has(relative)) {
        await rm(join(path, entry.name), { recursive: true, force: true });
      }
    }
  }

  const rootEntries = await readdir(OUTPUT, { withFileTypes: true });
  for (const entry of rootEntries) {
    if ((entry.isDirectory() && (entry.name === "tech" || entry.name === "ui"))
      || (entry.isFile() && expected.has(entry.name))) continue;
    await rm(join(OUTPUT, entry.name), { recursive: true, force: true });
  }
}

async function verifyPackageVersions() {
  for (const [name, expected] of Object.entries(EXPECTED_VERSIONS)) {
    const packageJson = JSON.parse(await readFile(join(ROOT, "node_modules", name, "package.json"), "utf8"));
    if (packageJson.version !== expected) throw new Error(`${name}: expected ${expected}, found ${packageJson.version}`);
  }
}

async function rasterise(svg, destination, size) {
  const input = Buffer.isBuffer(svg) ? svg : Buffer.from(svg);
  await sharp(input, { density: 384 }).resize(size, size, { fit: "contain" }).png({ compressionLevel: 9 }).toFile(destination);
}

async function lucideSvg(slug) {
  const source = join(ROOT, "node_modules", "lucide-static", "icons", `${slug}.svg`);
  await access(source, fsConstants.R_OK);
  return readFile(source, "utf8");
}

function simpleIconSvg(slug) {
  const icon = SIMPLE_ICONS.get(slug);
  if (!icon) throw new Error(`Simple Icon not found: ${slug}`);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#${icon.hex}"><path d="${icon.path}"/></svg>`;
}

function asWhiteMask(svg) {
  return String(svg).replaceAll("currentColor", "#ffffff")
    .replace(/fill="#[0-9a-fA-F]{3,8}"/g, 'fill="#ffffff"')
    .replace(/stroke="#[0-9a-fA-F]{3,8}"/g, 'stroke="#ffffff"');
}

function serviceSvg() {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 192 192"><rect width="192" height="192" rx="42" fill="#202938"/><g fill="none" stroke="#b7c2cf" stroke-width="10"><rect x="35" y="35" width="122" height="31" rx="12"/><rect x="35" y="81" width="122" height="31" rx="12"/><rect x="35" y="127" width="122" height="31" rx="12"/></g><g fill="#22d3ee"><circle cx="133" cy="50.5" r="6"/><circle cx="133" cy="96.5" r="6"/><circle cx="133" cy="142.5" r="6"/></g></svg>`;
}

async function verifyManifest(root, manifest) {
  for (const entry of [...Object.values(manifest.tech), ...Object.values(manifest.ui)]) {
    const relative = entry.file.replace(/^icons\//, "");
    const path = join(root, relative);
    await access(path, fsConstants.R_OK);
    const metadata = await sharp(path).metadata();
    const expected = relative.startsWith("tech/") ? TECH_SIZE : UI_SIZE;
    if (metadata.format !== "png" || metadata.width !== expected || metadata.height !== expected || !metadata.hasAlpha) throw new Error(`Invalid generated icon: ${entry.file}`);
  }
}

await main();
