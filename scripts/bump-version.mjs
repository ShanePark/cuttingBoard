#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const TOP_LEVEL_JSON_VERSION = /^(  "version": ")[^"]+/gm;

// Each pattern captures the text before one version value and must match exactly once.
const VERSION_FIELDS = [
  ["package.json", TOP_LEVEL_JSON_VERSION],
  ["package-lock.json", TOP_LEVEL_JSON_VERSION],
  ["package-lock.json", /^(    "": \{\n      "name": "cutting-board",\n      "version": ")[^"]+/gm],
  ["src-tauri/tauri.conf.json", TOP_LEVEL_JSON_VERSION],
  ["src-tauri/Cargo.toml", /^(\[package\]\nname = "cutting-board"\nversion = ")[^"]+/gm],
  ["src-tauri/Cargo.lock", /^(\[\[package\]\]\nname = "cutting-board"\nversion = ")[^"]+/gm]
];

/**
 * Set the application version in every version file, writing nothing unless
 * every field is found.
 *
 * @param {string} version
 * @param {string} root
 * @returns {string[]} The updated files, relative to root.
 */
export function bumpVersion(version, root = ROOT) {
  if (!VERSION.test(version)) {
    throw new Error(`Expected a version in x.y.z form, got "${version}".`);
  }

  const contents = new Map();
  for (const [file, pattern] of VERSION_FIELDS) {
    const text = contents.get(file) ?? readFileSync(join(root, file), "utf8");
    const matches = [...text.matchAll(pattern)].length;
    if (matches !== 1) {
      throw new Error(`Expected one match of ${pattern} in ${file}, found ${matches}.`);
    }
    contents.set(file, text.replace(pattern, (_match, prefix) => `${prefix}${version}`));
  }

  for (const [file, text] of contents) writeFileSync(join(root, file), text);
  return [...contents.keys()];
}

function run() {
  const args = process.argv.slice(2);
  if (args.length !== 1) {
    console.error("Usage: npm run bump-version -- <x.y.z>");
    process.exitCode = 1;
    return;
  }

  try {
    for (const file of bumpVersion(args[0])) console.log(`Updated ${file}`);
  } catch (error) {
    console.error(`Cannot bump the version: ${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) run();
