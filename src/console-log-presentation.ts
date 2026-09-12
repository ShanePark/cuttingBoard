export type ConsoleLogPresentation = {
  source: string;
  output: string;
  lineBreakOffsets: readonly number[];
};

const MINIMUM_ROLLING_TAIL_OVERLAP = 64;

function renderConsoleLog(source: string, lineBreakOffsets: readonly number[]): string {
  let output = "";
  let sourceOffset = 0;
  for (const lineBreakOffset of lineBreakOffsets) {
    output += source.slice(sourceOffset, lineBreakOffset) + "\n";
    sourceOffset = lineBreakOffset;
  }
  return output + source.slice(sourceOffset);
}

function presentation(source: string, lineBreakOffsets: readonly number[]): ConsoleLogPresentation {
  return {
    source,
    output: renderConsoleLog(source, lineBreakOffsets),
    lineBreakOffsets
  };
}

function normalizeLineEndings(value: string): string {
  return value.replace(/\r\n?/g, "\n");
}

function suffixPrefixOverlap(previous: string, next: string): number {
  if (!previous || !next) return 0;
  const prefixLengths = new Uint32Array(next.length);
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

export function reconcileConsoleLog(
  presentation: ConsoleLogPresentation | undefined,
  source: string
): ConsoleLogPresentation {
  const normalizedSource = normalizeLineEndings(source);
  if (!presentation) return presentationForSource(normalizedSource);
  if (normalizedSource === presentation.source) return presentation;
  if (normalizedSource.startsWith(presentation.source)) {
    return presentationForSource(normalizedSource, presentation.lineBreakOffsets);
  }
  const overlap = suffixPrefixOverlap(presentation.source, normalizedSource);
  if (overlap >= MINIMUM_ROLLING_TAIL_OVERLAP) {
    const droppedLength = presentation.source.length - overlap;
    const remainingLineBreaks = presentation.lineBreakOffsets
      .filter((offset) => offset >= droppedLength)
      .map((offset) => offset - droppedLength);
    return presentationForSource(normalizedSource, remainingLineBreaks);
  }
  return presentationForSource(normalizedSource);
}

export function appendConsoleLineBreak(
  presentation: ConsoleLogPresentation | undefined,
  source: string
): ConsoleLogPresentation {
  const current = reconcileConsoleLog(presentation, source);
  return {
    source: current.source,
    output: `${current.output}\n`,
    lineBreakOffsets: [...current.lineBreakOffsets, current.source.length]
  };
}

function presentationForSource(source: string, lineBreakOffsets: readonly number[] = []): ConsoleLogPresentation {
  return presentation(source, lineBreakOffsets);
}
