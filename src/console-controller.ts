import { uiIcon, type UiIconName } from "./icons";
import { escapeHtml as h } from "./html";
import { patchConsoleOutput } from "./console-dom";
import {
  captureConsoleLogSelection,
  consoleLogValue,
  consoleScrollElement,
  findConsoleLogMatches,
  hasConsoleLogSelection,
  isConsoleLogElement,
  isConsoleLogMutationKey,
  moveConsoleCaret,
  consolePageBoundary,
  revealConsoleCaret,
  restoreConsoleLogSelection,
  setConsoleLogValue,
  type ConsoleLogMatch,
  type ConsoleLogSelection
} from "./console-log-view";
import {
  appendConsoleLineBreak,
  reconcileConsoleLog,
  type ConsoleLogPresentation
} from "./console-log-presentation";
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
type ScrollState = { key: string | null; top: number; restoring: boolean; selection: ConsoleLogSelection | null };
type ContainerScrollState = { containerId: string | null; top: number; restoring: boolean; selection: ConsoleLogSelection | null };
type EscapedOffsets = ArrayLike<number>;
type ConsoleLogSearch = {
  output: HTMLElement;
  input: HTMLInputElement;
  value: string;
  query: string;
  matches: ConsoleLogMatch[];
  currentIndex: number;
  matchCache: Map<string, ConsoleLogMatch[]>;
  renderedValue: string;
  renderedQuery: string;
  escapedValue: string;
  escapedHtml: string;
  escapedOffsets: EscapedOffsets;
  highlightMode: "css" | "dom" | null;
  highlightText: Text | null;
  highlightRevision: number;
  highlightFrame: number | null;
  scrollLog: HTMLTextAreaElement | null;
  scrollHandler: ((event: Event) => void) | null;
};

const MIN_CONSOLE_HEIGHT = 220;
const DEFAULT_CONSOLE_HEIGHT = 336;
const MIN_BOARD_HEIGHT = 140;
const CONSOLE_RESIZE_STEP = 24;
const BOTTOM_PANELS: ReadonlyArray<{ id: BottomPanelId; label: string; icon: UiIconName }> = [
  { id: "console", label: "Console", icon: "terminal" }
];
const CONSOLE_SEARCH_HIGHLIGHT = "console-search-match";
const CONSOLE_SEARCH_CURRENT_HIGHLIGHT = "console-search-current";
const CONSOLE_SEARCH_HIGHLIGHT_CHUNK = 512;

export class ConsoleController {
  private readonly context: ConsoleControllerContext;
  private activeBottomPanel: BottomPanelId | null = "console";
  private consoleHeight = DEFAULT_CONSOLE_HEIGHT;
  private consoleFollow = true;
  private readonly logPresentations = new Map<string, ConsoleLogPresentation>();
  private readonly pendingOutputScrolls = new WeakSet<HTMLElement>();
  private readonly pendingConsoleInput = new WeakMap<HTMLTextAreaElement, ConsoleLogSelection>();
  private logSearch: ConsoleLogSearch | null = null;
  private readonly taskScroll: ScrollState = { key: null, top: 0, restoring: false, selection: null };
  private readonly serviceScroll: ScrollState = { key: null, top: 0, restoring: false, selection: null };
  private readonly containerScroll: Record<ContainerTab, ContainerScrollState> = {
    services: { containerId: null, top: 0, restoring: false, selection: null },
    docker: { containerId: null, top: 0, restoring: false, selection: null }
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

  handleSearchAction(action: string, target: HTMLElement): boolean {
    if (!["console-search-next", "console-search-previous", "close-console-search"].includes(action)) return false;
    const search = this.logSearch;
    if (!search || target.closest<HTMLElement>(".console-search") !== search.input.closest<HTMLElement>(".console-search")) return true;
    if (action === "close-console-search") {
      this.closeLogSearch(true);
      return true;
    }
    const direction = action === "console-search-next" ? "next" : "previous";
    if (this.commitLogSearch(search, true)) return true;
    this.moveLogSearch(direction);
    return true;
  }

  handleSearchInput(event: Event): boolean {
    const input = event.target instanceof HTMLInputElement && event.target.classList.contains("console-search-input")
      ? event.target
      : null;
    if (!input) return false;
    const search = this.logSearch;
    if (!search || search.input !== input) return true;
    // Search is explicit: keep typing local to the input and wait for Enter or
    // a navigation button before scanning and repainting the log.
    this.updateSearchStatus(search);
    return true;
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
    const scroll = consoleScrollElement(output);
    if (this.consoleFollow) scroll.scrollTop = scroll.scrollHeight;
    this.syncSearchHighlight(output);
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

  handleOutputKey(event: KeyboardEvent): boolean {
    const eventElement = event.target instanceof Element ? event.target : null;
    const searchInput = eventElement?.closest<HTMLInputElement>(".console-search-input");
    if (searchInput) {
      if (event.key === "Escape") {
        event.preventDefault();
        this.closeLogSearch(true);
        return true;
      }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f" && !event.altKey && !event.isComposing) {
        event.preventDefault();
        searchInput.select();
        return true;
      }
      if (event.key === "Enter" && !event.altKey && !event.isComposing) {
        event.preventDefault();
        const search = this.logSearch?.input === searchInput ? this.logSearch : null;
        if (this.commitLogSearch(search, true)) return true;
        this.moveLogSearch(event.shiftKey ? "previous" : "next");
        return true;
      }
      return false;
    }

    const output = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-output") : null;
    const log = output?.querySelector<HTMLElement>(".console-log");
    const key = output ? this.consoleOutputKey(output) : null;
    if (!output || !log || !key || !["log", "log-alert"].includes(output.dataset.consoleOutputKind ?? "")) return false;

    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f" && !event.altKey && !event.isComposing) {
      event.preventDefault();
      if (log instanceof HTMLTextAreaElement) this.openLogSearch(output, log);
      return true;
    }

    if (event.key === "Escape" && this.logSearch?.output === output) {
      event.preventDefault();
      this.closeLogSearch(true);
      return true;
    }

    if (log instanceof HTMLTextAreaElement && !event.altKey && !event.isComposing && (event.ctrlKey || event.metaKey)) {
      if (event.key === "PageUp" || event.key === "PageDown") {
        event.preventDefault();
        const direction = event.key === "PageUp" ? "top" : "bottom";
        const offset = consolePageBoundary(log, direction);
        moveConsoleCaret(log, offset, event.shiftKey);
        revealConsoleCaret(log, offset, direction);
        return true;
      }
    }

    if (event.key === "Enter" && !event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      const saved = this.logPresentations.get(key);
      const visible = consoleLogValue(log);
      const current = saved && visible === saved.output
        ? saved
        : reconcileConsoleLog(saved, visible);
      const next = appendConsoleLineBreak(current, current.source);
      this.logPresentations.set(key, next);
      setConsoleLogValue(log, next.output);
      if (log instanceof HTMLTextAreaElement) log.setSelectionRange(next.output.length, next.output.length, "none");
      this.consoleFollow = true;
      this.scheduleOutputScroll(output);
      return true;
    }

    if (log instanceof HTMLTextAreaElement && isConsoleLogMutationKey(event)) {
      event.preventDefault();
      return true;
    }

    return false;
  }

