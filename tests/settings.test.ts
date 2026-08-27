import assert from "node:assert/strict";
import test from "node:test";
import { updateSettingsFromRadio } from "../src/settings.ts";
import type { UiSettings } from "../src/types.ts";

const settings: UiSettings = {
  theme_mode: "dark",
  scan_interval_ms: 2000,
  window_width: 1280,
  window_height: 800,
  window_x: 40,
  window_y: 80,
  window_geometry_logical: true
};

test("updates one selected setting while preserving window settings", () => {
  const updated = updateSettingsFromRadio(settings, "theme_mode", "light");

  assert.deepEqual(updated, {
    ...settings,
    theme_mode: "light"
  });
});

test("normalizes selected scan intervals to the supported range", () => {
  assert.equal(updateSettingsFromRadio(settings, "scan_interval_ms", "30000").scan_interval_ms, 30000);
  assert.equal(updateSettingsFromRadio(settings, "scan_interval_ms", "100").scan_interval_ms, 500);
  assert.equal(updateSettingsFromRadio(settings, "scan_interval_ms", "120000").scan_interval_ms, 60000);
});

test("rejects unknown settings fields", () => {
  assert.throws(() => updateSettingsFromRadio(settings, "unknown", "value"), /Invalid settings option/);
});
