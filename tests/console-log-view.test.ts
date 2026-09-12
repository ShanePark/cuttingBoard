import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  consolePageBoundaryOffset,
  consolePageBoundaryOffsetForStarts,
  consoleSelectionForMove,
  findConsoleLogMatches,
  isConsoleLogMutationKey,
  mapConsoleOffset,
  mapConsoleOffsetPair
} from "../src/console-log-view.ts";

test("all log renderers expose a caret-enabled read-only text area", () => {
  for (const path of ["../src/launch-rendering.ts", "../src/services-rendering.ts", "../src/docker-rendering.ts"]) {
    const source = readFileSync(new URL(path, import.meta.url), "utf8");
    assert.match(source, /<textarea class=\"console-log\" aria-readonly=\"true\" spellcheck=\"false\" wrap=\"soft\"/);
    assert.doesNotMatch(source, /<textarea class=\"console-log\" readonly/);
  }
});

test("blocks edits while leaving navigation and copy shortcuts native", () => {
  const key = (value: string, options: Partial<KeyboardEvent> = {}): KeyboardEvent => ({
    key: value,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    shiftKey: false,
    isComposing: false,
    ...options
  } as KeyboardEvent);
  assert.equal(isConsoleLogMutationKey(key("a")), true);
  assert.equal(isConsoleLogMutationKey(key("Backspace")), true);
  assert.equal(isConsoleLogMutationKey(key("v", { ctrlKey: true })), true);
  assert.equal(isConsoleLogMutationKey(key("ArrowLeft")), false);
  assert.equal(isConsoleLogMutationKey(key("ArrowLeft", { shiftKey: true })), false);
  assert.equal(isConsoleLogMutationKey(key("c", { ctrlKey: true })), false);
  assert.equal(isConsoleLogMutationKey(key("Home")), false);
});

test("finds case-insensitive literal matches in a log", () => {
  assert.deepEqual(findConsoleLogMatches("Boot ERROR\nerror: retry", "error"), [
    { start: 5, end: 10 },
    { start: 11, end: 16 }
  ]);
  assert.deepEqual(findConsoleLogMatches("a+b a.b", "a+b"), [{ start: 0, end: 3 }]);
  assert.deepEqual(findConsoleLogMatches("aaaa", "aa"), [
    { start: 0, end: 2 },
    { start: 2, end: 4 }
  ]);
  assert.deepEqual(findConsoleLogMatches("aaab", "aab"), [{ start: 1, end: 4 }]);
  assert.deepEqual(findConsoleLogMatches("K", "k"), [{ start: 0, end: 1 }]);
  assert.deepEqual(findConsoleLogMatches("matching text", ""), []);
});

test("preserves selection offsets when a live log grows", () => {
  assert.equal(mapConsoleOffset(4, "first line\n", "first line\nsecond line\n"), 4);
  assert.equal(mapConsoleOffset(11, "first line\n", "first line\nsecond line\n"), 11);
});

test("maps selection offsets into a rolling log tail", () => {
  const previous = "discarded\nstable line\n";
  const next = "stable line\nafter line\n";
  assert.equal(mapConsoleOffset(4, previous, next), 0);
  assert.equal(mapConsoleOffset(previous.length - "stable line\n".length, previous, next), 0);
  assert.equal(mapConsoleOffset(previous.length, previous, next), "stable line\n".length);
});

test("maps both selection endpoints through a rolling log tail", () => {
  const previous = "discarded\nstable line\n";
  const next = "stable line\nafter line\n";

  assert.deepEqual(mapConsoleOffsetPair(4, previous.length, previous, next), {
    start: 0,
    end: "stable line\n".length
  });
});

test("moves Ctrl+Page navigation to the visible page while retaining the caret column", () => {
  const value = "zero\nfirst line\nsecond line\nthird\n";
  assert.equal(consolePageBoundaryOffset(value, 1, 2, "top", 12), 12);
  assert.equal(consolePageBoundaryOffset(value, 1, 2, "bottom", 12), 23);
});

test("keeps Ctrl+Page navigation inside wrapped visual lines", () => {
  const value = "0123456789\nnext";
  const wrappedStarts = [0, 5, 10, 11];
  assert.equal(consolePageBoundaryOffsetForStarts(value, wrappedStarts, 1, 2, "top", 7), 7);
  assert.equal(consolePageBoundaryOffsetForStarts(value, wrappedStarts, 1, 2, "bottom", 7), 10);
});

test("Shift movement extends and reverses the existing selection from its anchor", () => {
  assert.deepEqual(consoleSelectionForMove(2, 5, "forward", 8, true), {
    start: 2,
    end: 8,
    direction: "forward"
  });
  assert.deepEqual(consoleSelectionForMove(2, 5, "backward", 1, true), {
    start: 1,
    end: 5,
    direction: "backward"
  });
  assert.deepEqual(consoleSelectionForMove(2, 5, "backward", 8, true), {
    start: 5,
    end: 8,
    direction: "forward"
  });
});
