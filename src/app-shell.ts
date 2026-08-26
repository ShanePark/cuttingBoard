import { uiIcon } from "./icons";

export function renderAppShell(): string {
  return `
  <div class="app-shell">
    <header class="toolbar">
      <nav class="tabs" aria-label="Workspace">
        <button class="tab is-active" type="button" data-tab="services" aria-label="Services" title="Services">${uiIcon("power", 18)}<span class="tab-label">Services</span><span class="tab-count" id="services-count">0</span></button>
        <button class="tab" type="button" data-tab="docker" aria-label="Docker" title="Docker">${uiIcon("docker", 18)}<span class="tab-label">Docker</span><span class="tab-count" id="docker-count">0</span></button>
        <button class="tab" type="button" data-tab="launch" aria-label="Launch" title="Launch">${uiIcon("play", 18)}<span class="tab-label">Launch</span><span class="tab-count" id="launch-count">0</span></button>
      </nav>
      <button class="gear-button" type="button" data-action="settings" aria-label="Settings" title="Settings">${uiIcon("settings", 18)}</button>
    </header>
    <main id="workspace" class="workspace" aria-live="polite"></main>
    <nav id="bottom-tabs" class="bottom-tabs" aria-label="Panels"></nav>
    <footer id="status-footer" class="footer" hidden><span id="app-status"></span></footer>
  </div>
  <div id="modal-root"></div>
  <div id="toast-root" aria-live="assertive"></div>
`;
}
