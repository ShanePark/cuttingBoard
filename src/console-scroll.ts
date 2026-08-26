export const CONSOLE_BOTTOM_EPSILON = 2;

export type ConsoleScrollMetrics = {
  scrollHeight: number;
  clientHeight: number;
  scrollTop: number;
};

export function consoleScrollDistanceFromBottom(metrics: ConsoleScrollMetrics): number {
  return Math.max(0, metrics.scrollHeight - metrics.clientHeight - metrics.scrollTop);
}

export function isConsoleAtBottom(metrics: ConsoleScrollMetrics, epsilon = CONSOLE_BOTTOM_EPSILON): boolean {
  return consoleScrollDistanceFromBottom(metrics) <= Math.max(0, epsilon);
}

export function clampConsoleScrollTop(scrollTop: number, scrollHeight: number, clientHeight: number): number {
  return Math.min(Math.max(0, scrollTop), Math.max(0, scrollHeight - clientHeight));
}

export function scrollTopForConsoleUpdate(
  metrics: Pick<ConsoleScrollMetrics, "scrollHeight" | "clientHeight">,
  previousScrollTop: number,
  follow: boolean
): number {
  return follow
    ? Math.max(0, metrics.scrollHeight - metrics.clientHeight)
    : clampConsoleScrollTop(previousScrollTop, metrics.scrollHeight, metrics.clientHeight);
}
