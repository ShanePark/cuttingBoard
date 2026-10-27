import { getCurrentWindow } from "@tauri-apps/api/window";
import { uiIcon } from "./icons";

export type WindowPlatform = "macos" | "linux" | "windows" | "other";

// The window chrome is chosen per platform in tauri.linux.conf.json and tauri.macos.conf.json:
// Linux has no native decorations and draws its own controls, macOS keeps its traffic lights
// over the toolbar, and Windows keeps the native title bar.
export function detectWindowPlatform(userAgent: string): WindowPlatform {
  if (/Macintosh|Mac OS X/.test(userAgent)) return "macos";
  if (/Windows/.test(userAgent)) return "windows";
  if (/Linux|X11/.test(userAgent)) return "linux";
  return "other";
}

export function renderWindowControls(platform: WindowPlatform): string {
  if (platform !== "linux") return "";
  return `<div class="window-controls" role="group" aria-label="Window">
          <button class="window-control" type="button" data-action="window-minimize" aria-label="Minimize" title="Minimize">${uiIcon("minimize", 16)}</button>
          <button class="window-control" type="button" data-action="window-toggle-maximize" aria-label="Maximize" title="Maximize">${uiIcon("maximize", 14)}</button>
          <button class="window-control window-close" type="button" data-action="window-close" aria-label="Close" title="Close">${uiIcon("close", 16)}</button>
        </div>`;
}

export async function runWindowAction(action: string): Promise<void> {
  const appWindow = getCurrentWindow();
  if (action === "window-minimize") await appWindow.minimize();
  else if (action === "window-toggle-maximize") await appWindow.toggleMaximize();
  // close() raises the same CloseRequested event as the window manager's close.
  else if (action === "window-close") await appWindow.close();
}

export async function trackMaximizedState(container: ParentNode): Promise<void> {
  const button = container.querySelector<HTMLButtonElement>('[data-action="window-toggle-maximize"]');
  if (!button) return;
  const appWindow = getCurrentWindow();
  const sync = async (): Promise<void> => {
    const maximized = await appWindow.isMaximized();
    if (button.dataset.maximized === String(maximized)) return;
    const label = maximized ? "Restore" : "Maximize";
    button.dataset.maximized = String(maximized);
    button.setAttribute("aria-label", label);
    button.title = label;
    button.innerHTML = uiIcon(maximized ? "restore" : "maximize", 14);
  };
  await appWindow.onResized(() => void sync().catch(() => undefined));
  await sync();
}
