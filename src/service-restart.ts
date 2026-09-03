import { matchedServiceForTask } from "./presentation-services";
import { isSpringService } from "./spring-prepare";
import type { LaunchProfile, LaunchTask, ManagedTaskSnapshot, ServiceSnapshot } from "./types";

export type SavedSpringTask = { profile: LaunchProfile; task: LaunchTask };

/**
 * Find the saved task represented by a service. Matching uses the same port and path rule as the
 * launch board, preventing an unrelated service from receiving the task restart path merely
 * because it belongs to the same project.
 *
 * A task without metadata is accepted for a live Spring service for compatibility with profiles
 * saved before project-aware preparation was introduced; the native runner can infer its build
 * tool from the project on restart.
 */
export function savedSpringTaskForService(
  service: ServiceSnapshot,
  profiles: readonly LaunchProfile[]
): SavedSpringTask | null {
  for (const profile of profiles) {
    for (const task of profile.tasks) {
      if (task.prepare?.kind !== "spring_boot" && !isSpringService(service)) continue;
      if (matchedServiceForTask(profile, task, [service])?.id === service.id) return { profile, task };
    }
  }
  return null;
}

export type ServiceRestartApi = {
  restartTask?: (profileId: string, taskName: string) => Promise<ManagedTaskSnapshot>;
  restartService: (serviceId: string) => Promise<void>;
};

/** Invoke the prepared task restart when a saved task matches, preserving raw restart fallback. */
export async function restartServiceWithPreparation(
  service: ServiceSnapshot,
  profiles: readonly LaunchProfile[],
  api: ServiceRestartApi
): Promise<void | ManagedTaskSnapshot> {
  const springTask = savedSpringTaskForService(service, profiles);
  if (springTask && api.restartTask) return api.restartTask(springTask.profile.id, springTask.task.name);
  return api.restartService(service.id);
}
