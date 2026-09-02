import { uiIcon } from "./icons";
import { escapeHtml as h } from "./html";
import {
  renderContainerDetails,
  renderContainerGroupDetails,
  renderGroupDetails,
  renderInfoDetails,
  renderProfileDetails,
  renderServiceDetails,
  renderTaskDetails,
  infoCopy
} from "./modal-content";
import { openModal } from "./modal";
import { matchedServiceForTask } from "./launch-rendering";
import type {
  AppInfo,
  ContainerInfo,
  LaunchProfile,
  LaunchTask,
  ManagedTaskSnapshot,
  ServiceSnapshot,
  ThemeMode,
  UiSettings
} from "./types";
import type { ServiceGroup } from "./presentation";

export const SOURCE_URL = "https://github.com/ShanePark/cuttingBoard";

export type ModalFormsContext = {
  openModal: typeof openModal;
  getSettings: () => UiSettings;
  getAppInfo: () => AppInfo | null;
  getContainers: () => readonly ContainerInfo[];
  getServices: () => readonly ServiceSnapshot[];
  snapshotFor: (profileId: string, taskName: string) => ManagedTaskSnapshot | undefined;
  blocksEditing: (profile: LaunchProfile) => boolean;
};

export type ModalFormsController = ReturnType<typeof createModalForms>;

