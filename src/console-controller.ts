import { uiIcon, type UiIconName } from "./icons";
import { escapeHtml as h } from "./html";
import { patchConsoleOutput } from "./console-dom";
import {
  CONSOLE_BOTTOM_EPSILON,
  isConsoleAtBottom,
  scrollTopForConsoleUpdate
} from "./console-scroll";
import type { ContainerTab, DockerLogState } from "./docker-rendering";
import type { ServicesConsoleTarget } from "./services-rendering";

export type ConsoleOutputPatch = {
  markup: string;
  kind: string;
  log: string;
};

export type ConsoleContainerState = {
  selectedContainerId: string | null;
  logState: DockerLogState;
};

export type ConsoleControllerContext = {
  elements: {
    workspace: HTMLElement;
    bottomTabs: HTMLElement;
  };
  state: {
    selectedTaskKey: () => string | null;
    selectedTaskDomKey: () => string | null;
    selectedServiceId: () => string | null;
    servicesConsoleTarget: () => ServicesConsoleTarget | null;
    container: (tab: ContainerTab) => ConsoleContainerState;
  };
  render: {
    rerender: (force?: boolean) => void;
    service: (serviceId: string | null) => ConsoleOutputPatch;
    docker: (tab: ContainerTab, containerId: string | null) => ConsoleOutputPatch;
    launch: () => ConsoleOutputPatch | null;
  };
};

type BottomPanelId = "console";
type ScrollState = { key: string | null; top: number; restoring: boolean };
type ContainerScrollState = { containerId: string | null; top: number; restoring: boolean };

const MIN_CONSOLE_HEIGHT = 220;
const DEFAULT_CONSOLE_HEIGHT = 336;
const MIN_BOARD_HEIGHT = 140;
const CONSOLE_RESIZE_STEP = 24;
const BOTTOM_PANELS: ReadonlyArray<{ id: BottomPanelId; label: string; icon: UiIconName }> = [
  { id: "console", label: "Console", icon: "terminal" }
];

export class ConsoleController {
  private readonly context: ConsoleControllerContext;
  private activeBottomPanel: BottomPanelId | null = "console";
  private consoleHeight = DEFAULT_CONSOLE_HEIGHT;
  private consoleFollow = true;
  private readonly taskScroll: ScrollState = { key: null, top: 0, restoring: false };
  private readonly serviceScroll: ScrollState = { key: null, top: 0, restoring: false };
  private readonly containerScroll: Record<ContainerTab, ContainerScrollState> = {
    services: { containerId: null, top: 0, restoring: false },
    docker: { containerId: null, top: 0, restoring: false }
  };

  constructor(context: ConsoleControllerContext) {
    this.context = context;
  }

  activePanel(): BottomPanelId | null {
    return this.activeBottomPanel;
  }

  isPanelOpen(id: string): boolean {
    return this.activeBottomPanel === id;
  }

  renderBottomPanelTabs(): string {
    return BOTTOM_PANELS.map((panel) => {
      const open = this.activeBottomPanel === panel.id;
      const label = `${open ? "Hide" : "Show"} ${panel.label.toLowerCase()}`;
      return `<button class="bottom-tab${open ? " is-active" : ""}" type="button" data-action="toggle-bottom-panel" data-panel-id="${h(panel.id)}" aria-pressed="${open ? "true" : "false"}" aria-label="${h(label)}" title="${h(label)}">${uiIcon(panel.icon, 15)}<span class="tab-label">${h(panel.label)}</span>${uiIcon("chevronDown", 12, "bottom-tab-caret")}</button>`;
    }).join("");
  }

  toggleBottomPanel(id: string): void {
    const panel = BOTTOM_PANELS.find((item) => item.id === id);
    if (!panel) throw new Error("Unknown panel.");
    this.activeBottomPanel = this.activeBottomPanel === panel.id ? null : panel.id;
    this.context.elements.bottomTabs.innerHTML = this.renderBottomPanelTabs();
    this.context.render.rerender(true);
    this.applyConsoleHeight();
  }

  renderConsoleResizer(): string {
    return `<div class="console-resizer" role="separator" aria-orientation="horizontal" tabindex="0" aria-label="Resize console" aria-valuemin="${MIN_CONSOLE_HEIGHT}" aria-valuemax="${this.maxConsoleHeight()}" aria-valuenow="${this.appliedConsoleHeight()}" title="Drag to resize the console"></div>`;
  }

