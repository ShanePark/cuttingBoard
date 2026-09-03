import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const styles = readFileSync(new URL("../src/styles/header-board.css", import.meta.url), "utf8");
const tileRendering = readFileSync(new URL("../src/tile-rendering.ts", import.meta.url), "utf8");
const servicesRendering = readFileSync(new URL("../src/services-rendering.ts", import.meta.url), "utf8");

test("keeps service icons at their density-sized width", () => {
  assert.match(styles, /\.icon-well \{\s*position: relative;\s*flex: 0 0 auto;\s*width: calc\(62px - 16px \* var\(--tile-density\)\);/);
});

test("truncates long service names to one line while retaining the full title", () => {
  assert.match(styles, /\.tile-name \{\s*display: -webkit-box;\s*-webkit-box-orient: vertical;\s*-webkit-line-clamp: 2;[\s\S]*?overflow-wrap: anywhere;/);
  assert.match(styles, /\.services-view \.service-tile:not\(\.container-tile\) \.tile-name \{\s*display: block;\s*text-overflow: ellipsis;\s*white-space: nowrap;/);
  assert.match(tileRendering, /<h3 class="tile-name" title="\$\{escapeHtml\(title\)\}">\$\{escapeHtml\(title\)\}<\/h3>/);
});

test("puts the full service title on the full-card action hover target", () => {
  assert.match(servicesRendering, /const serviceCardTitle = serviceTitle\(service\) \|\| service\.display_name;/);
  assert.match(servicesRendering, /const overlayTitle = serviceCardTitle;/);
  assert.match(servicesRendering, /aria-label="\$\{h\(tileActionLabel\)\}" title="\$\{h\(overlayTitle\)\}"/);
});
