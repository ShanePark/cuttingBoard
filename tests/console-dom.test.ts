import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import * as typescript from "typescript";

function moduleUrl(source: string): string {
  const output = typescript.transpileModule(source, {
    compilerOptions: { module: typescript.ModuleKind.ESNext, target: typescript.ScriptTarget.ES2022 }
  }).outputText;
  return `data:text/javascript;charset=utf-8,${encodeURIComponent(output)}`;
}

const scrollModuleUrl = moduleUrl(readFileSync(new URL("../src/console-scroll.ts", import.meta.url), "utf8"));
const logViewModuleUrl = moduleUrl(readFileSync(new URL("../src/console-log-view.ts", import.meta.url), "utf8"));
const consoleDomSource = readFileSync(new URL("../src/console-dom.ts", import.meta.url), "utf8")
  .replace('from "./console-scroll"', `from "${scrollModuleUrl}"`)
  .replace('from "./console-log-view"', `from "${logViewModuleUrl}"`);
const { patchConsoleOutput } = await import(moduleUrl(consoleDomSource));

class FakeClassList {
  private readonly names: Set<string>;

  constructor(...names: string[]) {
    this.names = new Set(names);
  }

  contains(name: string): boolean {
    return this.names.has(name);
  }
}

class FakeElement {
  readonly tagName: string;
  readonly classList: FakeClassList;
  readonly dataset: Record<string, string> = {};
  readonly children: FakeElement[] = [];
  textContent = "";
  scrollTop = 0;
  scrollLeft = 0;
  scrollHeight = 100;
  clientHeight = 40;
  scrollWidth = 100;
  clientWidth = 80;

  constructor(tagName: string, ...classNames: string[]) {
    this.tagName = tagName;
    this.classList = new FakeClassList(...classNames);
  }

  appendChild(child: FakeElement): void {
    this.children.push(child);
  }

  querySelector<T extends FakeElement>(selector: string): T | null {
    for (const child of this.children) {
      if (child.matches(selector)) return child as T;
      const nested = child.querySelector<T>(selector);
      if (nested) return nested;
    }
    return null;
  }

  private matches(selector: string): boolean {
    if (selector.startsWith(".")) return this.classList.contains(selector.slice(1));
    return this.tagName.toLowerCase() === selector.toLowerCase();
  }
}

class FakeTextArea extends FakeElement {
  value: string;
  selectionStart = 0;
  selectionEnd = 0;
  selectionDirection: "forward" | "backward" | "none" = "none";

  constructor(value: string) {
    super("TEXTAREA", "console-log");
    this.value = value;
  }

  focus(): void {
    (globalThis as unknown as { document: FakeDocument }).document.activeElement = this;
  }

  setSelectionRange(start: number, end: number, direction: "forward" | "backward" | "none"): void {
    this.selectionStart = start;
    this.selectionEnd = end;
    this.selectionDirection = direction;
  }
}

class FakeTemplate extends FakeElement {
  readonly content = new FakeElement("#fragment");

  set innerHTML(markup: string) {
    const alertText = markup.match(/class="[^"]*console-alert[^"]*"[^>]*>[\s\S]*?<span>([\s\S]*?)<\/span>/)?.[1];
    if (alertText !== undefined) {
      const alert = new FakeElement("DIV", "console-alert");
      const text = new FakeElement("SPAN");
      text.textContent = alertText;
      alert.appendChild(text);
      this.content.appendChild(alert);
    }
  }
}

class FakeDocument {
  activeElement: FakeElement | null = null;
  templateCreations = 0;

  createElement(tagName: string): FakeTemplate {
    assert.equal(tagName, "template");
    this.templateCreations += 1;
    return new FakeTemplate("TEMPLATE");
  }
}

