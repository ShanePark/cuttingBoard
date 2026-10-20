import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { launchProfileIsCollapsible, launchProfileIsExpanded, launchProfileIsFullyStopped, launchProfileIsIdle } from "../src/launch-state.ts";

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

test("only fully stopped profiles with several tasks collapse by default", () => {
  assert.equal(launchProfileIsFullyStopped(["stopped", "stopped"]), true);
  assert.equal(launchProfileIsFullyStopped(["stopped", "failed"]), false);
  assert.equal(launchProfileIsFullyStopped(["stopped", "running"]), false);
  assert.equal(launchProfileIsFullyStopped([]), false);
  assert.equal(launchProfileIsExpanded(["stopped", "stopped"], false), false);
  assert.equal(launchProfileIsExpanded(["stopped", "stopped"], true), true);
  assert.equal(launchProfileIsExpanded(["stopped", "failed"], false), true);
  assert.equal(launchProfileIsCollapsible(["stopped", "stopped"]), true);
  assert.equal(launchProfileIsCollapsible(["stopped"]), false);
  assert.equal(launchProfileIsExpanded(["stopped"], false), true);
});

test("an idle profile is marked on its section and labelled Stopped in its header", () => {
  assert.match(rendering, /const idle = launchProfileIsIdle\(states\)/);
  assert.match(rendering, /const collapsible = launchProfileIsCollapsible\(states\)/);
  assert.match(rendering, /class="launch-profile service-section\$\{idle \? " is-idle" : ""\}\$\{expanded \? "" : " is-collapsed"\}"/);
  assert.match(rendering, /const headerSummary = `\$\{renderGroupCount\(profile\.tasks\.length\)\}\$\{expanded && idle \? `<span class="section-state">Stopped<\/span>` : ""\}`/);
  assert.match(rendering, /\$\{context\.renderGroupTitle\([\s\S]+\)\}\$\{headerSummary\}<\/h2>/);
  assert.match(rendering, /data-action="toggle-profile"/);
  assert.match(rendering, /aria-controls="\$\{h\(taskListId\)\}" aria-expanded=/);
});

test("collapsed stopped profiles render a non-interactive card stack behind one header disclosure", () => {
  assert.match(rendering, /class="launch-profile-stack" style="--stack-depth:/);
  assert.match(rendering, /class="launch-profile-stack-cards" aria-hidden="true"/);
  assert.match(rendering, /renderTask\(profile, task, context, \{ interactive: false, stackIndex:/);
  assert.match(rendering, /class="launch-profile-stack-toggle launch-profile-toggle" \$\{toggleAttributes\} tabindex="-1" aria-hidden="true"/);
  assert.match(rendering, /const headerToggle = collapsible\s/);
  assert.match(rendering, /const cardAttributes = interactive\s*\n\s*\? ` data-action="select-task"/);
});

test("idle profiles and stopped task cards carry their own muted styling", () => {
  assert.match(styles, /^\.launch-profile\.is-idle \{/m);
  assert.match(styles, /^\.section-state \{/m);
  assert.match(styles, /^\.task-card\.state-stopped:not\(\.is-selected\) \{ --task-idle-surface:/m);
  assert.match(styles, /^\.task-card\.state-stopped \.icon-well \{ border-color: var\(--hairline\); opacity: \.68; \}/m);
  assert.match(styles, /^\.task-card\.state-stopped \.icon-well \.tech-icon \{ filter: saturate\(\.35\); \}/m);
  assert.match(styles, /^\.task-card \.metric-state\.state-stopped \{ color: var\(--text-dim\); \}/m);
  assert.match(styles, /^\.task-list\[hidden\] \{ display: none; \}/m);
  assert.match(styles, /^\.launch-profile-toggle \{/m);
  assert.match(styles, /^\.launch-profile-stack-card \{/m);
  assert.match(styles, /\.launch-profile-header-toggle\[aria-expanded="false"\] \.ui-icon \{ transform: rotate\(-90deg\); \}/);
  assert.match(styles, /\.launch-profile\.is-collapsed \.launch-profile-heading h2 \{/);
  assert.match(styles, /\.launch-profile\.is-collapsed \.launch-profile-heading \.group-title-button \{/);
  assert.match(styles, /\.launch-profile-stack:hover \.launch-profile-stack-card/);
  assert.match(styles, /@media \(prefers-reduced-motion: reduce\)/);
});
