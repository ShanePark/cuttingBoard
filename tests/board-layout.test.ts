import assert from "node:assert/strict";
import test from "node:test";
import { boardColumns, boardIsCompact } from "../src/board-layout.ts";

test("keeps a board of up to twelve cards at the roomy size", () => {
  for (const count of [0, 1, 6, 10, 12]) assert.equal(boardIsCompact(count, undefined), false, `${count} cards`);
});

test("turns compact once a board holds thirteen cards", () => {
  assert.equal(boardIsCompact(13, undefined), true);
  assert.equal(boardIsCompact(13, false), true);
  assert.equal(boardIsCompact(120, false), true);
});

test("stays compact until the board drops to nine cards", () => {
  for (const count of [12, 11, 10]) assert.equal(boardIsCompact(count, true), true, `${count} cards`);
  assert.equal(boardIsCompact(9, true), false);
  assert.equal(boardIsCompact(0, true), false);
});

test("stays roomy while a card count wavers below thirteen", () => {
  for (const count of [10, 11, 12, 11, 12]) assert.equal(boardIsCompact(count, false), false, `${count} cards`);
});

test("takes its columns from the room alone", () => {
  // A 1056px window leaves about 1024px for the cards.
  assert.equal(boardColumns(1024, 12, false), 3);
  assert.equal(boardColumns(1024, 8, true), 4);
  assert.equal(boardColumns(300 * 4 + 12 * 3, 12, false), 4);
  assert.equal(boardColumns(300 * 4 + 12 * 3 - 1, 12, false), 3);
});

test("never returns fewer than one column", () => {
  assert.equal(boardColumns(0, 12, false), 1);
  assert.equal(boardColumns(200, 8, true), 1);
});
