import assert from "node:assert/strict";
import { reconcileConsoleLog } from "../src/console-log-presentation.ts";
import { mapConsoleOffsetPair } from "../src/console-log-view.ts";

const KIB = 1024;
const MIB = KIB * KIB;
const REALISTIC_SIZES = [64 * KIB, 96 * KIB];
const STRESS_SIZES = [512 * KIB, 2 * MIB];
const REALISTIC_ITERATIONS = 24;
const STRESS_ITERATIONS = 10;
const WARMUP_ITERATIONS = 6;

function renderConsoleLog(source, lineBreakOffsets) {
  let output = "";
  let sourceOffset = 0;
  for (const lineBreakOffset of lineBreakOffsets) {
    output += source.slice(sourceOffset, lineBreakOffset) + "\n";
    sourceOffset = lineBreakOffset;
  }
  return output + source.slice(sourceOffset);
}

function normalizeLineEndings(value) {
  return value.replace(/\r\n?/g, "\n");
}

// This is the pre-change implementation kept in the benchmark so every run
// compares the production helper with the exact old algorithm.
function legacySuffixPrefixOverlap(previous, next) {
  if (!previous || !next) return 0;
  const prefixLengths = new Array(next.length).fill(0);
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

function legacyCommonPrefixLength(previous, next) {
  const limit = Math.min(previous.length, next.length);
  let index = 0;
  while (index < limit && previous[index] === next[index]) index += 1;
  return index;
}

function legacyMapConsoleOffset(offset, previous, next) {
  const safeOffset = Math.min(Math.max(offset, 0), previous.length);
  if (previous === next || next.startsWith(previous)) return Math.min(safeOffset, next.length);
  const prefix = legacyCommonPrefixLength(previous, next);
  const overlap = legacySuffixPrefixOverlap(previous, next);
  const dropped = previous.length - overlap;
  if (overlap > 0) {
    if (safeOffset < dropped) return Math.min(safeOffset, prefix);
    return Math.min(next.length, Math.max(0, safeOffset - dropped));
  }
  if (safeOffset <= prefix) return safeOffset;
  return Math.min(safeOffset, next.length);
}

function legacyMapConsoleOffsetPair(start, end, previous, next) {
  // The old restore path called mapConsoleOffset separately for each endpoint.
  return {
    start: legacyMapConsoleOffset(start, previous, next),
    end: legacyMapConsoleOffset(end, previous, next)
  };
}

function legacyReconcileConsoleLog(presentation, source) {
  const normalizedSource = normalizeLineEndings(source);
  if (!presentation) {
    return {
      source: normalizedSource,
      output: renderConsoleLog(normalizedSource, []),
      lineBreakOffsets: []
    };
  }
  if (normalizedSource === presentation.source) return presentation;
  if (normalizedSource.startsWith(presentation.source)) {
    return {
      source: normalizedSource,
      output: renderConsoleLog(normalizedSource, presentation.lineBreakOffsets),
      lineBreakOffsets: presentation.lineBreakOffsets
    };
  }
  const overlap = legacySuffixPrefixOverlap(presentation.source, normalizedSource);
  if (overlap >= 64) {
    const droppedLength = presentation.source.length - overlap;
    const remainingLineBreaks = presentation.lineBreakOffsets
      .filter((offset) => offset >= droppedLength)
      .map((offset) => offset - droppedLength);
    return {
      source: normalizedSource,
      output: renderConsoleLog(normalizedSource, remainingLineBreaks),
      lineBreakOffsets: remainingLineBreaks
    };
  }
  return {
    source: normalizedSource,
    output: renderConsoleLog(normalizedSource, []),
    lineBreakOffsets: []
  };
}

function seededLog(length, seed) {
  let state = seed >>> 0;
  let line = "";
  let output = "";
  while (output.length < length) {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    const request = state.toString(16).padStart(8, "0");
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    const duration = state % 1000;
    line = `2026-09-12T12:00:${String(state % 60).padStart(2, "0")}Z INFO worker request=${request} status=200 duration=${duration}ms\n`;
    output += line;
  }
  return output.slice(0, length);
}

function rollingFixture(length) {
  const shift = Math.max(1024, Math.floor(length / 16));
  const previous = seededLog(length, length + 17);
  const suffix = seededLog(shift, length + 29);
  return { previous, next: previous.slice(shift) + suffix, shift };
}

function lineBreaks(value) {
  const offsets = [];
  for (let index = 0; index < value.length; index += 1) {
    if (value[index] === "\n") offsets.push(index);
  }
  return offsets;
}

function presentationFixture(previous, next) {
  const lineBreakOffsets = lineBreaks(previous).filter((_, index) => index % 3 === 0);
  return {
    previous: {
      source: previous,
      output: renderConsoleLog(previous, lineBreakOffsets),
      lineBreakOffsets
    },
    next
  };
}

function mapFixtures() {
  const fixtures = [
    {
      label: "empty",
      previous: "",
      next: "",
      pairs: [[-2, 3], [0, 0]]
    },
    {
      label: "append",
      previous: "first line\n",
      next: "first line\nsecond line\n",
      pairs: [[-4, 4], [0, 11], [11, 100]]
    },
    {
      label: "replacement",
      previous: "old output\n",
      next: "new output\n",
      pairs: [[-4, 4], [2, 11], [11, 100]]
    },
    {
      label: "unicode-crlf-ansi",
      previous: "\u001b[32mready 😀\r\nαβγ\r\n",
      next: "\u001b[32mready 😀\r\nαβγ\r\nlatest ✅\r\n",
      pairs: [[-4, 4], [5, 14], [20, 100]]
    },
    {
      label: "repeated",
      previous: "abababababababababab",
      next: "babababababababababa",
      pairs: [[0, 3], [4, 16], [20, 30]]
    }
  ];
  for (const size of [...REALISTIC_SIZES, ...STRESS_SIZES]) {
    const fixture = rollingFixture(size);
    fixtures.push({
      label: `${size / KIB} KiB rolling`,
      previous: fixture.previous,
      next: fixture.next,
      pairs: [[-8, Math.floor(fixture.shift / 2)], [Math.floor(fixture.shift / 2), fixture.previous.length - Math.floor(fixture.shift / 2)], [fixture.previous.length, fixture.previous.length + 8]]
    });
  }
  return fixtures;
}

function assertMapEquivalence(fixtures) {
  for (const fixture of fixtures) {
    for (const [start, end] of fixture.pairs) {
      assert.deepEqual(
        mapConsoleOffsetPair(start, end, fixture.previous, fixture.next),
        legacyMapConsoleOffsetPair(start, end, fixture.previous, fixture.next),
        `${fixture.label} selection ${start}:${end}`
      );
    }
  }
}

function assertPresentationEquivalence() {
  const append = presentationFixture("first line\n", "first line\nsecond line\n");
  const replace = presentationFixture("old output\n", "new output\n");
  const unicode = presentationFixture("ready 😀\r\nαβγ\r\n", "ready 😀\r\nαβγ\r\nlatest ✅\r\n");
  const fixtures = [append, replace, unicode];
  for (const size of [...REALISTIC_SIZES, ...STRESS_SIZES]) {
    const rolling = rollingFixture(size);
    fixtures.push(presentationFixture(rolling.previous, rolling.next));
  }
  for (const fixture of fixtures) {
    assert.deepEqual(
      reconcileConsoleLog(fixture.previous, fixture.next),
      legacyReconcileConsoleLog(fixture.previous, fixture.next),
      `presentation ${fixture.previous.source.length} -> ${fixture.next.length}`
    );
  }
}

function elapsedMilliseconds(callback) {
  const started = process.hrtime.bigint();
  callback();
  return Number(process.hrtime.bigint() - started) / 1e6;
}

function percentile(samples, fraction) {
  const sorted = [...samples].sort((left, right) => left - right);
  const position = (sorted.length - 1) * fraction;
  const lower = Math.floor(position);
  const upper = Math.ceil(position);
  if (lower === upper) return sorted[lower] ?? 0;
  return (sorted[lower] ?? 0) + ((sorted[upper] ?? 0) - (sorted[lower] ?? 0)) * (position - lower);
}

function benchmark(
  label,
  legacy,
  current,
  iterations,
  legacyBytesPerCall,
  currentBytesPerCall,
  rebuiltBytesPerCall = 0
) {
  for (let index = 0; index < WARMUP_ITERATIONS; index += 1) {
    legacy();
    current();
  }

  const legacySamples = [];
  const currentSamples = [];
  for (let index = 0; index < iterations; index += 1) {
    if (index % 2 === 0) {
      legacySamples.push(elapsedMilliseconds(legacy));
      currentSamples.push(elapsedMilliseconds(current));
    } else {
      currentSamples.push(elapsedMilliseconds(current));
      legacySamples.push(elapsedMilliseconds(legacy));
    }
  }

  const legacyMedian = percentile(legacySamples, 0.5);
  const currentMedian = percentile(currentSamples, 0.5);
  const legacyP95 = percentile(legacySamples, 0.95);
  const currentP95 = percentile(currentSamples, 0.95);
  const speedup = currentMedian === 0 ? Infinity : legacyMedian / currentMedian;
  const legacyProcessedMiB = (legacyBytesPerCall * iterations) / MIB;
  const currentProcessedMiB = (currentBytesPerCall * iterations) / MIB;
  const rebuiltMiB = (rebuiltBytesPerCall * iterations) / MIB;
  console.log(
    `${label}: n=${iterations}, processed ${legacyProcessedMiB.toFixed(1)} -> ${currentProcessedMiB.toFixed(1)} MiB, `
      + `rebuilt=${rebuiltMiB.toFixed(1)} MiB, `
      + `median ${legacyMedian.toFixed(3)}ms -> ${currentMedian.toFixed(3)}ms (${speedup.toFixed(2)}x), `
      + `p95 ${legacyP95.toFixed(3)}ms -> ${currentP95.toFixed(3)}ms`
  );
}

function reportPrefixStorage(length) {
  const typed = new Uint32Array(length);
  const estimatedLegacyBytes = length * 8;
  console.log(
    `KMP prefix storage (${length} code units): `
      + `legacy Array<number> element slots ~= ${estimatedLegacyBytes} B, `
      + `Uint32Array backing store=${typed.byteLength} B (50.0% less before array/object overhead)`
  );
}

function reportOptionalGcStorage(length) {
  if (typeof globalThis.gc !== "function") return;
  const copies = 8;
  globalThis.gc();
  const before = process.memoryUsage();
  const legacy = Array.from({ length: copies }, () => new Array(length).fill(0));
  globalThis.gc();
  const afterLegacy = process.memoryUsage();
  const typed = Array.from({ length: copies }, () => new Uint32Array(length));
  globalThis.gc();
  const afterTyped = process.memoryUsage();
  // Keep both allocations alive until after the measurements above so V8 cannot
  // collect one representation while its counterpart is being measured.
  assert.equal(legacy.length + typed.length, copies * 2);
  console.log(
    `GC allocation sample (${copies} x ${length}): `
      + `legacy heapUsed delta=${((afterLegacy.heapUsed - before.heapUsed) / KIB).toFixed(1)} KiB, `
      + `typed arrayBuffers delta=${((afterTyped.arrayBuffers - afterLegacy.arrayBuffers) / KIB).toFixed(1)} KiB`
  );
}

const fixtures = mapFixtures();
assertMapEquivalence(fixtures);
assertPresentationEquivalence();
console.log("equivalence: map selection fixtures and console presentation fixtures passed");

for (const size of REALISTIC_SIZES) {
  const fixture = rollingFixture(size);
  const presentation = presentationFixture(fixture.previous, fixture.previous).previous;
  const bytes = fixture.previous.length + fixture.next.length;
  const pairs = [Math.floor(fixture.shift / 2), fixture.previous.length - Math.floor(fixture.shift / 2)];
  benchmark(
    `map pair ${size / KIB} KiB`,
    () => legacyMapConsoleOffsetPair(pairs[0], pairs[1], fixture.previous, fixture.next),
    () => mapConsoleOffsetPair(pairs[0], pairs[1], fixture.previous, fixture.next),
    REALISTIC_ITERATIONS,
    bytes * 2,
    bytes
  );
  benchmark(
    `presentation ${size / KIB} KiB`,
    () => legacyReconcileConsoleLog(presentation, fixture.next),
    () => reconcileConsoleLog(presentation, fixture.next),
    REALISTIC_ITERATIONS,
    bytes,
    bytes,
    fixture.next.length
  );
  reportPrefixStorage(size);
  reportOptionalGcStorage(size);
}

for (const size of STRESS_SIZES) {
  const fixture = rollingFixture(size);
  const presentation = presentationFixture(fixture.previous, fixture.previous).previous;
  const bytes = fixture.previous.length + fixture.next.length;
  const pairs = [Math.floor(fixture.shift / 2), fixture.previous.length - Math.floor(fixture.shift / 2)];
  benchmark(
    `map pair ${size / KIB} KiB stress`,
    () => legacyMapConsoleOffsetPair(pairs[0], pairs[1], fixture.previous, fixture.next),
    () => mapConsoleOffsetPair(pairs[0], pairs[1], fixture.previous, fixture.next),
    STRESS_ITERATIONS,
    bytes * 2,
    bytes
  );
  benchmark(
    `presentation ${size / KIB} KiB stress`,
    () => legacyReconcileConsoleLog(presentation, fixture.next),
    () => reconcileConsoleLog(presentation, fixture.next),
    STRESS_ITERATIONS,
    bytes,
    bytes,
    fixture.next.length
  );
  reportPrefixStorage(size);
}
