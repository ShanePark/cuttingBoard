import type { LaunchTask } from "./types";

export function launchTasksEquivalent(left: LaunchTask[], right: LaunchTask[]): boolean {
  if (left.length !== right.length) return false;
  const normalise = (task: LaunchTask): string => JSON.stringify([task.name, task.cwd, task.command, task.expected_port, task.container ?? null]);
  const leftDefinitions = left.map(normalise).sort();
  const rightDefinitions = right.map(normalise).sort();
  return leftDefinitions.every((definition, index) => definition === rightDefinitions[index]);
}
