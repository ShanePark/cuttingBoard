import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const servicesRendering = readFileSync(new URL("../src/services-rendering.ts", import.meta.url), "utf8");
const uiSupport = readFileSync(new URL("../src/ui-support.ts", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("service restart control is declared before stop", () => {
  const restart = servicesRendering.indexOf('data-action="restart-service"');
  const stop = servicesRendering.indexOf('data-action="stop-service"', restart);

  assert.notEqual(restart, -1);
  assert.notEqual(stop, -1);
  assert.ok(restart < stop);
});

test("service restart controls route request and confirmation actions", () => {
  assert.match(main, /action === "restart-service"\) serviceActions\.requestRestartService/);
  assert.match(main, /action === "confirm-restart-service"\) await serviceActions\.confirmRestartService/);
});

test("live service metrics preserve the restarting state", () => {
  assert.match(uiSupport, /const operations = context\.getOperations\(\);[\s\S]*uptimeText\(\s*service,\s*operations\.has\(`stop:\$\{service\.id\}`\),\s*operations\.has\(`restart:\$\{service\.id\}`\)/);
});
