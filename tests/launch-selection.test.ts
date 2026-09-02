import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const launchDom = readFileSync(new URL("../src/launch-dom.ts", import.meta.url), "utf8");
const rendering = readFileSync(new URL("../src/launch-rendering.ts", import.meta.url), "utf8");

test("selecting a launch task patches the view in place and falls back to a full render", () => {
  const selectTask = main.slice(main.indexOf("function selectTask("), main.indexOf("function selectService("));
  assert.match(main, /import \{ patchLaunchSelection \} from "\.\/launch-dom"/);
  assert.match(selectTask, /patchLaunchSelection\(workspaceElement, selectedTaskKey, renderLaunchConsole\(ref, launchConsoleRenderingContext\(\)\)\)/);
  assert.match(selectTask, /consoleController\.restoreLaunchConsoleScroll\(\)/);
  assert.match(selectTask, /renderLaunch\(true\)/);
  assert.match(selectTask, /if \(focus\) focusTaskRow\(profileId, taskName\)/);
});

test("the selection patch toggles the card highlight and swaps the console section", () => {
  assert.match(launchDom, /export function patchLaunchSelection\(workspace: HTMLElement, selectedTaskKey: string \| null, consoleMarkup: string\): boolean/);
  assert.match(launchDom, /querySelector<HTMLElement>\("\.launch-view"\)/);
  assert.match(launchDom, /querySelectorAll<HTMLElement>\("\.task-card"\)/);
  assert.match(launchDom, /classList\.toggle\("is-selected", selected\)/);
  assert.match(launchDom, /setAttribute\("aria-current", ariaCurrent\)/);
  assert.match(launchDom, /\.outerHTML = consoleMarkup/);
});

test("an external task without a log file explains how output can appear", () => {
  assert.match(rendering, /<strong>\$\{snapshot\?\.external_log_path \? "Waiting for external output" : "Output unavailable"\}<\/strong>/);
  assert.match(rendering, /logging\.file\.name/);
  assert.doesNotMatch(rendering, /External process output is not captured\./);
});

test("the console log renders without font ligatures so a large tail lays out quickly", () => {
  const css = readFileSync(new URL("../src/styles/console.css", import.meta.url), "utf8");

  assert.match(css, /\.console-log \{[^}]*font-variant-ligatures: none;/);
});