  /** Keep the log editable to the browser so it can expose a real caret, while
   * rejecting every value mutation. Native navigation and copying still work. */
  handleOutputMutation(event: Event): boolean {
    if (!isConsoleLogElement(event.target)) return false;
    const log = event.target;
    const output = log.closest<HTMLElement>(".console-output");
    if (!output) return false;

    if (event.type === "input") {
      this.restoreConsoleInput(log, output);
      return true;
    }

    this.pendingConsoleInput.set(log, {
      value: log.value,
      start: log.selectionStart,
      end: log.selectionEnd,
      direction: log.selectionDirection,
      focused: true,
      scrollTop: log.scrollTop,
      scrollLeft: log.scrollLeft
    });
    event.preventDefault();
    return true;
  }

  private restoreConsoleInput(log: HTMLTextAreaElement, output: HTMLElement): void {
    const previous = this.pendingConsoleInput.get(log);
    this.pendingConsoleInput.delete(log);
    const key = this.consoleOutputKey(output);
    const presentation = key ? this.logPresentations.get(key) : undefined;
    const expected = presentation?.output;
    if (expected === undefined || log.value === expected) return;

    const state = previous?.value === expected ? previous : {
      value: log.value,
      start: log.selectionStart,
      end: log.selectionEnd,
      direction: log.selectionDirection,
      focused: true,
      scrollTop: log.scrollTop,
      scrollLeft: log.scrollLeft
    };
    setConsoleLogValue(log, expected);
    restoreConsoleLogSelection(output, { ...state, value: state.value });
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
    const scroll = consoleScrollElement(output);
    this.taskScroll.top = scroll.scrollTop;
    this.taskScroll.selection = captureConsoleLogSelection(output);
  }

  restoreLaunchConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".launch-console:not(.docker-console):not(.docker-service-console) .console-output");
    if (!output) return;
    this.restorePresentedLog(output);
    restoreConsoleLogSelection(output, this.taskScroll.selection);
    this.taskScroll.restoring = true;
    const selectedTaskKey = this.context.state.selectedTaskKey();
    const savedScrollTop = this.taskScroll.key === selectedTaskKey ? this.taskScroll.top : 0;
    const scroll = consoleScrollElement(output);
    const follow = this.consoleFollow && !hasConsoleLogSelection(output);
    if (!follow) this.consoleFollow = false;
    scroll.scrollTop = scrollTopForConsoleUpdate(scroll, savedScrollTop, follow);
    this.taskScroll.key = selectedTaskKey;
    this.taskScroll.top = scroll.scrollTop;
    this.taskScroll.selection = captureConsoleLogSelection(output);
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
    const scrollElement = consoleScrollElement(output);
    scroll.top = scrollElement.scrollTop;
    scroll.selection = captureConsoleLogSelection(output);
  }

  restoreDockerConsoleState(): void {
    this.restoreDockerConsoleScroll();
  }

  resetTaskSelection(key: string | null): void {
    this.taskScroll.key = key;
    this.taskScroll.top = 0;
    this.taskScroll.selection = null;
  }

  resetServiceSelection(key: string | null): void {
    this.serviceScroll.key = key;
    this.serviceScroll.top = 0;
    this.serviceScroll.selection = null;
  }

  resetContainerSelection(tab: ContainerTab, containerId: string | null): void {
    const scroll = this.containerScroll[tab];
    scroll.containerId = containerId;
    scroll.top = 0;
    scroll.selection = null;
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
    const scrollElement = consoleScrollElement(output);
    scrollElement.scrollTop = scrollElement.scrollHeight;
    const serviceConsole = output.closest<HTMLElement>(".service-console:not(.docker-service-console)");
    if (serviceConsole) {
      this.serviceScroll.key = this.context.state.selectedServiceId();
      this.serviceScroll.top = scrollElement.scrollTop;
      this.serviceScroll.selection = captureConsoleLogSelection(output);
    } else {
      const dockerConsole = output.closest<HTMLElement>(".docker-console, .docker-service-console");
      if (dockerConsole) {
        const tab: ContainerTab = dockerConsole.classList.contains("docker-service-console") ? "services" : "docker";
        const state = this.context.state.container(tab);
        const scroll = this.containerScroll[tab];
        scroll.containerId = state.selectedContainerId;
        scroll.top = scrollElement.scrollTop;
        scroll.selection = captureConsoleLogSelection(output);
      } else {
        this.taskScroll.key = this.context.state.selectedTaskKey();
        this.taskScroll.top = scrollElement.scrollTop;
        this.taskScroll.selection = captureConsoleLogSelection(output);
      }
    }
    this.updateConsoleScrollAffordance(output);
    this.syncSearchHighlight(output);
  }

  handleScroll(event: Event): void {
    const target = event.target instanceof Element ? event.target.closest<HTMLElement>(".console-output") : null;
    if (!target) return;
    const serviceConsole = target.closest<HTMLElement>(".service-console:not(.docker-service-console)");
    if (serviceConsole) {
      const serviceId = serviceConsole.dataset.consoleServiceId || null;
      const servicesConsoleTarget = this.context.state.servicesConsoleTarget();
      if (servicesConsoleTarget?.kind !== "service" || serviceId !== this.context.state.selectedServiceId()) return;
      const scrollElement = consoleScrollElement(target);
      this.serviceScroll.key = this.context.state.selectedServiceId();
      this.serviceScroll.top = scrollElement.scrollTop;
      this.serviceScroll.selection = captureConsoleLogSelection(target);
      if (!this.serviceScroll.restoring) this.consoleFollow = isConsoleAtBottom(scrollElement, CONSOLE_BOTTOM_EPSILON);
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
      const scrollElement = consoleScrollElement(target);
      scroll.containerId = state.selectedContainerId;
      scroll.top = scrollElement.scrollTop;
      scroll.selection = captureConsoleLogSelection(target);
      if (!scroll.restoring) this.consoleFollow = isConsoleAtBottom(scrollElement, CONSOLE_BOTTOM_EPSILON);
      this.updateConsoleScrollAffordance(target);
      return;
    }
    const taskKey = target.closest<HTMLElement>(".launch-console")?.dataset.consoleTaskKey ?? null;
    if (taskKey !== this.context.state.selectedTaskDomKey()) return;
    const scrollElement = consoleScrollElement(target);
    this.taskScroll.key = this.context.state.selectedTaskKey();
    this.taskScroll.top = scrollElement.scrollTop;
    this.taskScroll.selection = captureConsoleLogSelection(target);
    if (!this.taskScroll.restoring) this.consoleFollow = isConsoleAtBottom(scrollElement, CONSOLE_BOTTOM_EPSILON);
    this.updateConsoleScrollAffordance(target);
  }

  private captureServiceConsoleState(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
    if (!output) return;
    const serviceId = output.closest<HTMLElement>(".service-console:not(.docker-service-console)")?.dataset.consoleServiceId || null;
    const target = this.context.state.servicesConsoleTarget();
    if (target?.kind !== "service" || serviceId !== this.context.state.selectedServiceId()) return;
    const scroll = consoleScrollElement(output);
    this.serviceScroll.key = this.context.state.selectedServiceId();
    this.serviceScroll.top = scroll.scrollTop;
    this.serviceScroll.selection = captureConsoleLogSelection(output);
  }

  private restoreServiceConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".service-console:not(.docker-service-console) .console-output");
    if (!output) return;
    this.restorePresentedLog(output);
    restoreConsoleLogSelection(output, this.serviceScroll.selection);
    this.serviceScroll.restoring = true;
    const selectedServiceId = this.context.state.selectedServiceId();
    const savedScrollTop = this.serviceScroll.key === selectedServiceId ? this.serviceScroll.top : 0;
    const scroll = consoleScrollElement(output);
    const follow = this.consoleFollow && !hasConsoleLogSelection(output);
    if (!follow) this.consoleFollow = false;
    scroll.scrollTop = scrollTopForConsoleUpdate(scroll, savedScrollTop, follow);
    this.serviceScroll.key = selectedServiceId;
    this.serviceScroll.top = scroll.scrollTop;
    this.serviceScroll.selection = captureConsoleLogSelection(output);
    this.updateConsoleScrollAffordance(output);
    window.setTimeout(() => { this.serviceScroll.restoring = false; }, 0);
  }

  private restoreDockerConsoleScroll(): void {
    const output = this.context.elements.workspace.querySelector<HTMLElement>(".docker-console .console-output, .docker-service-console .console-output");
    if (!output) return;
    this.restorePresentedLog(output);
    const consoleElement = output.closest<HTMLElement>(".docker-console, .docker-service-console");
    const tab: ContainerTab = consoleElement?.classList.contains("docker-service-console") ? "services" : "docker";
    const state = this.context.state.container(tab);
    const scroll = this.containerScroll[tab];
    restoreConsoleLogSelection(output, scroll.selection);
    scroll.restoring = true;
    const savedScrollTop = scroll.containerId === state.selectedContainerId ? scroll.top : 0;
    const scrollElement = consoleScrollElement(output);
    const follow = this.consoleFollow && !hasConsoleLogSelection(output);
    if (!follow) this.consoleFollow = false;
    scrollElement.scrollTop = scrollTopForConsoleUpdate(scrollElement, savedScrollTop, follow);
    scroll.containerId = state.selectedContainerId;
    scroll.top = scrollElement.scrollTop;
    scroll.selection = captureConsoleLogSelection(output);
    this.updateConsoleScrollAffordance(output);
    window.setTimeout(() => { scroll.restoring = false; }, 0);
  }

  private renderConsoleSearch(): string {
    return `<div class="console-search" role="search"><input class="console-search-input" type="search" aria-label="Find in log" placeholder="Find in log (Enter)" autocomplete="off" spellcheck="false"><span class="console-search-count" data-console-search-count aria-live="polite"></span><button class="console-search-button" type="button" data-action="console-search-previous" aria-label="Previous match" title="Previous match">↑</button><button class="console-search-button" type="button" data-action="console-search-next" aria-label="Next match" title="Next match">↓</button><button class="console-search-button console-search-close" type="button" data-action="close-console-search" aria-label="Close search" title="Close search">×</button></div>`;
  }

  private openLogSearch(output: HTMLElement, log: HTMLTextAreaElement): void {
    if (this.logSearch?.output !== output) {
      this.closeLogSearch(false);
      const shell = output.closest<HTMLElement>(".console-output-shell");
      shell?.querySelector<HTMLElement>(".console-search")?.remove();
    }

    let search = this.logSearch;
    if (!search) {
      const shell = output.closest<HTMLElement>(".console-output-shell");
      if (!shell) return;
      shell.insertAdjacentHTML("afterbegin", this.renderConsoleSearch());
      const input = shell.querySelector<HTMLInputElement>(".console-search-input");
      if (!input) return;
      search = {
        output,
        input,
        value: "",
        query: "",
        matches: [],
        currentIndex: 0,
        matchCache: new Map(),
        renderedValue: "",
        renderedQuery: "",
        escapedValue: "",
        escapedHtml: "",
        escapedOffsets: [],
        highlightMode: null,
        highlightText: null,
        highlightRevision: 0,
        highlightFrame: null,
        scrollLog: null,
        scrollHandler: null
      };
      this.logSearch = search;
    }

    this.bindSearchScroll(search, log);
    // Ctrl+F only opens the draft. The selected text is intentionally not
    // searched until the user confirms it with Enter or a search button.
    search.input.value = search.query || (log.selectionStart !== log.selectionEnd
      ? log.value.slice(log.selectionStart, log.selectionEnd)
      : "");
    this.updateSearchStatus(search);
    search.input.focus({ preventScroll: true });
    search.input.select();
  }

  private closeLogSearch(focusLog: boolean): void {
    const search = this.logSearch;
    if (!search) return;
    const log = search.output.querySelector<HTMLTextAreaElement>(".console-log");
    const match = search.matches[search.currentIndex];
    search.output.querySelector<HTMLElement>(".console-search-highlights")?.remove();
    this.cancelCssSearchHighlightWork(search);
    this.clearCssSearchHighlights();
    if (search.scrollLog && search.scrollHandler) search.scrollLog.removeEventListener("scroll", search.scrollHandler);
    search.scrollLog = null;
    search.scrollHandler = null;
    search.input.closest<HTMLElement>(".console-search")?.remove();
    this.logSearch = null;
    if (focusLog && log) {
      log.focus({ preventScroll: true });
      if (match) {
        log.setSelectionRange(match.start, match.end, "forward");
        revealConsoleCaret(log, match.start, "top");
      }
    }
  }

  private moveLogSearch(direction: "next" | "previous"): void {
    const search = this.logSearch;
    if (!search) return;
    if (!search.output.isConnected) {
      this.closeLogSearch(false);
      return;
    }
    if (!search.matches.length) return;
    const step = direction === "next" ? 1 : -1;
    search.currentIndex = (search.currentIndex + step + search.matches.length) % search.matches.length;
    this.consoleFollow = false;
    this.renderSearchHighlights(search);
    this.updateSearchStatus(search);
    this.revealSearchMatch(search);
    search.input.focus({ preventScroll: true });
  }

  /** Commit the current draft and return true when it performed a new search. */
  private commitLogSearch(search: ConsoleLogSearch | null, reveal: boolean): boolean {
    if (!search || this.logSearch !== search || !search.input.isConnected) return false;
    if (search.input.value === search.query) return false;
    this.updateLogSearch(search, search.input.value, reveal, true);
    return true;
  }

  private updateLogSearch(search: ConsoleLogSearch, query: string, reveal: boolean, resetCurrent = false): void {
    const log = search.output.querySelector<HTMLTextAreaElement>(".console-log");
    if (!log) {
      this.closeLogSearch(false);
      return;
    }
    const previousStart = resetCurrent ? 0 : search.matches[search.currentIndex]?.start ?? 0;
    const valueChanged = search.value !== log.value;
    const queryChanged = search.query !== query;
    if (!valueChanged && !queryChanged && (!query || !search.matches.length || search.output.querySelector(".console-search-highlights"))) {
      this.updateSearchStatus(search);
      if (reveal && search.matches.length) this.revealSearchMatch(search);
      return;
    }
    if (valueChanged) {
      search.matchCache.clear();
      search.escapedValue = "";
      search.escapedHtml = "";
      search.escapedOffsets = [];
    }
    search.value = log.value;
    search.query = query;
    search.matches = this.searchMatches(search, log.value, query);
    const nextIndex = search.matches.findIndex((match) => match.start >= previousStart);
    search.currentIndex = resetCurrent ? 0 : nextIndex >= 0 ? nextIndex : 0;
    if (query) this.consoleFollow = false;
    this.renderSearchHighlights(search);
    this.updateSearchStatus(search);
    if (reveal && search.matches.length) this.revealSearchMatch(search);
  }

  private searchMatches(
    search: ConsoleLogSearch,
    value: string,
    query: string
  ): ConsoleLogMatch[] {
    if (search.matchCache.has(query)) return search.matchCache.get(query) ?? [];
    const matches = findConsoleLogMatches(value, query);
    search.matchCache.set(query, matches);
    while (search.matchCache.size > 12) {
      const oldest = search.matchCache.keys().next().value;
      if (oldest === undefined) break;
      search.matchCache.delete(oldest);
    }
    return matches;
  }

  private escapedSearchValue(search: ConsoleLogSearch, value: string): { html: string; offsets: EscapedOffsets } {
    if (search.escapedValue === value) return { html: search.escapedHtml, offsets: search.escapedOffsets };
    const chunks = new Array<string>(value.length);
    const offsets = new Uint32Array(value.length + 1);
    let htmlLength = 0;
    for (let index = 0; index < value.length; index += 1) {
      offsets[index] = htmlLength;
      const character = value[index] ?? "";
      const escaped = character === "&" ? "&amp;"
        : character === "<" ? "&lt;"
          : character === ">" ? "&gt;"
            : character === "'" ? "&#39;"
              : character === '"' ? "&quot;"
                : character;
      chunks[index] = escaped;
      htmlLength += escaped.length;
    }
    offsets[value.length] = htmlLength;
    search.escapedValue = value;
    search.escapedHtml = chunks.join("");
    search.escapedOffsets = offsets;
    return { html: search.escapedHtml, offsets: search.escapedOffsets };
  }

  private updateSearchStatus(search: ConsoleLogSearch): void {
    const bar = search.input.closest<HTMLElement>(".console-search");
    const count = bar?.querySelector<HTMLElement>("[data-console-search-count]");
    const draftChanged = search.input.value !== search.query;
    const hasQuery = search.query.length > 0 && !draftChanged;
    if (count) {
      count.textContent = !hasQuery ? "" : search.matches.length === 0 ? "No results" : `${search.currentIndex + 1} of ${search.matches.length}`;
    }
    bar?.classList.toggle("is-empty", !hasQuery || search.matches.length === 0);
    bar?.querySelectorAll<HTMLButtonElement>("[data-action='console-search-previous'], [data-action='console-search-next']")
      .forEach((button) => { button.disabled = !draftChanged && search.matches.length === 0; });
  }

  private renderSearchHighlights(search: ConsoleLogSearch): void {
    const output = search.output;
    const log = output.querySelector<HTMLTextAreaElement>(".console-log");
    const existing = output.querySelector<HTMLElement>(".console-search-highlights");
    if (log) this.bindSearchScroll(search, log);
    if (!log || !search.query || !search.matches.length) {
      existing?.remove();
      this.cancelCssSearchHighlightWork(search);
      this.clearCssSearchHighlights();
      search.renderedValue = "";
      search.renderedQuery = "";
      search.highlightMode = null;
      search.highlightText = null;
      return;
    }
    if (this.renderCssSearchHighlights(search, log, existing)) return;
    if (existing && search.renderedValue === log.value && search.renderedQuery === search.query) {
      existing.querySelector<HTMLElement>(".is-current")?.classList.remove("is-current");
      existing.querySelectorAll<HTMLElement>(".console-search-match")[search.currentIndex]?.classList.add("is-current");
      this.syncSearchHighlight(output);
      return;
    }
    let cursor = 0;
    const escaped = this.escapedSearchValue(search, log.value);
    const slice = (start: number, end: number): string => escaped.html.slice(escaped.offsets[start] ?? 0, escaped.offsets[end] ?? escaped.html.length);
    const highlighted = search.matches.map((match, index) => {
      const prefix = slice(cursor, match.start);
      const current = index === search.currentIndex ? " is-current" : "";
      cursor = match.end;
      return `${prefix}<mark class="console-search-match${current}">${slice(match.start, match.end)}</mark>`;
    }).join("");
    const markup = `<div class="console-search-highlights" aria-hidden="true"><pre class="console-search-highlight-text">${highlighted}${slice(cursor, log.value.length)}</pre></div>`;
    existing?.remove();
    output.insertAdjacentHTML("beforeend", markup);
    search.renderedValue = log.value;
    search.renderedQuery = search.query;
    search.highlightMode = "dom";
    search.highlightText = null;
    this.syncSearchHighlight(output);
  }

  private supportsCssSearchHighlights(): boolean {
    return typeof CSS !== "undefined" && typeof Highlight !== "undefined" && "highlights" in CSS;
  }

  private clearCssSearchHighlights(): void {
    if (!this.supportsCssSearchHighlights()) return;
    CSS.highlights.delete(CONSOLE_SEARCH_HIGHLIGHT);
    CSS.highlights.delete(CONSOLE_SEARCH_CURRENT_HIGHLIGHT);
  }

  private renderCssSearchHighlights(
    search: ConsoleLogSearch,
    log: HTMLTextAreaElement,
    existing: HTMLElement | null
  ): boolean {
    if (!this.supportsCssSearchHighlights()) return false;
    const reusable = existing && search.highlightMode === "css" && search.highlightText?.isConnected
      && search.renderedValue === log.value && search.renderedQuery === search.query;
    if (reusable) {
      CSS.highlights.delete(CONSOLE_SEARCH_CURRENT_HIGHLIGHT);
      const current = search.matches[search.currentIndex];
      if (current && search.highlightText) {
        const range = document.createRange();
        range.setStart(search.highlightText, current.start);
        range.setEnd(search.highlightText, current.end);
        const currentHighlight = new Highlight(range);
        currentHighlight.priority = 1;
        CSS.highlights.set(CONSOLE_SEARCH_CURRENT_HIGHLIGHT, currentHighlight);
      }
      this.syncSearchHighlight(search.output);
      return true;
    }
    if (!reusable) {
      this.cancelCssSearchHighlightWork(search);
      existing?.remove();
      this.clearCssSearchHighlights();
      const layer = document.createElement("div");
      layer.className = "console-search-highlights";
      layer.setAttribute("aria-hidden", "true");
      const text = document.createElement("pre");
      text.className = "console-search-highlight-text";
      text.textContent = log.value;
      layer.appendChild(text);
      search.output.appendChild(layer);
      search.highlightMode = "css";
      search.highlightText = text.firstChild instanceof Text ? text.firstChild : null;
      search.renderedValue = log.value;
      search.renderedQuery = search.query;
    }
    const text = search.highlightText;
    if (!text) return false;
    this.clearCssSearchHighlights();
    const matchHighlight = new Highlight();
    CSS.highlights.set(CONSOLE_SEARCH_HIGHLIGHT, matchHighlight);
    const current = search.matches[search.currentIndex];
    if (current) {
      this.setCssCurrentHighlight(current, text);
    }
    this.appendCssSearchRanges(search, text, matchHighlight, search.matches, search.highlightRevision);
    this.syncSearchHighlight(search.output);
    return true;
  }

  private cancelCssSearchHighlightWork(search: ConsoleLogSearch): void {
    search.highlightRevision += 1;
    if (search.highlightFrame !== null) {
      window.cancelAnimationFrame(search.highlightFrame);
      search.highlightFrame = null;
    }
  }

  private setCssCurrentHighlight(match: ConsoleLogMatch, text: Text): void {
    const range = document.createRange();
    range.setStart(text, match.start);
    range.setEnd(text, match.end);
    const currentHighlight = new Highlight(range);
    currentHighlight.priority = 1;
    CSS.highlights.set(CONSOLE_SEARCH_CURRENT_HIGHLIGHT, currentHighlight);
  }

  // Dense logs can produce tens of thousands of ranges, so yield between small batches.
  private appendCssSearchRanges(
    search: ConsoleLogSearch,
    text: Text,
    highlight: Highlight,
    matches: readonly ConsoleLogMatch[],
    revision: number,
    start = 0
  ): void {
    const chunkStart = performance.now();
    let index = start;
    const add = (highlight as unknown as { add(range: AbstractRange): void }).add.bind(highlight);
    while (index < matches.length && index - start < CONSOLE_SEARCH_HIGHLIGHT_CHUNK && performance.now() - chunkStart < 6) {
      const match = matches[index];
      if (match) {
        const range = document.createRange();
        range.setStart(text, match.start);
        range.setEnd(text, match.end);
        add(range);
      }
      index += 1;
    }
    if (index >= matches.length) {
      search.highlightFrame = null;
      return;
    }
    search.highlightFrame = window.requestAnimationFrame(() => {
      search.highlightFrame = null;
      if (this.logSearch !== search || search.highlightMode !== "css" || search.highlightRevision !== revision || search.highlightText !== text) return;
      this.appendCssSearchRanges(search, text, highlight, matches, revision, index);
    });
  }

  private revealSearchMatch(search: ConsoleLogSearch): void {
    const log = search.output.querySelector<HTMLTextAreaElement>(".console-log");
    const match = search.matches[search.currentIndex];
    if (!log || !match) return;
    const logRect = log.getBoundingClientRect();
    const highlighted = search.highlightMode === "dom"
      ? search.output.querySelectorAll<HTMLElement>(".console-search-match")[search.currentIndex] ?? null
      : null;
    let matchRect = highlighted?.getBoundingClientRect() ?? null;
    if (!matchRect && search.highlightMode === "css" && search.highlightText?.isConnected) {
      const range = document.createRange();
      const end = Math.max(match.start + 1, match.end);
      range.setStart(search.highlightText, match.start);
      range.setEnd(search.highlightText, Math.min(end, search.highlightText.length));
      matchRect = range.getClientRects()[0] ?? null;
    }
    if (matchRect) {
      const searchBar = search.input.closest<HTMLElement>(".console-search")?.getBoundingClientRect();
      const clearance = searchBar && matchRect.right > searchBar.left && matchRect.left < searchBar.right
        ? Math.max(0, searchBar.bottom - logRect.top + 4)
        : 0;
      const targetTop = log.scrollTop + matchRect.top - logRect.top - clearance;
      log.scrollTop = Math.min(Math.max(0, log.scrollHeight - log.clientHeight), Math.max(0, targetTop));
    } else {
      revealConsoleCaret(log, match.start, "top");
    }
    this.syncSearchHighlight(search.output);
  }

  private bindSearchScroll(search: ConsoleLogSearch, log: HTMLTextAreaElement): void {
    if (search.scrollLog === log) return;
    if (search.scrollLog && search.scrollHandler) search.scrollLog.removeEventListener("scroll", search.scrollHandler);
    const handler = (): void => {
      if (this.logSearch !== search || !log.isConnected) return;
      this.syncSearchHighlight(search.output, false);
    };
    search.scrollLog = log;
    search.scrollHandler = handler;
    log.addEventListener("scroll", handler, { passive: true });
  }

  private syncSearchHighlight(output: HTMLElement, refreshMetrics = true): void {
    if (this.logSearch?.output !== output) return;
    const log = output.querySelector<HTMLTextAreaElement>(".console-log");
    const layer = output.querySelector<HTMLElement>(".console-search-highlights");
    const text = layer?.querySelector<HTMLElement>(".console-search-highlight-text");
    if (!log || !layer || !text) return;
    if (refreshMetrics) {
      const styles = getComputedStyle(log);
      layer.style.top = `${log.offsetTop}px`;
      layer.style.left = `${log.offsetLeft}px`;
      layer.style.width = `${log.clientWidth}px`;
      layer.style.height = `${log.clientHeight}px`;
      text.style.top = "0px";
      text.style.left = "0px";
      text.style.width = `${log.clientWidth}px`;
      text.style.height = `${Math.max(log.scrollHeight, log.clientHeight)}px`;
      // WebKit exposes an empty `font` shorthand for textareas. Copying it would
      // leave the pre at its default 13px size while the log uses 11px, shifting
      // every wrapped line and making otherwise correct ranges appear misplaced.
      text.style.fontFamily = styles.fontFamily;
      text.style.fontSize = styles.fontSize;
      text.style.fontStyle = styles.fontStyle;
      text.style.fontWeight = styles.fontWeight;
      text.style.fontStretch = styles.fontStretch;
      text.style.fontVariant = styles.fontVariant;
      text.style.fontVariantLigatures = styles.fontVariantLigatures;
      text.style.fontKerning = styles.fontKerning;
      text.style.fontFeatureSettings = styles.fontFeatureSettings;
      text.style.fontVariationSettings = styles.fontVariationSettings;
      text.style.lineHeight = styles.lineHeight;
      text.style.letterSpacing = styles.letterSpacing;
      text.style.wordSpacing = styles.wordSpacing;
      text.style.whiteSpace = styles.whiteSpace;
      text.style.overflowWrap = styles.overflowWrap;
      text.style.wordBreak = styles.wordBreak;
      text.style.setProperty("tab-size", styles.tabSize);
      text.style.textAlign = styles.textAlign;
      text.style.textIndent = styles.textIndent;
      text.style.textTransform = styles.textTransform;
      text.style.direction = styles.direction;
    }
    // Scroll the mirror itself instead of moving its text with a negative top.
    // WebKit keeps CSS Custom Highlight ranges in the mirror's scroll layer,
    // so their painted rectangles move with the corresponding textarea rows.
    layer.scrollTop = log.scrollTop;
    layer.scrollLeft = log.scrollLeft;
  }

  private refreshLogSearch(output: HTMLElement): void {
    if (this.logSearch?.output !== output) return;
    if (!output.isConnected || !output.querySelector(".console-log")) {
      this.closeLogSearch(false);
      return;
    }
    this.updateLogSearch(this.logSearch, this.logSearch.query, false);
  }

  private maxConsoleHeight(): number {
    return Math.max(MIN_CONSOLE_HEIGHT, this.context.elements.workspace.clientHeight - MIN_BOARD_HEIGHT);
  }

  // The requested height is kept as is so the console returns to it when the window grows again.
  private appliedConsoleHeight(): number {
    return Math.round(Math.min(Math.max(this.consoleHeight, MIN_CONSOLE_HEIGHT), this.maxConsoleHeight()));
  }

  private patchOutput(output: HTMLElement, patch: ConsoleOutputPatch): void {
    const hasLog = patch.kind === "log" || patch.kind === "log-alert";
    const key = this.consoleOutputKey(output);
    if (hasLog && hasConsoleLogSelection(output)) this.consoleFollow = false;
    let visibleLog = patch.log;
    if (hasLog && key) {
      const presentation = reconcileConsoleLog(this.logPresentations.get(key), patch.log);
      this.logPresentations.set(key, presentation);
      visibleLog = presentation.output;
    } else if (key) {
      this.logPresentations.delete(key);
    }
    patchConsoleOutput(output, patch.markup, patch.kind, visibleLog, this.consoleFollow);
    this.refreshLogSearch(output);
    this.updateConsoleScrollAffordance(output);
  }

  private restorePresentedLog(output: HTMLElement): void {
    const log = output.querySelector<HTMLElement>(".console-log");
    const key = this.consoleOutputKey(output);
    if (!log || !key) return;
    const presentation = reconcileConsoleLog(this.logPresentations.get(key), consoleLogValue(log));
    this.logPresentations.set(key, presentation);
    if (consoleLogValue(log) !== presentation.output) setConsoleLogValue(log, presentation.output);
  }

  private consoleOutputKey(output: HTMLElement): string | null {
    const consoleElement = output.closest<HTMLElement>(".launch-console");
    if (!consoleElement) return null;
    if (consoleElement.classList.contains("docker-console") || consoleElement.classList.contains("docker-service-console")) {
      const containerId = consoleElement.dataset.consoleContainerId;
      if (!containerId) return null;
      const tab = consoleElement.classList.contains("docker-service-console") ? "services" : "docker";
      return `container:${tab}:${containerId}`;
    }
    if (consoleElement.classList.contains("service-console")) {
      const serviceId = consoleElement.dataset.consoleServiceId;
      return serviceId ? `service:${serviceId}` : null;
    }
    const taskKey = consoleElement.dataset.consoleTaskKey;
    return taskKey ? `task:${taskKey}` : null;
  }

  private scheduleOutputScroll(output: HTMLElement): void {
    if (this.pendingOutputScrolls.has(output)) return;
    this.pendingOutputScrolls.add(output);
    window.requestAnimationFrame(() => {
      this.pendingOutputScrolls.delete(output);
      if (!output.isConnected) return;
      const scroll = consoleScrollElement(output);
      scroll.scrollTop = scroll.scrollHeight;
      this.updateConsoleScrollAffordance(output);
    });
  }

  private updateConsoleScrollAffordance(output: HTMLElement): void {
    const button = output.closest<HTMLElement>(".console-output-shell")?.querySelector<HTMLButtonElement>("[data-action='jump-to-bottom']");
    if (!button) return;
    button.hidden = isConsoleAtBottom(consoleScrollElement(output));
  }
}

export function createConsoleController(context: ConsoleControllerContext): ConsoleController {
  return new ConsoleController(context);
}
