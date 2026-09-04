import assert from "node:assert/strict";
import test from "node:test";
import {
  appendConsoleLineBreak,
  reconcileConsoleLog
} from "../src/console-log-presentation.ts";

test("adds one visual line break without changing the source log", () => {
  const presentation = appendConsoleLineBreak(undefined, "first line\n");

  assert.deepEqual(presentation, {
    source: "first line\n",
    output: "first line\n\n",
    lineBreakOffsets: [11]
  });
});

test("keeps visual line breaks ahead of newly appended log output", () => {
  const separated = appendConsoleLineBreak(
    appendConsoleLineBreak(undefined, "first line\n"),
    "first line\n"
  );

  assert.deepEqual(reconcileConsoleLog(separated, "first line\nnext line\n"), {
    source: "first line\nnext line\n",
    output: "first line\n\n\nnext line\n",
    lineBreakOffsets: [11, 11]
  });
});

test("drops visual line breaks when the source log is replaced", () => {
  const separated = appendConsoleLineBreak(undefined, "old output\n");

  assert.deepEqual(reconcileConsoleLog(separated, "new output\n"), {
    source: "new output\n",
    output: "new output\n",
    lineBreakOffsets: []
  });
});

test("keeps visual line breaks when a full log tail rolls forward", () => {
  const dropped = "discarded prefix\n".repeat(8);
  const overlap = "stable log line\n".repeat(8);
  const beforeRoll = appendConsoleLineBreak(undefined, dropped + overlap);
  const withNewOutput = reconcileConsoleLog(beforeRoll, `${dropped}${overlap}after separator\n`);

  assert.deepEqual(
    reconcileConsoleLog(withNewOutput, `${overlap}after separator\nlatest line\n`),
    {
      source: `${overlap}after separator\nlatest line\n`,
      output: `${overlap}\nafter separator\nlatest line\n`,
      lineBreakOffsets: [overlap.length]
    }
  );
});
