import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { bumpVersion } from "../scripts/bump-version.mjs";

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const VERSION_FILES = ["package.json", "package-lock.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"];

function read(root: string, file: string): string {
  return readFileSync(join(root, file), "utf8");
}

function cargoVersion(text: string, header: string): string | undefined {
  return text.match(new RegExp(`^${header}\\nname = "cutting-board"\\nversion = "([^"]+)"`, "m"))?.[1];
}

function withVersionFileCopy(run: (root: string) => void): void {
  const root = mkdtempSync(join(tmpdir(), "cutting-board-version-"));
  try {
    for (const file of VERSION_FILES) cpSync(join(ROOT, file), join(root, file));
    run(root);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

/** The lines that differ between two texts with the same line count, as [before, after, previous line]. */
function changedLines(before: string, after: string): [string, string, string][] {
  const beforeLines = before.split("\n");
  const afterLines = after.split("\n");
  assert.equal(afterLines.length, beforeLines.length);
  return beforeLines.flatMap((line, index): [string, string, string][] =>
    line === afterLines[index] ? [] : [[line, afterLines[index]!, beforeLines[index - 1] ?? ""]]
  );
}

test("all version files agree", () => {
  const { version } = JSON.parse(read(ROOT, "package.json"));
  const lock = JSON.parse(read(ROOT, "package-lock.json"));
  assert.equal(lock.version, version);
  assert.equal(lock.packages[""].version, version);
  assert.equal(JSON.parse(read(ROOT, "src-tauri/tauri.conf.json")).version, version);
  assert.equal(cargoVersion(read(ROOT, "src-tauri/Cargo.toml"), "\\[package\\]"), version);
  assert.equal(cargoVersion(read(ROOT, "src-tauri/Cargo.lock"), "\\[\\[package\\]\\]"), version);
});

test("the changelog has an entry for the current version", () => {
  const { version } = JSON.parse(read(ROOT, "package.json"));
  const entry = new RegExp(`^## ${version.replaceAll(".", "\\.")} - \\d{4}-\\d{2}-\\d{2}$`, "m");
  assert.match(read(ROOT, "CHANGELOG.md"), entry);
});

test("bumping changes only the application's version lines", () => {
  withVersionFileCopy((root) => {
    const before = new Map(VERSION_FILES.map((file) => [file, read(root, file)]));
    assert.deepEqual(bumpVersion("9.8.7", root).sort(), [...VERSION_FILES].sort());

    const current = JSON.parse(before.get("package.json")!).version;
    const jsonLine = (indent: string, version: string) => `${indent}"version": "${version}",`;
    const tomlLine = (version: string) => `version = "${version}"`;
    const changed = (file: string) => changedLines(before.get(file)!, read(root, file));

    assert.deepEqual(changed("package.json").map(([old, next]) => [old, next]), [[jsonLine("  ", current), jsonLine("  ", "9.8.7")]]);
    assert.deepEqual(changed("src-tauri/tauri.conf.json").map(([old, next]) => [old, next]), [[jsonLine("  ", current), jsonLine("  ", "9.8.7")]]);
    assert.deepEqual(changed("package-lock.json").map(([old, next]) => [old, next]), [
      [jsonLine("  ", current), jsonLine("  ", "9.8.7")],
      [jsonLine("      ", current), jsonLine("      ", "9.8.7")]
    ]);
    assert.deepEqual(changed("src-tauri/Cargo.toml"), [[tomlLine(current), tomlLine("9.8.7"), 'name = "cutting-board"']]);
    assert.deepEqual(changed("src-tauri/Cargo.lock"), [[tomlLine(current), tomlLine("9.8.7"), 'name = "cutting-board"']]);
  });
});

test("rejects a version that is not x.y.z", () => {
  for (const version of ["", "1", "1.2", "1.2.3.4", "v1.2.3", "1.2.3-rc.1", "01.2.3", "1.2.3\n"]) {
    assert.throws(() => bumpVersion(version, "/nonexistent"), /x\.y\.z form/, JSON.stringify(version));
  }
});

test("writes nothing when a version field cannot be found", () => {
  withVersionFileCopy((root) => {
    writeFileSync(join(root, "src-tauri/Cargo.lock"), "# no packages\n");
    const before = VERSION_FILES.map((file) => read(root, file));
    assert.throws(() => bumpVersion("9.8.7", root), /src-tauri\/Cargo\.lock/);
    assert.deepEqual(VERSION_FILES.map((file) => read(root, file)), before);
  });
});