  renderConsoleJumpButton(): string {
    return `<button class="console-jump-bottom" type="button" data-action="jump-to-bottom" aria-label="Jump to bottom" title="Jump to bottom" hidden>${uiIcon("chevronDown", 20)}</button>`;
  }

  applyConsoleHeight(): void {
    const height = this.appliedConsoleHeight();
    document.documentElement.style.setProperty("--console-height", `${height}px`);
    const resizer = this.context.elements.workspace.querySelector<HTMLElement>(".console-resizer");
    if (resizer) {
      resizer.setAttribute("aria-valuenow", String(height));
      resizer.setAttribute("aria-valuemax", String(this.maxConsoleHeight()));
    }
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".console-output");
    if (!output) return;
    if (this.consoleFollow) output.scrollTop = output.scrollHeight;
    this.updateConsoleScrollAffordance(output);
  }

  setConsoleHeight(height: number): void {
    const next = Math.round(Math.min(Math.max(height, MIN_CONSOLE_HEIGHT), this.maxConsoleHeight()));
    if (next === this.consoleHeight) return;
    this.consoleHeight = next;
    this.applyConsoleHeight();
  }

  handleResizeStart(event: PointerEvent): void {
    if (event.button !== 0) return;
    const handle = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-resizer") : null;
    const consoleElement = handle?.closest<HTMLElement>(".launch-console");
    if (!handle || !consoleElement) return;
    event.preventDefault();
    handle.focus();
    const startY = event.clientY;
    const startHeight = consoleElement.offsetHeight;
    const resize = (move: PointerEvent): void => this.setConsoleHeight(startHeight + startY - move.clientY);
    const finish = (): void => {
      window.removeEventListener("pointermove", resize);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      document.body.classList.remove("is-console-resizing");
    };
    window.addEventListener("pointermove", resize);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
    document.body.classList.add("is-console-resizing");
  }

  handleResizeKey(event: KeyboardEvent): boolean {
    const resizer = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>(".console-resizer") : null;
    if (!resizer || (event.key !== "ArrowUp" && event.key !== "ArrowDown")) return false;
    event.preventDefault();
    this.setConsoleHeight(this.appliedConsoleHeight() + (event.key === "ArrowUp" ? CONSOLE_RESIZE_STEP : -CONSOLE_RESIZE_STEP));
    return true;
  }

  captureServicesConsoleState(): void {
    this.captureServiceConsoleState();
    this.captureDockerConsoleState();
  }

  restoreServicesConsoleState(): void {
    if (this.context.state.servicesConsoleTarget()?.kind === "container") this.restoreDockerConsoleScroll();
    else this.restoreServiceConsoleScroll();
  }

  captureLaunchConsoleState(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".launch-console:not(.docker-console):not(.docker-service-console) .console-output");
    if (!output) return;
    const taskKey = output.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
    if (taskKey !== this.context.state.selectedTaskDomKey()) return;
    this.taskScroll.key = this.context.state.selectedTaskKey();
    this.taskScroll.top = output.scrollTop;
  }

  restoreLaunchConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".launch-console:not(.docker-console):not(.docker-service-console) .console-output");
    if (!output) return;
    this.taskScroll.restoring = true;
    const selectedTaskKey = this.context.state.selectedTaskKey();
    const savedScrollTop = this.taskScroll.key === selectedTaskKey ? this.taskScroll.top : 0;
    output.scrollTop = scrollTopForConsoleUpdate(output, savedScrollTop, this.consoleFollow);
    this.taskScroll.key = selectedTaskKey;
    this.taskScroll.top = output.scrollTop;
    this.updateConsoleScrollAffordance(output);
    window.setTimeout(() => { this.taskScroll.restoring = false; }, 0);
  }

  captureDockerConsoleState(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".docker-console .console-output, .docker-service-console .console-output");
    if (!output) return;
    const consoleElement = output.closest<HTMLElement>(".docker-console, .docker-service-console");
    const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
    const state = this.context.state.container(tab);
    const containerId = consoleElement?.dataset.consoleContainerId || null;
    if (containerId !== state.selectedContainerId) return;
    const scroll = this.containerScroll[tab];
    scroll.containerId = state.selectedContainerId;
    scroll.top = output.scrollTop;
  }

  restoreDockerConsoleState(): void {
    this.restoreDockerConsoleScroll();
  }

  resetTaskSelection(key: string | null): void {
    this.taskScroll.key = key;
    this.taskScroll.top = 0;
  }

  resetServiceSelection(key: string | null): void {
    this.serviceScroll.key = key;
    this.serviceScroll.top = 0;
  }

  resetContainerSelection(tab: ContainerTab, containerId: string | null): void {
    const scroll = this.containerScroll[tab];
    scroll.containerId = containerId;
    scroll.top = 0;
  }

  updateServiceConsoleDom(): void {
    const consoleElement = this.context.elements.workspace.querySelector<HTMLElement>(".service-console");
    if (!consoleElement) return;
    if (consoleElement.dataset.consoleKind === "container") {
      const containerId = consoleElement.dataset.consoleContainerId || null;
      const servicesState = this.context.state.container("services");
      const target = this.context.state.servicesConsoleTarget();
      if (target?.kind !== "container" || containerId !== servicesState.selectedContainerId) return;
      const output = consoleElement.querySelector<HTMLElement>(".console-output");
      if (!output) return;
      this.patchOutput(output, this.context.render.docker("services", containerId));
      return;
    }
    const serviceId = consoleElement.dataset.consoleServiceId || null;
    const target = this.context.state.servicesConsoleTarget();
    if (target?.kind !== "service" || serviceId !== this.context.state.selectedServiceId()) return;
    const output = consoleElement.querySelector<HTMLElement>(".console-output");
    if (!output) return;
    this.patchOutput(output, this.context.render.service(serviceId));
  }

  updateDockerConsoleDom(): void {
    const consoleElement = this.context.elements.workspace.querySelector<HTMLElement>(".docker-console, .docker-service-console");
    if (!consoleElement) return;
    const tab: ContainerTab = consoleElement.classList.contains("docker-service-console") ? "services" : "docker";
    const state = this.context.state.container(tab);
    const containerId = consoleElement.dataset.consoleContainerId || null;
    if (containerId !== state.selectedContainerId) return;
    const output = consoleElement.querySelector<HTMLElement>(".console-output");
    if (!output) return;
    this.patchOutput(output, this.context.render.docker(tab, containerId));
  }

  updateLaunchConsoleDom(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".launch-view > .launch-console .console-output");
    if (!output) return;
    const patch = this.context.render.launch();
    if (!patch) return;
    this.patchOutput(output, patch);
  }

  jumpToBottom(button: HTMLElement): void {
    const output = button.closest<HTMLElement>(".console-output-shell")?.querySelector<HTMLElement>(".console-output");
    if (!output) return;
    this.consoleFollow = true;
    output.scrollTop = output.scrollHeight;
    const serviceConsole = output.closest<HTMLElement>(".service-console:not(.docker-service-console)");
    if (serviceConsole) {
      this.serviceScroll.key = this.context.state.selectedServiceId();
      this.serviceScroll.top = output.scrollTop;
    } else {
      const dockerConsole = output.closest<HTMLElement>(".docker-console, .docker-service-console");
      if (dockerConsole) {
        const tab: ContainerTab = dockerConsole.classList.contains("docker-service-console") ? "services" : "docker";
        const state = this.context.state.container(tab);
        const scroll = this.containerScroll[tab];
        scroll.containerId = state.selectedContainerId;
        scroll.top = output.scrollTop;
      } else {
        this.taskScroll.key = this.context.state.selectedTaskKey();
        this.taskScroll.top = output.scrollTop;
      }
    }
    this.updateConsoleScrollAffordance(output);
  }

  handleScroll(event: Event): void {
    const target = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-output") : null;
    if (!target) return;
    const serviceConsole = target.closest<HTMLElement>(".service-console:not(.docker-service-console)");
    if (serviceConsole) {
      const serviceId = serviceConsole.dataset.consoleServiceId || null;
      const servicesConsoleTarget = this.context.state.servicesConsoleTarget();
      if (servicesConsoleTarget?.kind !== "service" || serviceId !== this.context.state.selectedServiceId()) return;
      this.serviceScroll.key = this.context.state.selectedServiceId();
      this.serviceScroll.top = target.scrollTop;
      if (!this.serviceScroll.restoring) this.consoleFollow = isConsoleAtBottom(target, CONSOLE_BOTTOM_EPSILON);
      this.updateConsoleScrollAffordance(target);
      return;
    }
    const dockerConsole = target.closest<HTMLElement>(".docker-console, .docker-service-console");
    if (dockerConsole) {
      const tab: ContainerTab = dockerConsole.classList.contains("docker-service-console") ? "services" : "docker";
      const state = this.context.state.container(tab);
      const containerId = dockerConsole.dataset.consoleContainerId || null;
      const servicesConsoleTarget = this.context.state.servicesConsoleTarget();
      if (dockerConsole.classList.contains("docker-service-console") && servicesConsoleTarget?.kind !== "container") return;
      if (containerId !== state.selectedContainerId) return;
      const scroll = this.containerScroll[tab];
      scroll.containerId = state.selectedContainerId;
      scroll.top = target.scrollTop;
      if (!scroll.restoring) this.consoleFollow = isConsoleAtBottom(target, CONSOLE_BOTTOM_EPSILON);
      this.updateConsoleScrollAffordance(target);
      return;
    }
    const taskKey = target.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
    if (taskKey !== this.context.state.selectedTaskDomKey()) return;
    this.taskScroll.key = this.context.state.selectedTaskKey();
    this.taskScroll.top = target.scrollTop;
    if (!this.taskScroll.restoring) this.consoleFollow = isConsoleAtBottom(target, CONSOLE_BOTTOM_EPSILON);
    this.updateConsoleScrollAffordance(target);
  }

  private captureServiceConsoleState(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
    if (!output) return;
    const serviceId = output.closest<HTMLElement>(".service-console:not(.docker-service-console)")?.dataset.consoleServiceId || null;
    const target = this.context.state.servicesConsoleTarget();
    if (target?.kind !== "service" || serviceId !== this.context.state.selectedServiceId()) return;
    this.serviceScroll.key = this.context.state.selectedServiceId();
    this.serviceScroll.top = output.scrollTop;
  }

  private restoreServiceConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
    if (!output) return;
    this.serviceScroll.restoring = true;
    const selectedServiceId = this.context.state.selectedServiceId();
    const savedScrollTop = this.serviceScroll.key === selectedServiceId ? this.serviceScroll.top : 0;
    output.scrollTop = scrollTopForConsoleUpdate(output, savedScrollTop, this.consoleFollow);
    this.serviceScroll.key = selectedServiceId;
    this.serviceScroll.top = output.scrollTop;
    this.updateConsoleScrollAffordance(output);
    window.setTimeout(() => { this.serviceScroll.restoring = false; }, 0);
  }

  private restoreDockerConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".docker-console .console-output, .docker-service-console .console-output");
    if (!output) return;
    const consoleElement = output.closest<HTMLElement>(".docker-console, .docker-service-console");
    const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
    const state = this.context.state.container(tab);
    const scroll = this.containerScroll[tab];
    scroll.restoring = true;
    const savedScrollTop = scroll.containerId === state.selectedContainerId ? scroll.top : 0;
    output.scrollTop = scrollTopForConsoleUpdate(output, savedScrollTop, this.consoleFollow);
    scroll.containerId = state.selectedContainerId;
    scroll.top = output.scrollTop;
    this.updateConsoleScrollAffordance(output);
    window.setTimeout(() => { scroll.restoring = false; }, 0);
  }

  private maxConsoleHeight(): number {
    return Math.max(MIN_CONSOLE_HEIGHT, this.context.elements.workspace.clientHeight - MIN_BOARD_HEIGHT);
  }

  // The requested height is kept as is so the console returns to it when the window grows again.
  private appliedConsoleHeight(): number {
    return Math.round(Math.min(Math.max(this.consoleHeight, MIN_CONSOLE_HEIGHT), this.maxConsoleHeight()));
  }

  private patchOutput(output: HTMLElement, patch: ConsoleOutputPatch): void {
    patchConsoleOutput(output, patch.markup, patch.kind, patch.log, this.consoleFollow);
    this.updateConsoleScrollAffordance(output);
  }

  private updateConsoleScrollAffordance(output: HTMLElement): void {
    const button = output.closest<HTMLElement>(".console-output-shell")?.querySelector<HTMLButtonElement>("[data-action='jump-to-bottom']");
    if (!button) return;
    button.hidden = isConsoleAtBottom(output);
  }
}

export function createConsoleController(context: ConsoleControllerContext): ConsoleController {
  return new ConsoleController(context);
}
