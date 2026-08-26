import { required } from "./ui-support";

export type KeyboardNavigationContext = {
  handleResizeKey: (event: KeyboardEvent) => boolean;
  selectTask: (profileId: string, taskName: string, focus: boolean) => void;
  selectContainer: (id: string, focus: boolean) => void;
};

export function focusTaskRow(profileId: string, taskName: string): void {
  const row = [...document.querySelectorAll<HTMLElement>(".task-card")]
    .find((item) => item.dataset.profileId === profileId && item.dataset.taskName === taskName);
  row?.focus();
}

export function focusServiceCard(id: string): void {
  const card = [...document.querySelectorAll<HTMLElement>(".service-select-button")]
    .find((item) => item.dataset.serviceId === id);
  card?.focus();
}

export function focusContainerCard(id: string): void {
  const card = [...document.querySelectorAll<HTMLElement>(".container-tile[data-action='select-container']")]
    .find((item) => item.dataset.containerId === id);
  card?.focus();
}

export function createKeyboardNavigation(context: KeyboardNavigationContext) {
  const handleKeyboard = (event: KeyboardEvent): void => {
    if (context.handleResizeKey(event)) return;
    const taskRow = event.target instanceof HTMLElement ? event.target.closest<HTMLElement>(".task-card") : null;
    if (taskRow && event.target === taskRow) {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        context.selectTask(required(taskRow.dataset.profileId), required(taskRow.dataset.taskName), true);
        return;
      }
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        const rows = [...document.querySelectorAll<HTMLElement>(".task-card")];
        const index = rows.indexOf(taskRow);
        const step = event.key === "ArrowDown" ? 1 : -1;
        rows[(index + step + rows.length) % rows.length]?.focus();
        event.preventDefault();
        return;
      }
    }
    const containerTile = event.target instanceof HTMLElement
      ? event.target.closest<HTMLElement>(".container-tile[data-action='select-container']")
      : null;
    if (containerTile && event.target === containerTile && (event.key === "Enter" || event.key === " ")) {
      event.preventDefault();
      context.selectContainer(required(containerTile.dataset.containerId), true);
      return;
    }
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const current = event.target instanceof HTMLElement ? event.target.closest<HTMLButtonElement>("[data-tile-action]") : null;
    const card = current?.closest<HTMLElement>(".service-tile");
    if (!current || !card) return;
    const actions = [...card.querySelectorAll<HTMLButtonElement>("[data-tile-action]:not([disabled])")];
    if (actions.length < 2) return;
    event.preventDefault();
    const index = Math.max(0, actions.indexOf(current));
    const step = event.key === "ArrowRight" ? 1 : -1;
    actions[(index + step + actions.length) % actions.length]?.focus();
  };

  return { handleKeyboard };
}
