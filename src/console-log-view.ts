export type ConsoleSelectionDirection = "forward" | "backward" | "none";

export type ConsoleLogSelection = {
  value: string;
  start: number;
  end: number;
  direction: ConsoleSelectionDirection;
  focused: boolean;
  scrollTop: number;
  scrollLeft: number;
};

function isTextArea(element: HTMLElement | null): element is HTMLTextAreaElement {
  return typeof HTMLTextAreaElement !== "undefined" && element instanceof HTMLTextAreaElement;
}

export function isConsoleLogElement(target: EventTarget | null): target is HTMLTextAreaElement {
  return typeof Element !== "undefined" && target instanceof Element && isTextArea(target as HTMLElement) && target.classList.contains("console-log");
}

/**
 * Return true for keys that would change a log value. Navigation, selection,
 * and clipboard-copy shortcuts intentionally remain native textarea behavior.
 */
export function isConsoleLogMutationKey(event: KeyboardEvent): boolean {
  if (event.isComposing) return true;
  const key = event.key.toLowerCase();
  if (key === "backspace" || key === "delete" || key === "enter") return true;
  if (key === "insert" && event.shiftKey) return true;
  if (key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) return true;
  return (event.ctrlKey || event.metaKey) && ["v", "x", "z", "y"].includes(key);
}

export function consoleLogValue(log: HTMLElement): string {
  return isTextArea(log) ? log.value : log.textContent ?? "";
}

export function setConsoleLogValue(log: HTMLElement, value: string): void {
  if (isTextArea(log)) log.value = value;
  else log.textContent = value;
}

/**
 * Textareas own the scroll position once a log has a native caret. Keeping this
 * helper as a fallback for the old pre-based markup makes DOM patching safe
 * during a hot reload and keeps the controller independent of the markup shape.
 */
export function consoleScrollElement(output: HTMLElement): HTMLElement {
  const log = output.querySelector<HTMLElement>(".console-log");
  return isTextArea(log) ? log : output;
}

export function captureConsoleLogSelection(output: HTMLElement): ConsoleLogSelection | null {
  const log = output.querySelector<HTMLElement>(".console-log");
  if (!isTextArea(log)) return null;
  return {
    value: log.value,
    start: log.selectionStart,
    end: log.selectionEnd,
    direction: log.selectionDirection,
    focused: document.activeElement === log,
    scrollTop: log.scrollTop,
    scrollLeft: log.scrollLeft
  };
}

export function hasConsoleLogSelection(output: HTMLElement): boolean {
  const state = captureConsoleLogSelection(output);
  return Boolean(state && state.start !== state.end);
}

function commonPrefixLength(previous: string, next: string): number {
  const limit = Math.min(previous.length, next.length);
  let index = 0;
  while (index < limit && previous[index] === next[index]) index += 1;
  return index;
}

function suffixPrefixOverlap(previous: string, next: string): number {
  if (!previous || !next) return 0;
  const prefixLengths = new Array<number>(next.length).fill(0);
  for (let index = 1, matched = 0; index < next.length; index += 1) {
    while (matched > 0 && next[index] !== next[matched]) matched = prefixLengths[matched - 1] ?? 0;
    if (next[index] === next[matched]) matched += 1;
    prefixLengths[index] = matched;
  }

  let matched = 0;
  for (let index = 0; index < previous.length; index += 1) {
    while (matched > 0 && previous[index] !== next[matched]) matched = prefixLengths[matched - 1] ?? 0;
    if (previous[index] === next[matched]) matched += 1;
    if (matched === next.length && index < previous.length - 1) matched = prefixLengths[matched - 1] ?? 0;
  }
  return matched;
}

/** Map a caret offset through an append, replacement, or rolling log tail. */
export function mapConsoleOffset(offset: number, previous: string, next: string): number {
  const safeOffset = Math.min(Math.max(offset, 0), previous.length);
  if (previous === next || next.startsWith(previous)) return Math.min(safeOffset, next.length);

  const prefix = commonPrefixLength(previous, next);
  const overlap = suffixPrefixOverlap(previous, next);
  const dropped = previous.length - overlap;
  if (overlap > 0) {
    if (safeOffset < dropped) return Math.min(safeOffset, prefix);
    return Math.min(next.length, Math.max(0, safeOffset - dropped));
  }
  if (safeOffset <= prefix) return safeOffset;
  return Math.min(safeOffset, next.length);
}