function withFakeDom(callback: (document: FakeDocument) => void): void {
  const globals = globalThis as unknown as Record<string, unknown>;
  const previousDocument = globals.document;
  const previousTextArea = globals.HTMLTextAreaElement;
  const document = new FakeDocument();
  globals.document = document;
  globals.HTMLTextAreaElement = FakeTextArea;
  try {
    callback(document);
  } finally {
    if (previousDocument === undefined) delete globals.document;
    else globals.document = previousDocument;
    if (previousTextArea === undefined) delete globals.HTMLTextAreaElement;
    else globals.HTMLTextAreaElement = previousTextArea;
  }
}

function alertElement(textContent: string): FakeElement {
  const alert = new FakeElement("DIV", "console-alert");
  const text = new FakeElement("SPAN");
  text.textContent = textContent;
  alert.appendChild(text);
  return alert;
}

function outputWithLog(kind: string, value: string, alertText?: string): { output: FakeElement; log: FakeTextArea } {
  const output = new FakeElement("DIV", "console-output");
  output.dataset.consoleOutputKind = kind;
  if (alertText !== undefined) output.appendChild(alertElement(alertText));
  const log = new FakeTextArea(value);
  output.appendChild(log);
  return { output, log };
}

test("skips template parsing when a plain log is unchanged", () => {
  withFakeDom((document) => {
    const { output, log } = outputWithLog("log", "same log");
    output.dataset.consoleStatus = "running";
    log.scrollTop = 12;

    patchConsoleOutput(output as HTMLElement, '<textarea class="console-log">same log</textarea>', "log", "same log", true);

    assert.equal(document.templateCreations, 0);
    assert.equal(output.dataset.consoleOutputKind, "log");
    assert.equal(output.dataset.consoleStatus, "running");
    assert.equal(output.classList.contains("console-output"), true);
    assert.equal(log.value, "same log");
    assert.equal(log.scrollTop, 12);
  });
});

test("preserves changed log selection and scroll while updating its value", () => {
  withFakeDom((document) => {
    const { output, log } = outputWithLog("log", "before\n");
    log.selectionStart = 2;
    log.selectionEnd = 5;
    log.selectionDirection = "forward";
    log.scrollTop = 9;
    document.activeElement = log;

    patchConsoleOutput(output as HTMLElement, '<textarea class="console-log">before\nafter\n</textarea>', "log", "before\nafter\n", false);

    assert.equal(document.templateCreations, 1);
    assert.equal(log.value, "before\nafter\n");
    assert.equal(log.selectionStart, 2);
    assert.equal(log.selectionEnd, 5);
    assert.equal(log.selectionDirection, "forward");
    assert.equal(log.scrollTop, 9);
    assert.equal(document.activeElement, log);
  });
});

test("preserves a backward selection and scroll when a log tail rolls forward", () => {
  withFakeDom((document) => {
    const previous = "discarded\nstable line\n";
    const next = "stable line\nafter line\n";
    const { output, log } = outputWithLog("log", previous);
    log.selectionStart = 4;
    log.selectionEnd = previous.length;
    log.selectionDirection = "backward";
    log.scrollTop = 9;
    document.activeElement = log;

    patchConsoleOutput(output as HTMLElement, `<textarea class="console-log">${next}</textarea>`, "log", next, false);

    assert.equal(log.selectionStart, 0);
    assert.equal(log.selectionEnd, "stable line\n".length);
    assert.equal(log.selectionDirection, "backward");
    assert.equal(log.scrollTop, 9);
    assert.equal(document.activeElement, log);
  });
});

test("keeps alert patching for changed log-alert output", () => {
  withFakeDom((document) => {
    const { output, log } = outputWithLog("log-alert", "old log", "old alert");

    patchConsoleOutput(
      output as HTMLElement,
      '<div class="console-alert"><span>new alert</span></div><textarea class="console-log">new log</textarea>',
      "log-alert",
      "new log",
      false
    );

    assert.equal(document.templateCreations, 1);
    assert.equal(output.querySelector(".console-alert")?.querySelector("span")?.textContent, "new alert");
    assert.equal(log.value, "new log");
  });
});
