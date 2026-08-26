import { scrollTopForConsoleUpdate } from "./console-scroll";

function patchConsoleMessage(current: HTMLElement, next: HTMLElement): void {
  const currentStrong = current.querySelector<HTMLElement>(":scope > strong");
  const nextStrong = next.querySelector<HTMLElement>(":scope > strong");
  if (currentStrong && nextStrong) {
    const currentElapsed = currentStrong.querySelector<HTMLElement>("[data-service-log-elapsed]");
    const nextElapsed = nextStrong.querySelector<HTMLElement>("[data-service-log-elapsed]");
    if (currentElapsed && nextElapsed) currentElapsed.textContent = nextElapsed.textContent;
    else if (currentStrong.innerHTML !== nextStrong.innerHTML) currentStrong.replaceChildren(...[...nextStrong.childNodes].map((child) => child.cloneNode(true)));
  }
  const currentCopy = [...current.children].filter((child) => child instanceof HTMLElement && child.tagName === "SPAN" && !child.classList.contains("console-message-icon")).at(-1) as HTMLElement | undefined;
  const nextCopy = [...next.children].filter((child) => child instanceof HTMLElement && child.tagName === "SPAN" && !child.classList.contains("console-message-icon")).at(-1) as HTMLElement | undefined;
  if (currentCopy && nextCopy && currentCopy.textContent !== nextCopy.textContent) currentCopy.textContent = nextCopy.textContent;
}

export function patchConsoleOutput(output: HTMLElement, markup: string, kind: string, log: string, follow: boolean): void {
  const sameKind = output.dataset.consoleOutputKind === kind;
  const hasLog = kind === "log" || kind === "log-alert";
  if (!sameKind) {
    const previousScrollTop = output.scrollTop;
    output.innerHTML = markup;
    output.dataset.consoleOutputKind = kind;
    if (hasLog) {
      output.scrollTop = scrollTopForConsoleUpdate(output, previousScrollTop, follow);
    }
    return;
  }

  const currentLog = output.querySelector<HTMLElement>(".console-log");
  if (hasLog && currentLog) {
    const previousScrollTop = output.scrollTop;
    const logChanged = currentLog.textContent !== log;
    if (logChanged) currentLog.textContent = log;
    const template = document.createElement("template");
    template.innerHTML = markup;
    const nextAlert = template.content.querySelector<HTMLElement>(".console-alert");
    const currentAlert = output.querySelector<HTMLElement>(".console-alert");
    const currentAlertText = currentAlert?.querySelector<HTMLElement>("span");
    const nextAlertText = nextAlert?.querySelector<HTMLElement>("span");
    if (currentAlertText && nextAlertText && currentAlertText.textContent !== nextAlertText.textContent) currentAlertText.textContent = nextAlertText.textContent;
    if (logChanged) {
      output.scrollTop = scrollTopForConsoleUpdate(output, previousScrollTop, follow);
    }
    return;
  }

  const template = document.createElement("template");
  template.innerHTML = markup;
  const nextMessage = template.content.querySelector<HTMLElement>(".console-message");
  const currentMessage = output.querySelector<HTMLElement>(".console-message");
  const nextAlert = template.content.querySelector<HTMLElement>(".console-alert");
  const currentAlert = output.querySelector<HTMLElement>(".console-alert");
  if (nextMessage && currentMessage && !nextAlert && !currentAlert) {
    patchConsoleMessage(currentMessage, nextMessage);
    return;
  }
  if (nextMessage && currentMessage && nextAlert && currentAlert) {
    patchConsoleMessage(currentMessage, nextMessage);
    const currentAlertText = currentAlert.querySelector<HTMLElement>("span");
    const nextAlertText = nextAlert.querySelector<HTMLElement>("span");
    if (currentAlertText && nextAlertText && currentAlertText.textContent !== nextAlertText.textContent) currentAlertText.textContent = nextAlertText.textContent;
    return;
  }
  output.innerHTML = markup;
  output.dataset.consoleOutputKind = kind;
}