export function createModalForms(context: ModalFormsContext) {
  function showGroupDetails(group: ServiceGroup): void {
    context.openModal(group.name, renderGroupDetails(group, [...context.getContainers()]));
  }

  function showContainerGroupDetails(groupName: string): void {
    const containers = context.getContainers().filter((container) => (container.compose_project ?? "Standalone containers") === groupName);
    context.openModal(groupName, renderContainerGroupDetails(containers));
  }

  function showProfileDetails(profile: LaunchProfile): void {
    context.openModal(profile.name, renderProfileDetails(profile));
  }

  function showTaskDetails(profile: LaunchProfile, taskName: string): void {
    const task = profile.tasks.find((item) => item.name === taskName);
    if (!task) throw new Error("The launch task no longer exists.");
    const snapshot = context.snapshotFor(profile.id, taskName);
    const matchedService = matchedServiceForTask(profile, task, context.getServices());
    context.openModal(task.name, renderTaskDetails(profile, task, snapshot, matchedService, context.getContainers()));
  }

  function showInfo(kind: string): void {
    const selected = infoCopy(kind);
    context.openModal(selected.title, renderInfoDetails(selected.message));
  }

  function toggleSettingsInfo(button: HTMLElement): void {
    const targetId = required(button.dataset.infoTarget);
    const info = document.getElementById(targetId);
    if (!info) throw new Error("The settings explanation is unavailable.");
    const expanded = info.hidden;
    info.hidden = !expanded;
    button.setAttribute("aria-expanded", String(expanded));
  }

  function showServiceDetails(service: ServiceSnapshot): void {
    context.openModal(service.display_name, renderServiceDetails(service));
  }

  function showContainerDetails(container: ContainerInfo): void {
    context.openModal(container.name, renderContainerDetails(container));
  }

  function showSettings(): void {
    const settings = context.getSettings();
    const appInfo = context.getAppInfo();
    const intervalChoices = [...new Set([1000, 2000, 5000, 10000, 30000, settings.scan_interval_ms])].sort((a, b) => a - b);
    context.openModal("Settings", `<form id="settings-form" class="settings-form" onsubmit="return false">
    <section class="settings-section" aria-labelledby="appearance-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("theme", 20)}</span><div class="settings-heading-copy"><h3 id="appearance-heading">Appearance</h3></div><button class="info-button icon-only-button settings-info-button" type="button" data-action="toggle-settings-info" data-info-kind="appearance" data-info-target="settings-info-appearance" aria-expanded="false" aria-controls="settings-info-appearance" aria-label="About appearance settings" title="About appearance settings">${uiIcon("info", 16)}</button></div>
      <p id="settings-info-appearance" class="settings-inline-info" hidden>Choose whether Cutting Board follows the system theme or always uses a light or dark appearance.</p>
      <fieldset class="choice-fieldset"><legend class="sr-only">Theme</legend><div class="theme-options">
        ${themeChoice("system", "System", "Follow your device", settings.theme_mode)}${themeChoice("light", "Light", "Bright and clear", settings.theme_mode)}${themeChoice("dark", "Dark", "Easy on the eyes", settings.theme_mode)}
      </div></fieldset>
    </section>
    <section class="settings-section" aria-labelledby="scanning-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("scan", 20)}</span><div class="settings-heading-copy"><h3 id="scanning-heading">Scanning</h3></div><button class="info-button icon-only-button settings-info-button" type="button" data-action="toggle-settings-info" data-info-kind="scanning" data-info-target="settings-info-scanning" aria-expanded="false" aria-controls="settings-info-scanning" aria-label="About scanning settings" title="About scanning settings">${uiIcon("info", 16)}</button></div>
      <p id="settings-info-scanning" class="settings-inline-info" hidden>This controls how often Cutting Board refreshes the list of running local services. A longer interval uses less CPU.</p>
      <fieldset class="choice-fieldset interval-control"><legend class="sr-only">Scan interval</legend><div class="interval-options">
          ${intervalChoices.map((value) => `<label class="interval-choice"><input type="radio" name="scan_interval_ms" value="${value}" ${settings.scan_interval_ms === value ? "checked" : ""}><span>${h(formatInterval(value))}</span></label>`).join("")}
        </div></fieldset>
    </section>
    <section class="settings-section" aria-labelledby="about-heading">
      <div class="settings-section-heading"><span class="settings-heading-icon">${uiIcon("info", 20)}</span><div class="settings-heading-copy"><h3 id="about-heading">About</h3><p>Cutting Board${appInfo?.version ? ` ${h(appInfo.version)}` : ""}</p></div></div>
      <button class="source-link" type="button" data-action="open-source" aria-label="View Cutting Board source code on GitHub" title="Open source on GitHub">${uiIcon("github", 20)}<span><strong>View source on GitHub</strong><small>${h(SOURCE_URL.replace("https://", ""))}</small></span>${uiIcon("external", 16)}</button>
      <p class="settings-note">Settings are stored locally on this device. Cutting Board does not collect telemetry and does not start automatically at login.</p>
    </section>
    <div class="modal-actions"><button class="primary-button" type="button" data-action="close-modal">Done</button></div>
  </form>`);
  }

  function showProfileEditor(profile: LaunchProfile | null): void {
    const tasks = profile?.tasks.length ? profile.tasks : [{ name: "Backend", cwd: ".", command: "", expected_port: null }];
    const active = profile ? context.blocksEditing(profile) : false;
    const readOnly = active ? "readonly" : "";
    const disabled = active ? "disabled" : "";
    const appInfo = context.getAppInfo();
    const deleteAction = profile
      ? `<button class="secondary-button danger-button icon-button-label profile-delete-action" type="button" data-action="delete-profile" data-profile-id="${h(profile.id)}" aria-label="Delete ${h(profile.name)}" title="Delete profile" ${appInfo?.demo ? "disabled" : ""}>${uiIcon("trash", 15)} Delete</button>`
      : "";
    const activeNote = active ? `<p class="form-note">This profile is read-only while any task is running. Stop every task before editing or saving it. Deleting it is still allowed and stops the tasks Cutting Board started.</p>` : "";
    context.openModal(profile ? "Edit Launch Profile" : "Add Launch Profile", `<form id="profile-form" class="form-stack${active ? " is-readonly" : ""}" onsubmit="return false">
    <label>Profile name<input name="name" required maxlength="80" value="${h(profile?.name ?? "")}" ${readOnly}></label>
    <label>Project root<div class="field-with-button"><input id="project-root" name="project_root" required value="${h(profile?.project_root ?? "")}" ${readOnly}><button class="secondary-button" type="button" data-action="choose-root" ${disabled}>Choose</button></div></label>
    <div class="task-editor-heading"><strong>Tasks</strong><button class="quiet-button icon-button-label" type="button" data-action="add-task-row" ${disabled}>${uiIcon("plus", 13)} Add task</button></div>
    <div id="task-editors">${tasks.map((task) => renderTaskEditor(task, active)).join("")}</div>
    ${activeNote}<div class="modal-actions">${deleteAction}<button class="secondary-button" type="button" data-action="close-modal">Cancel</button><button class="primary-button" type="button" data-action="save-profile" ${profile ? `data-profile-id="${h(profile.id)}"` : ""} ${appInfo?.demo || active ? "disabled" : ""}>Save</button></div>
  </form>`);
  }

  function addTaskRow(): void {
    document.querySelector("#task-editors")?.insertAdjacentHTML("beforeend", renderTaskEditor({ name: "", cwd: ".", command: "", expected_port: null }));
  }

  return {
    showGroupDetails,
    showContainerGroupDetails,
    showProfileDetails,
    showTaskDetails,
    showInfo,
    toggleSettingsInfo,
    showServiceDetails,
    showContainerDetails,
    showSettings,
    showProfileEditor,
    addTaskRow,
    hasProfileForm,
    readProfileForm
  };
}

