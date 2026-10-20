// Headless screenshots of the frontend against a mocked demo backend, for checking UI changes
// without opening a window on the desktop (Chromium headless shell only).
// Usage: npm run screenshots -- [outdir] [baseUrl]
// The Vite dev server must already be running (npm run dev, port 1420).
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { installTauriMock } from "./mock-tauri.mjs";

const outDir = resolve(process.argv[2] ?? join(tmpdir(), "cutting-board-screenshots"));
const baseUrl = process.argv[3] ?? "http://127.0.0.1:1420";
mkdirSync(outDir, { recursive: true });

try {
  await fetch(baseUrl);
} catch {
  console.error(`No dev server answered at ${baseUrl}. Start it with \`npm run dev\` first.`);
  process.exit(1);
}

const browser = await chromium.launch({ headless: true });
const problems = [];

async function openApp(themeMode, viewport) {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: 1,
    colorScheme: themeMode === "light" ? "light" : "dark",
    reducedMotion: "reduce"
  });
  await context.addInitScript(installTauriMock, { themeMode });
  const page = await context.newPage();
  page.on("pageerror", (error) => problems.push(`[${themeMode}] pageerror: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") problems.push(`[${themeMode}] console: ${message.text()}`);
  });
  await page.goto(baseUrl, { waitUntil: "load" });
  await page.waitForSelector(".service-section", { timeout: 15000 });
  // Bootstrap is done once the system metrics poll has written a value.
  await page.waitForFunction(
    () => document.querySelector('[data-system-metric-value="cpu"]')?.textContent !== "—",
    null,
    { timeout: 15000 }
  );
  await settle(page);
  return { context, page };
}

async function settle(page) {
  await page.evaluate(() => document.fonts?.ready);
  await page.waitForTimeout(400);
}

async function shot(page, name) {
  const path = `${outDir}/${name}.png`;
  await page.screenshot({ path });
  const fatal = await page.evaluate(() => /could not start/i.test(document.body.innerText));
  if (fatal) problems.push(`${name}: fatal bootstrap screen visible`);
  console.log(path);
}

async function closeModal(page) {
  await page.keyboard.press("Escape");
  await page.waitForSelector(".modal-backdrop", { state: "detached", timeout: 5000 }).catch(() => undefined);
  await settle(page);
}

async function clickTab(page, tab) {
  await page.click(`[data-tab="${tab}"]`);
  await settle(page);
}

for (const theme of ["dark", "light"]) {
  const { context, page } = await openApp(theme, { width: 1080, height: 720 });

  await shot(page, `${theme}-1-services`);

  await page.click('[data-action="service-details"]');
  await page.waitForSelector(".modal-backdrop");
  await settle(page);
  await shot(page, `${theme}-6-service-details`);
  await closeModal(page);

  await page.locator('[data-action="select-service"]').first().click();
  await settle(page);
  await shot(page, `${theme}-2-services-selected`);

  await clickTab(page, "docker");
  await shot(page, `${theme}-3-docker`);

  await clickTab(page, "launch");
  await shot(page, `${theme}-4-launch`);

  await page.click('[data-action="settings"]');
  await page.waitForSelector(".modal-backdrop");
  await settle(page);
  await shot(page, `${theme}-5-settings`);
  await closeModal(page);

  await context.close();
}

{
  const { context, page } = await openApp("dark", { width: 600, height: 720 });
  await shot(page, "dark-7-services-600");
  await context.close();
}

await browser.close();
if (problems.length) {
  console.error("Problems:\n" + problems.join("\n"));
  process.exitCode = 1;
}
