import assert from "node:assert/strict";
import test from "node:test";
import { boardDensity, boardPlan } from "../src/board-layout.ts";

const GAP = 14;

/** A board exactly wide enough for this many cards at their comfortable width. */
function space(columns: number, baseDensity = 0) {
  return { width: columns * 300 + (columns - 1) * GAP, gap: GAP, baseDensity };
}

test("keeps a board with a handful of cards at its roomy layout", () => {
  assert.equal(boardDensity(0), 0);
  assert.equal(boardDensity(1), 0);
  assert.equal(boardDensity(10), 0);
});

test("compacts the cards gradually as a board fills up", () => {
  const densities = [12, 16, 20, 24].map(boardDensity);
  for (const [index, density] of densities.entries()) {
    assert.ok(density > 0 && density < 1, `expected a partial density, got ${density}`);
    if (index > 0) assert.ok(density > densities[index - 1]!, "expected the density to keep rising");
  }
});

test("stops compacting once the board is full", () => {
  assert.equal(boardDensity(26), 1);
  assert.equal(boardDensity(120), 1);
});

test("splits the groups on screen evenly instead of always taking every column that fits", () => {
  // Three groups of six: four columns would break each of them into 4 + 2, three into 3 + 3.
  assert.deepEqual(boardPlan([6, 6, 6], space(4)), { columns: 3, density: 0 });
  // A three-card group next to a six-card group packs into the same three columns.
  assert.equal(boardPlan([3, 6], space(4)).columns, 3);
  assert.equal(boardPlan([8], space(5)).columns, 4);
});

test("keeps every column the board fits when narrowing would not tidy anything", () => {
  assert.deepEqual(boardPlan([2], space(5)), { columns: 5, density: 0 });
  assert.equal(boardPlan([1, 1, 1, 1, 1, 1], space(4)).columns, 4);
  assert.equal(boardPlan([5, 4], space(5)).columns, 5);
  assert.equal(boardPlan([], space(4)).columns, 4);
});

test("never gives up a row to tidy the groups", () => {
  // Ten cards in three columns leave two holes, but two columns would cost a whole extra row.
  assert.deepEqual(boardPlan([10], space(3)), { columns: 3, density: 0 });
  assert.equal(boardPlan([6, 6, 6], space(6)).columns, 6);
});

test("shrinks the cards when the extra column saves a row", () => {
  // Seven cards and three: three columns need 3 + 3 + 1 and a row of its own for the second
  // group, four columns need 4 + 3 and the same single row, so the cards give up some width.
  const plan = boardPlan([7, 3], { width: 1040, gap: GAP, baseDensity: 0 });
  assert.equal(plan.columns, 4);
  assert.ok(plan.density > 0 && plan.density <= 1, `expected compacted cards, got ${plan.density}`);
});

test("leaves the cards at full size when the extra column saves nothing", () => {
  assert.deepEqual(boardPlan([3], { width: 1040, gap: GAP, baseDensity: 0 }), { columns: 3, density: 0 });
});

test("never drops below the compactness the card count already asks for", () => {
  assert.equal(boardPlan([6, 6, 6], space(4, 0.5)).density, 0.5);
});

test("never returns fewer than one column", () => {
  assert.equal(boardPlan([4], { width: 0, gap: GAP, baseDensity: 0 }).columns, 1);
  assert.equal(boardPlan([4], { width: 300, gap: GAP, baseDensity: 0 }).columns, 1);
});