export function restoreConsoleLogSelection(output: HTMLElement, state: ConsoleLogSelection | null): void {
  if (!state) return;
  const log = output.querySelector<HTMLElement>(".console-log");
  if (!isTextArea(log)) return;

  const start = mapConsoleOffset(state.start, state.value, log.value);
  const end = mapConsoleOffset(state.end, state.value, log.value);
  const direction = start <= end ? state.direction : state.direction === "forward" ? "backward" : "forward";
  if (state.focused) log.focus({ preventScroll: true });
  log.setSelectionRange(Math.min(start, end), Math.max(start, end), direction);
  log.scrollTop = Math.min(Math.max(0, state.scrollTop), Math.max(0, log.scrollHeight - log.clientHeight));
  log.scrollLeft = Math.min(Math.max(0, state.scrollLeft), Math.max(0, log.scrollWidth - log.clientWidth));
}

function lineStarts(value: string): number[] {
  const starts = [0];
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === "\n") starts.push(index + 1);
  }
  return starts;
}

function lineHeight(textarea: HTMLTextAreaElement): number {
  const parsed = Number.parseFloat(getComputedStyle(textarea).lineHeight);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : 16;
}

function lineIndexAt(starts: readonly number[], offset: number): number {
  let low = 0;
  let high = starts.length - 1;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if ((starts[middle] ?? 0) <= offset) low = middle + 1;
    else high = middle - 1;
  }
  return Math.max(0, high);
}

/** Return the offset at the top or bottom of the visible logical page. */
export function consolePageBoundary(textarea: HTMLTextAreaElement, direction: "top" | "bottom"): number {
  const height = lineHeight(textarea);
  const visibleLines = Math.max(1, Math.floor(textarea.clientHeight / height));
  const firstVisibleLine = Math.max(0, Math.floor(textarea.scrollTop / height));
  const activeOffset = textarea.selectionDirection === "backward" ? textarea.selectionStart : textarea.selectionEnd;
  return consolePageBoundaryOffset(textarea.value, firstVisibleLine, visibleLines, direction, activeOffset);
}

export function consolePageBoundaryOffset(
  value: string,
  firstVisibleLine: number,
  visibleLines: number,
  direction: "top" | "bottom",
  currentOffset = 0
): number {
  const starts = lineStarts(value);
  const currentLine = lineIndexAt(starts, Math.min(Math.max(currentOffset, 0), value.length));
  const currentLineStart = starts[currentLine] ?? 0;
  const currentLineEnd = starts[currentLine + 1] === undefined ? value.length : Math.max(currentLineStart, (starts[currentLine + 1] ?? 1) - 1);
  const column = Math.max(0, Math.min(currentOffset, currentLineEnd) - currentLineStart);
  const pageLine = direction === "top"
    ? Math.min(Math.max(0, firstVisibleLine), starts.length - 1)
    : Math.min(Math.max(0, firstVisibleLine) + Math.max(1, visibleLines) - 1, starts.length - 1);
  const targetStart = starts[pageLine] ?? 0;
  const targetEnd = starts[pageLine + 1] === undefined ? value.length : Math.max(targetStart, (starts[pageLine + 1] ?? 1) - 1);
  return Math.min(targetEnd, targetStart + column);
}

export function revealConsoleCaret(textarea: HTMLTextAreaElement, offset: number, direction: "top" | "bottom"): void {
  const starts = lineStarts(textarea.value);
  const line = lineIndexAt(starts, Math.min(Math.max(offset, 0), textarea.value.length));
  const top = line * lineHeight(textarea);
  textarea.scrollTop = direction === "top"
    ? Math.max(0, top)
    : Math.max(0, top - textarea.clientHeight + lineHeight(textarea));
}

export function moveConsoleCaret(textarea: HTMLTextAreaElement, offset: number, extend: boolean): void {
  const target = Math.min(Math.max(offset, 0), textarea.value.length);
  const selection = consoleSelectionForMove(
    textarea.selectionStart,
    textarea.selectionEnd,
    textarea.selectionDirection,
    target,
    extend
  );
  textarea.setSelectionRange(selection.start, selection.end, selection.direction);
}

export function consoleSelectionForMove(
  start: number,
  end: number,
  direction: ConsoleSelectionDirection,
  target: number,
  extend: boolean
): { start: number; end: number; direction: ConsoleSelectionDirection } {
  if (!extend) return { start: target, end: target, direction: "none" };
  const anchor = direction === "backward" ? end : start;
  const nextDirection: ConsoleSelectionDirection = target < anchor ? "backward" : target > anchor ? "forward" : "none";
  return {
    start: Math.min(anchor, target),
    end: Math.max(anchor, target),
    direction: nextDirection
  };
}
