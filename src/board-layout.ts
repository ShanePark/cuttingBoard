const COMFORTABLE_TILE_WIDTH = 300;
const COMPACT_TILE_WIDTH = 248;
// A board with only a handful of cards keeps its roomy layout. Past that the cards compact
// gradually with every extra card, so a busy machine still shows most of its work at once.
const COMFORTABLE_TILE_COUNT = 10;
const COMPACT_TILE_COUNT = 26;

export type BoardLayoutContext = {
  workspace: HTMLElement;
  onResize: () => void;
};

/** How compact a board holding this many cards should be, from 0 (roomy) to 1 (fully compact). */
export function boardDensity(tileCount: number): number {
  const ratio = (tileCount - COMFORTABLE_TILE_COUNT) / (COMPACT_TILE_COUNT - COMFORTABLE_TILE_COUNT);
  return Math.round(Math.min(1, Math.max(0, ratio)) * 100) / 100;
}

export type BoardSpace = {
  /** Width available to the cards, gaps included. */
  width: number;
  gap: number;
  /** How compact the cards are before the layout asks for anything narrower. */
  baseDensity: number;
};

export type BoardPlan = { columns: number; density: number };

/**
 * Picks the column count that suits the groups on screen, and how compact the cards have to be
 * to reach it. A group spans one column per card, so the column count decides how each group
 * breaks up: four columns split a six-card group into 4 + 2, three columns into an even 3 + 3.
 * Candidates are scored on the rows they cost first, then on the holes they leave inside groups,
 * then on how far the cards have to shrink — so cards only give up size to save a row.
 */
export function boardPlan(groupSizes: readonly number[], space: BoardSpace): BoardPlan {
  const widest = Math.max(1, Math.floor((space.width + space.gap) / (COMPACT_TILE_WIDTH + space.gap)));
  let best: BoardPlan & BoardCost = { columns: 1, density: densityForColumns(1, space), ...estimateBoardCost(groupSizes, 1) };
  for (let columns = 2; columns <= widest; columns++) {
    const candidate = { columns, density: densityForColumns(columns, space), ...estimateBoardCost(groupSizes, columns) };
    if (isTidier(candidate, best)) best = candidate;
  }
  return { columns: best.columns, density: best.density };
}

function isTidier(candidate: BoardPlan & BoardCost, best: BoardPlan & BoardCost): boolean {
  if (candidate.height !== best.height) return candidate.height < best.height;
  if (candidate.gaps !== best.gaps) return candidate.gaps < best.gaps;
  if (candidate.density !== best.density) return candidate.density < best.density;
  return candidate.columns > best.columns;
}

/** How compact the cards have to be for this many of them to fit side by side. */
function densityForColumns(columns: number, space: BoardSpace): number {
  const cardWidth = (space.width - (columns - 1) * space.gap) / columns;
  const shortfall = (COMFORTABLE_TILE_WIDTH - cardWidth) / (COMFORTABLE_TILE_WIDTH - COMPACT_TILE_WIDTH);
  // Rounded up, so the cards are never left a fraction of a pixel too wide for the column.
  const needed = Math.ceil(Math.min(1, Math.max(0, shortfall)) * 100) / 100;
  return Math.max(space.baseDensity, needed);
}

type BoardRow = { free: number; height: number };
type BoardCost = { height: number; gaps: number };

// Heights are counted in half-card rows so a group header still weighs something next to the
// cards below it, and gaps are the cells a group leaves empty on its own last row.
const GROUP_HEADER_HEIGHT = 1;
const CARD_ROW_HEIGHT = 2;

function estimateBoardCost(groupSizes: readonly number[], columns: number): BoardCost {
  const rows: BoardRow[] = [];
  let gaps = 0;
  for (const size of groupSizes) {
    const span = Math.min(Math.max(1, size), columns);
    const cardRows = Math.ceil(size / span);
    gaps += span * cardRows - size;
    // Groups land where `grid-auto-flow: row dense` puts them: the first row with room to spare.
    const row = rows.find((candidate) => candidate.free >= span) ?? addBoardRow(rows, columns);
    row.free -= span;
    row.height = Math.max(row.height, GROUP_HEADER_HEIGHT + cardRows * CARD_ROW_HEIGHT);
  }
  return { height: rows.reduce((total, row) => total + row.height, 0), gaps };
}

function addBoardRow(rows: BoardRow[], columns: number): BoardRow {
  const row = { free: columns, height: 0 };
  rows.push(row);
  return row;
}

export function createBoardLayout(context: BoardLayoutContext) {
  const applyBoardLayout = (): void => {
    const board = context.workspace.querySelector<HTMLElement>(".board");
    if (!board) return;
    // The board is measured at the density the card count alone asks for. Planning a denser
    // layout only shrinks the padding and the gap further, so the room measured here is never
    // more than the room the chosen layout gets.
    const baseDensity = boardDensity(board.querySelectorAll(".service-tile").length);
    board.style.setProperty("--tile-density", String(baseDensity));
    const styles = window.getComputedStyle(board);
    const gap = Number.parseFloat(styles.columnGap) || 0;
    const width = board.clientWidth - Number.parseFloat(styles.paddingLeft) - Number.parseFloat(styles.paddingRight);
    // The add-a-profile card of the launch view takes part in the packing as a group of one.
    const groups = [...board.querySelectorAll<HTMLElement>(".service-section, .launch-add-card")];
    const plan = boardPlan(groups.map((group) => Number(group.dataset.tiles) || 1), { width, gap, baseDensity });
    board.style.setProperty("--tile-density", String(plan.density));
    board.style.setProperty("--board-columns", String(plan.columns));
    for (const section of board.querySelectorAll<HTMLElement>(".service-section")) {
      const tiles = Number(section.dataset.tiles) || 1;
      section.style.setProperty("--section-span", String(Math.min(Math.max(1, tiles), plan.columns)));
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
