const MIN_TILE_WIDTH = 300;

export type BoardLayoutContext = {
  workspace: HTMLElement;
  onResize: () => void;
};

export function createBoardLayout(context: BoardLayoutContext) {
  const applyBoardLayout = (): void => {
    const board = context.workspace.querySelector<HTMLElement>(".board");
    if (!board) return;
    const styles = window.getComputedStyle(board);
    const gap = Number.parseFloat(styles.columnGap) || 0;
    const width = board.clientWidth - Number.parseFloat(styles.paddingLeft) - Number.parseFloat(styles.paddingRight);
    const columns = Math.max(1, Math.floor((width + gap) / (MIN_TILE_WIDTH + gap)));
    board.style.setProperty("--board-columns", String(columns));
    for (const section of board.querySelectorAll<HTMLElement>(".service-section")) {
      const tiles = Number(section.dataset.tiles) || 1;
      section.style.setProperty("--section-span", String(Math.min(Math.max(1, tiles), columns)));
    }
  };

  const installBoardObserver = (): void => {
    new ResizeObserver(() => {
      applyBoardLayout();
      context.onResize();
    }).observe(context.workspace);
  };

  return { applyBoardLayout, installBoardObserver };
}
