import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const apiSource = readFileSync(new URL("../src/api.ts", import.meta.url), "utf8");
const nativeSource = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const shellSource = readFileSync(new URL("../src/app-shell.ts", import.meta.url), "utf8");
const mainSource = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");
const metricsSource = readFileSync(new URL("../src-tauri/src/system_metrics.rs", import.meta.url), "utf8");

test("exposes a compact accessible system metrics indicator", () => {
  assert.match(shellSource, /id="system-metrics"[^>]*role="group"[^>]*aria-label="System resource usage"/);
  assert.doesNotMatch(shellSource, /id="system-metrics"[^>]*aria-live=/);
  assert.match(shellSource, /data-system-metric-value="cpu">—/);
  assert.match(shellSource, /data-system-metric-value="memory">—/);
  assert.match(shellSource, /CPU[\s\S]*system-metrics-separator[\s\S]*MEM/);
});

test("polls one native system metrics command every two seconds", () => {
  assert.match(apiSource, /invoke<SystemMetrics>\("system_metrics"\)/);
  assert.match(nativeSource, /system_metrics,/);
  assert.match(mainSource, /const SYSTEM_METRICS_POLL_INTERVAL = 2000/);
  assert.match(mainSource, /window\.setInterval\(\(\) => void refreshSystemMetrics\(\), SYSTEM_METRICS_POLL_INTERVAL\)/);
});

test("keeps CPU deltas on a reused native System and hides the first sample", () => {
  assert.match(metricsSource, /system: System/);
  assert.doesNotMatch(metricsSource, /System::new_all/);
  assert.match(metricsSource, /last_cpu_sample/);
  assert.match(metricsSource, /None/);
  assert.match(metricsSource, /average_cpu_percent\(&self\.system\)/);
});
