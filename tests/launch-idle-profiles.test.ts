import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { launchProfileIsIdle } from "../src/launch-state.ts";

const rendering = readFileSync(new URL("../src/launch-rendering.ts", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/styles/launch.css", import.meta.url), "utf8");

test("a profile is idle only when it has tasks and none of them is alive", () => {
  assert.equal(launchProfileIsIdle(["stopped", "stopped"]), true);
  assert.equal(launchProfileIsIdle(["stopped", "failed"]), true);
  assert.equal(launchProfileIsIdle(["stopped", "running"]), false);
  assert.equal(launchProfileIsIdle(["failed", "starting"]), false);
  assert.equal(launchProfileIsIdle(["stopping"]), false);
  assert.equal(launchProfileIsIdle(["external"]), false);
  assert.equal(launchProfileIsIdle([]), false);
});

test("an idle profile is marked on its section and labelled Stopped in its header", () => {
  assert.match(rendering, /const idle = launchProfileIsIdle\(snapshots\.map\(\(snapshot\) => snapshot\?\.state \?\? "stopped"\)\)/);
  assert.match(rendering, /class="launch-profile service-section\$\{idle \? " is-idle" : ""\}"/);
  assert.match(rendering, /\$\{renderGroupCount\(profile\.tasks\.length\)\}\$\{idle \? `<span class="section-state">Stopped<\/span>` : ""\}<\/h2>/);
});

test("idle profiles and stopped task cards carry their own muted styling", () => {
  assert.match(styles, /^\.launch-profile\.is-idle \{/m);
  assert.match(styles, /^\.section-state \{/m);
  assert.match(styles, /^\.task-card\.state-stopped:not\(\.is-selected\) \{ --task-idle-surface:/m);
  assert.match(styles, /^\.task-card\.state-stopped \.icon-well \{ border-color: var\(--hairline\); opacity: \.68; \}/m);
  assert.match(styles, /^\.task-card\.state-stopped \.icon-well \.tech-icon \{ filter: saturate\(\.35\); \}/m);
  assert.match(styles, /^\.task-card \.metric-state\.state-stopped \{ color: var\(--text-dim\); \}/m);
});
