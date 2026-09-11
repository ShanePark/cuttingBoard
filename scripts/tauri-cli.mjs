#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const TAURI_CLI = resolve(ROOT, "node_modules/@tauri-apps/cli/tauri.js");
const SIGNING_IDENTITY_ENV = "APPLE_SIGNING_IDENTITY";
const SIGNING_IDENTITY_PREFIXES = [
  "Developer ID Application:",
  "Apple Development:"
];

/**
 * Parse the identity names emitted by `security find-identity`.
 *
 * @param {string} output
 * @returns {string[]}
 */
export function parseSigningIdentities(output) {
  return output
    .split(/\r?\n/)
    .map((line) => line.match(/^\s*\d+\)\s+[0-9A-F]{40}\s+"([^"]+)"\s*$/i)?.[1])
    .filter((identity) => identity !== undefined && SIGNING_IDENTITY_PREFIXES.some((prefix) => identity.startsWith(prefix)));
}

/**
 * Return whether these arguments produce a signed macOS app bundle.
 *
 * @param {string[]} args
 * @param {string} platform
 * @returns {boolean}
 */
export function isMacosBundleBuild(args, platform = process.platform) {
  if (
    platform !== "darwin"
    || args[0] !== "build"
    || args.includes("--no-bundle")
    || args.includes("--no-sign")
    || args.some((arg) => ["-h", "--help", "-V", "--version"].includes(arg))
  ) {
    return false;
  }
  return true;
}

/**
 * Resolve a stable local signing identity. An explicit identity wins; when it
 * is omitted, a single preferred keychain identity is selected automatically.
 *
 * @param {{ env?: Record<string, string | undefined>, securityOutput?: string, securityError?: string, securityStatus?: number }} options
 * @returns {string}
 */
export function resolveSigningIdentity({
  env = process.env,
  securityOutput,
  securityError,
  securityStatus
} = {}) {
  const explicit = env[SIGNING_IDENTITY_ENV]?.trim();
  if (explicit) {
    if (explicit === "-") {
      throw new Error(`${SIGNING_IDENTITY_ENV}=- requests ad-hoc signing, which would make macOS privacy permissions recur after every update.`);
    }
    return explicit;
  }

  const result = securityOutput === undefined
    ? spawnSync("/usr/bin/security", ["find-identity", "-v", "-p", "codesigning"], { encoding: "utf8" })
    : { stdout: securityOutput, stderr: securityError ?? "", status: securityStatus ?? 0 };
  if (result.status !== 0) {
    throw new Error(`Could not inspect macOS code-signing identities: ${(result.stderr || "security exited unsuccessfully").trim()}`);
  }

  const identities = parseSigningIdentities(result.stdout);
  for (const prefix of SIGNING_IDENTITY_PREFIXES) {
    const matches = identities.filter((identity) => identity.startsWith(prefix));
    if (matches.length === 1) return matches[0];
    if (matches.length > 1) {
      throw new Error(`Multiple macOS code-signing identities match ${prefix} Set ${SIGNING_IDENTITY_ENV} explicitly before building.`);
    }
  }
  throw new Error(`No usable macOS code-signing identity was found. Install a Developer ID Application or Apple Development certificate, or set ${SIGNING_IDENTITY_ENV} explicitly.`);
}

/**
 * @param {string[]} args
 * @param {Record<string, string | undefined>} env
 * @param {string} platform
 * @returns {string[]}
 */
export function signedBuildArgs(args, env = process.env, platform = process.platform) {
  if (!isMacosBundleBuild(args, platform)) return args;
  const identity = resolveSigningIdentity({ env });
  return [...args, "--config", JSON.stringify({ bundle: { macOS: { signingIdentity: identity } } })];
}

function run() {
  const args = process.argv.slice(2);
  let forwarded;
  try {
    forwarded = signedBuildArgs(args);
  } catch (error) {
    console.error(`Cannot build a signed macOS app: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
    return;
  }

  const child = spawn(process.execPath, [TAURI_CLI, ...forwarded], { stdio: "inherit", env: process.env });
  child.on("error", (error) => {
    console.error(`Could not start the Tauri CLI: ${error.message}`);
    process.exitCode = 1;
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code ?? 1;
    }
  });
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) run();