export function hasProfileForm(): boolean {
  return Boolean(document.querySelector<HTMLFormElement>("#profile-form"));
}

export function renderTaskEditor(task: LaunchTask, readOnly = false): string {
  const inputState = readOnly ? "readonly" : "";
  const buttonState = readOnly ? "disabled" : "";
  const container = task.container?.trim() ?? "";
  // A container task is answered by Docker, so it carries the container it starts instead of a
  // command. The field stays read-only: the binding comes from the Docker listing, not by hand.
  const commandField = container
    ? `<label>Container<input data-task-field="container" value="${h(container)}" readonly></label>`
    : `<label>Command<input data-task-field="command" required value="${h(task.command)}" ${inputState}></label>`;
  return `<fieldset class="task-editor"><button class="remove-task" type="button" data-action="remove-task-row" aria-label="Remove task" title="Remove task" ${buttonState}>${uiIcon("trash", 15)}</button><label>Name<input data-task-field="name" required value="${h(task.name)}" ${inputState}></label><label>Working directory<input data-task-field="cwd" required value="${h(task.cwd)}" ${inputState}></label>${commandField}<label>Expected port (optional)<input data-task-field="expected_port" type="number" min="1" max="65535" value="${task.expected_port ?? ""}" ${inputState}></label></fieldset>`;
}

export function readProfileForm(id: string | null): LaunchProfile | null {
  const form = document.querySelector<HTMLFormElement>("#profile-form");
  if (!form) return null;
  const data = new FormData(form);
  const tasks = [...form.querySelectorAll<HTMLElement>(".task-editor")].map((row) => {
    const value = (name: string): string => row.querySelector<HTMLInputElement>(`[data-task-field='${name}']`)?.value.trim() ?? "";
    const portValue = value("expected_port");
    const container = value("container");
    return { name: value("name"), cwd: value("cwd"), command: value("command"), expected_port: portValue ? Number(portValue) : null, container: container || null } satisfies LaunchTask;
  });
  const profile: LaunchProfile = {
    id: id ?? crypto.randomUUID().replaceAll("-", ""),
    name: String(data.get("name") ?? "").trim(),
    project_root: String(data.get("project_root") ?? "").trim(),
    tasks
  };
  if (!profile.name || !profile.project_root || !tasks.length || tasks.some((task) => !task.name || !task.cwd || (!task.command && !task.container))) throw new Error("Complete every profile and task field.");
  return profile;
}

export function themeChoice(value: ThemeMode, label: string, description: string, selectedTheme: ThemeMode = "dark"): string {
  return `<label class="theme-choice" title="${h(description)}"><input type="radio" name="theme_mode" value="${value}" ${selectedTheme === value ? "checked" : ""}><span class="theme-preview theme-preview-${value}" aria-hidden="true"><i></i><i></i><i></i></span><span class="theme-copy"><strong>${label}</strong></span><span class="choice-check">${uiIcon("check", 14)}</span></label>`;
}

export function formatInterval(milliseconds: number): string {
  const seconds = milliseconds / 1000;
  return `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)} sec`;
}

function required(value: string | undefined): string {
  if (!value) throw new Error("Missing action identifier.");
  return value;
}
