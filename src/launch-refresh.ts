import type { LaunchProfile, ManagedTaskSnapshot } from "./types";

export type LaunchRefreshContext = {
  api: {
    profiles: () => Promise<LaunchProfile[]>;
    taskSnapshots: () => Promise<ManagedTaskSnapshot[]>;
  };
  getActiveTab: () => string;
  getSelectedTaskKey: () => string | null;
  setProfiles: (profiles: LaunchProfile[]) => void;
  setSnapshots: (snapshots: ManagedTaskSnapshot[]) => void;
  renderHeaderCounts: () => void;
  renderLaunch: (force?: boolean) => void;
  messageOf: (error: unknown) => string;
  toast: (message: string, error?: boolean) => void;
};

export function createLaunchRefresh(context: LaunchRefreshContext) {
  let snapshotGeneration = 0;
  let refreshRequests = 0;
  let snapshotPollBusy = false;

  const refreshLaunch = async (force = false): Promise<void> => {
    const generation = ++snapshotGeneration;
    refreshRequests += 1;
    try {
      const [nextProfiles, nextSnapshots] = await Promise.all([context.api.profiles(), context.api.taskSnapshots()]);
      if (generation !== snapshotGeneration) return;
      context.setProfiles(nextProfiles);
      context.setSnapshots(nextSnapshots);
      context.renderHeaderCounts();
      if (context.getActiveTab() === "launch") context.renderLaunch(force);
    } catch (error) {
      context.toast(context.messageOf(error), true);
    } finally {
      refreshRequests -= 1;
    }
  };

  const refreshSelectedLaunchLogs = async (): Promise<void> => {
    if (context.getActiveTab() !== "launch" || !context.getSelectedTaskKey() || refreshRequests > 0 || snapshotPollBusy) return;
    snapshotPollBusy = true;
    const generation = ++snapshotGeneration;
    try {
      const nextSnapshots = await context.api.taskSnapshots();
      if (generation !== snapshotGeneration || refreshRequests > 0) return;
      context.setSnapshots(nextSnapshots);
      if (context.getActiveTab() === "launch") context.renderLaunch(false);
    } catch {
      // The global scan will surface workspace errors; a transient log poll should not interrupt it.
    } finally {
      snapshotPollBusy = false;
    }
  };

  return { refreshLaunch, refreshSelectedLaunchLogs };
}
