import { launchTaskKey } from "./launch-rendering";

// Moves the selected-task highlight and swaps the console section without rebuilding the launch view.
// Returns false when the launch view is not on screen so the caller can fall back to a full render.
export function patchLaunchSelection(workspace: HTMLElement, selectedTaskKey: string | null, consoleMarkup: string): boolean {
  const view = workspace.querySelector<HTMLElement>(".launch-view");
  if (!view) return false;
  for (const card of workspace.querySelectorAll<HTMLElement>(".task-card")) {
    const selected = launchTaskKey(card.dataset.profileId ?? "", card.dataset.taskName ?? "") === selectedTaskKey;
    if (card.classList.contains("is-selected") !== selected) card.classList.toggle("is-selected", selected);
    const ariaCurrent = selected ? "true" : "false";
    if (card.getAttribute("aria-current") !== ariaCurrent) card.setAttribute("aria-current", ariaCurrent);
  }
  const consoleSection = view.querySelector<HTMLElement>(":scope > .launch-console");
  if (consoleSection) {
    if (consoleMarkup) consoleSection.outerHTML = consoleMarkup;
    else consoleSection.remove();
  } else if (consoleMarkup) {
    view.insertAdjacentHTML("beforeend", consoleMarkup);
  }
  return true;
}
