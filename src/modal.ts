import { escapeHtml } from "./html";
import { uiIcon } from "./icons";

let modalFocusReturn: HTMLElement | null = null;

export function openModal(title: string, body: string): void {
  const activeElement = document.activeElement;
  modalFocusReturn = activeElement instanceof HTMLElement && activeElement !== document.body ? activeElement : null;
  const modalRoot = document.getElementById("modal-root");
  if (!modalRoot) throw new Error("Missing #modal-root");
  modalRoot.innerHTML = `<div class="modal-backdrop" role="presentation"><section class="modal" tabindex="-1" role="dialog" aria-modal="true" aria-labelledby="modal-title"><header class="modal-header"><h2 id="modal-title">${escapeHtml(title)}</h2><button type="button" class="modal-close" data-action="close-modal" aria-label="Close">${uiIcon("close", 18)}</button></header><div class="modal-body">${body}</div></section></div>`;
  const modal = document.querySelector<HTMLElement>(".modal");
  const actions = modal?.querySelector<HTMLElement>(".modal-body .modal-actions");
  if (actions) {
    actions.classList.add("modal-footer");
    modal?.append(actions);
  }
  document.querySelector<HTMLElement>(".modal button, .modal input, .modal select")?.focus();
  const appShell = document.querySelector<HTMLElement>(".app-shell");
  appShell?.setAttribute("inert", "");
  appShell?.setAttribute("aria-hidden", "true");
}

export function closeModal(): void {
  if (!document.querySelector(".modal-backdrop")) return;
  document.getElementById("modal-root")?.replaceChildren();
  const appShell = document.querySelector<HTMLElement>(".app-shell");
  appShell?.removeAttribute("inert");
  appShell?.removeAttribute("aria-hidden");
  const focusReturn = modalFocusReturn;
  modalFocusReturn = null;
  if (focusReturn?.isConnected) focusReturn.focus();
}

export function trapModalFocus(event: KeyboardEvent): void {
  const modal = document.querySelector<HTMLElement>(".modal");
  if (!modal) return;
  const focusable = [...modal.querySelectorAll<HTMLElement>(
    'button:not([disabled]), [href], input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'
  )].filter((element) => !element.hasAttribute("hidden") && element.getAttribute("aria-hidden") !== "true");
  if (focusable.length === 0) {
    event.preventDefault();
    modal.focus();
    return;
  }
  const first = focusable[0]!;
  const last = focusable[focusable.length - 1]!;
  if (!modal.contains(document.activeElement) || document.activeElement === modal) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (event.shiftKey && document.activeElement === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && document.activeElement === last) {
    event.preventDefault();
    first.focus();
  }
}
