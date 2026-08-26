import assert from "node:assert/strict";
import test from "node:test";
import {
  clampConsoleScrollTop,
  isConsoleAtBottom,
  scrollTopForConsoleUpdate
} from "../src/console-scroll.ts";

test("treats a small subpixel remainder as the bottom", () => {
  assert.equal(isConsoleAtBottom({ scrollHeight: 1002, clientHeight: 400, scrollTop: 600 }), true);
  assert.equal(isConsoleAtBottom({ scrollHeight: 1003, clientHeight: 400, scrollTop: 600 }), false);
});

test("clamps a preserved scroll position to the new output bounds", () => {
  assert.equal(clampConsoleScrollTop(800, 900, 400), 500);
  assert.equal(clampConsoleScrollTop(-10, 900, 400), 0);
});

test("follows the new bottom only when follow mode is enabled", () => {
  const metrics = { scrollHeight: 1200, clientHeight: 400 };
  assert.equal(scrollTopForConsoleUpdate(metrics, 140, true), 800);
  assert.equal(scrollTopForConsoleUpdate(metrics, 140, false), 140);
});
