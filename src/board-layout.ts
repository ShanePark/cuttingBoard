const COMFORTABLE_TILE_WIDTH = 300;
const COMPACT_TILE_WIDTH = 248;
// Cards come in two sizes only. A board turns compact once it holds this many cards and turns
// roomy again only after dropping well below that, so a card or two coming and going around the
// threshold never resizes the whole board back and forth.
const COMPACT_ENTER_TILE_COUNT = 13;
const COMPACT_LEAVE_TILE_COUNT = 9;
const MOVE_DURATION_MS = 220;
const APPEAR_DURATION_MS = 160;

export type BoardLayoutContext = {
  workspace: HTMLElement;
  onResize: () => void;
};

/** Whether a board holding this many cards uses compact cards, given whether it already did. */
export function boardIsCompact(tileCount: number, wasCompact: boolean | undefined): boolean {
  if (wasCompact) return tileCount > COMPACT_LEAVE_TILE_COUNT;
  return tileCount >= COMPACT_ENTER_TILE_COUNT;
}

/**
 * How many columns fit the board at this card size. The count depends on the room alone, never
 * on the cards, so cards coming and going leave every other card in its column.
 */
export function boardColumns(width: number, gap: number, compact: boolean): number {
  const tileWidth = compact ? COMPACT_TILE_WIDTH : COMFORTABLE_TILE_WIDTH;
  return Math.max(1, Math.floor((width + gap) / (tileWidth + gap)));
}

type Offset = { left: number; top: number };
type BoardSnapshot = {
  view: string | undefined;
  width: number;
  compact: boolean;
  sections: Map<string, Offset>;
  tiles: Map<string, Offset>;
};

function tileKey(tile: HTMLElement): string | null {
  const { metricsId, containerId, profileId, taskName } = tile.dataset;
  if (metricsId) return `service:${metricsId}`;
  if (containerId) return `container:${containerId}`;
  if (profileId && taskName !== undefined) return `task:${profileId}\u0000${taskName}`;
  return null;
}

function offsetWithin(element: HTMLElement, origin: DOMRect): Offset {
  const rect = element.getBoundingClientRect();
  return { left: rect.left - origin.left, top: rect.top - origin.top };
}

export function createBoardLayout(context: BoardLayoutContext) {
  const compactByView = new Map<string, boolean>();
  let previous: BoardSnapshot | null = null;

  const applyBoardLayout = (): void => {
    const board = context.workspace.querySelector<HTMLElement>(".board");
    if (!board) {
      previous = null;
      return;
    }
    const view = context.workspace.dataset.view;
    const viewKey = view ?? "";
    const groups = [...board.querySelectorAll<HTMLElement>(".service-section")];
    // Launch stacks keep both their preview cards and hidden task cards in the DOM; section metadata
    // is the single source of truth for how many tiles the board is actually laying out.
    const renderedTileCount = groups.reduce((total, group) => total + (Number(group.dataset.tiles) || 1), 0);
    const compact = boardIsCompact(renderedTileCount, compactByView.get(viewKey));
    compactByView.set(viewKey, compact);
    // The gap and padding follow the density, so they are measured after it is set.
    board.style.setProperty("--tile-density", compact ? "1" : "0");
    const styles = window.getComputedStyle(board);
    const gap = Number.parseFloat(styles.columnGap) || 0;
    const width = board.clientWidth - Number.parseFloat(styles.paddingLeft) - Number.parseFloat(styles.paddingRight);
    const columns = boardColumns(width, gap, compact);
    board.style.setProperty("--board-columns", String(columns));
    for (const section of groups) {
      const tiles = Number(section.dataset.tiles) || 1;
      section.style.setProperty("--section-span", String(Math.min(Math.max(1, tiles), columns)));
    }
    const next = snapshotBoard(board, groups, { view, width: board.clientWidth, compact });
    // Only a change of cards moves anything; a new tab, a resize or a size step simply redraws.
    const animate = previous !== null
      && previous.view === next.view
      && previous.width === next.width
      && previous.compact === next.compact
      && !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (animate) animateBoardChanges(groups, previous!, next);
    previous = next;
  };

  const installBoardObserver = (): void => {
    new ResizeObserver(() => {
      applyBoardLayout();
      context.onResize();
    }).observe(context.workspace);
  };

  return { applyBoardLayout, installBoardObserver };
}

function boardTiles(section: HTMLElement): HTMLElement[] {
  return [...section.querySelectorAll<HTMLElement>(".service-tile:not(.launch-profile-stack-card)")]
    .filter((tile) => tile.offsetParent !== null);
}

function sectionKey(section: HTMLElement): string | null {
  return section.getAttribute("aria-labelledby");
}

/**
 * Records where every section sits on the board and every card sits in its section. Cards are
 * measured against their section so a card moves on its own only when it changes place inside
 * its group; the section's own movement carries it the rest of the way.
 */
function snapshotBoard(
  board: HTMLElement,
  groups: readonly HTMLElement[],
  meta: Pick<BoardSnapshot, "view" | "width" | "compact">
): BoardSnapshot {
  const boardRect = board.getBoundingClientRect();
  const sections = new Map<string, Offset>();
  const tiles = new Map<string, Offset>();
  for (const section of groups) {
    const key = sectionKey(section);
    if (!key) continue;
    sections.set(key, offsetWithin(section, boardRect));
    const sectionRect = section.getBoundingClientRect();
    for (const tile of boardTiles(section)) {
      const id = tileKey(tile);
      if (id) tiles.set(`${key}\u0000${id}`, offsetWithin(tile, sectionRect));
    }
  }
  return { ...meta, sections, tiles };
}

function animateBoardChanges(groups: readonly HTMLElement[], before: BoardSnapshot, after: BoardSnapshot): void {
  for (const section of groups) {
    const key = sectionKey(section);
    if (!key) continue;
    const from = before.sections.get(key);
    const to = after.sections.get(key);
    if (!from) {
      appear(section);
      continue;
    }
    if (to) slide(section, from, to);
    for (const tile of boardTiles(section)) {
      const id = tileKey(tile);
      if (!id) continue;
      const tileFrom = before.tiles.get(`${key}\u0000${id}`);
      const tileTo = after.tiles.get(`${key}\u0000${id}`);
      if (!tileFrom) appear(tile);
      else if (tileTo) slide(tile, tileFrom, tileTo);
    }
  }
}

function slide(element: HTMLElement, from: Offset, to: Offset): void {
  const dx = from.left - to.left;
  const dy = from.top - to.top;
  if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
  element.animate(
    [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }],
    { duration: MOVE_DURATION_MS, easing: "cubic-bezier(.2, .8, .2, 1)" }
  );
}

function appear(element: HTMLElement): void {
  element.animate([{ opacity: 0 }, { opacity: 1 }], { duration: APPEAR_DURATION_MS, easing: "ease-out" });
}
