import assert from "node:assert/strict";
import test from "node:test";
import { clampListScrollTop, createListScroll } from "../src/list-scroll.ts";

function fakeList(scrollTop: number, scrollHeight = 2400, clientHeight = 600) {
  return { scrollTop, scrollHeight, clientHeight };
}

test("clamps a preserved list position to the new content bounds", () => {
  assert.equal(clampListScrollTop(1500, { scrollHeight: 2400, clientHeight: 600 }), 1500);
  assert.equal(clampListScrollTop(1500, { scrollHeight: 900, clientHeight: 600 }), 300);
  assert.equal(clampListScrollTop(-20, { scrollHeight: 2400, clientHeight: 600 }), 0);
});

test("restores the position after the view re-renders its markup", () => {
  let list = fakeList(820);
  const scroll = createListScroll(() => list, () => "docker");
  scroll.capture();
  list = fakeList(0);
  scroll.restore("docker");
  assert.equal(list.scrollTop, 820);
});

test("does not carry a position over to another view", () => {
  let list = fakeList(820);
  let view = "docker";
  const scroll = createListScroll(() => list, () => view);
  scroll.capture();
  view = "services";
  list = fakeList(0);
  scroll.restore("services");
  assert.equal(list.scrollTop, 0);
});

test("ignores a capture taken while no view is mounted", () => {
  let list: ReturnType<typeof fakeList> | null = fakeList(820);
  const scroll = createListScroll(() => list, () => null);
  scroll.capture();
  list = fakeList(0);
  scroll.restore("docker");
  assert.equal(list.scrollTop, 0);
});
