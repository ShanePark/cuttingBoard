import type { ThemeMode, UiSettings } from "./types";

const THEME_MODES: readonly ThemeMode[] = ["dark", "light", "system"];

export function updateSettingsFromRadio(
  settings: UiSettings,
  name: string,
  value: string
): UiSettings {
  if (name === "theme_mode" && THEME_MODES.includes(value as ThemeMode)) {
    return { ...settings, theme_mode: value as ThemeMode };
  }

  if (name === "scan_interval_ms") {
    const interval = Number(value);
    return {
      ...settings,
      scan_interval_ms: Math.min(60000, Math.max(500, interval || 2000))
    };
  }

  throw new Error("Invalid settings option.");
}
