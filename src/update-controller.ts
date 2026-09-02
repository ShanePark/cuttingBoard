import type { UpdateCheckResult } from "./types";

export const UPDATE_CHECK_INTERVAL_MS = 30_000;

export interface UpdateControllerApi {
  checkForUpdate: () => Promise<UpdateCheckResult>;
  updateAndRestart: () => Promise<void>;
}

export interface UpdateControllerUi {
  setUpdateAvailable: (available: boolean) => void;
  setUpdateBusy: (busy: boolean) => void;
  showUpdateStarted: () => void;
  showError: (message: string) => void;
}

export interface UpdateController {
  checkForUpdate: () => Promise<void>;
  updateAndRestart: () => Promise<void>;
}

export function createUpdateController(api: UpdateControllerApi, ui: UpdateControllerUi): UpdateController {
  let checkBusy = false;
  let updateBusy = false;
  let updateAvailable = false;
  let updateAttempt = 0;

  const checkForUpdate = async (): Promise<void> => {
    // A slow git check must not stack up interval callbacks or overwrite an update in progress.
    if (checkBusy || updateBusy) return;
    checkBusy = true;
    const attemptAtStart = updateAttempt;
    try {
      const result = await api.checkForUpdate();
      if (attemptAtStart !== updateAttempt || updateBusy) return;
      updateAvailable = result.available;
      ui.setUpdateAvailable(updateAvailable);
    } catch {
      if (attemptAtStart !== updateAttempt || updateBusy) return;
      updateAvailable = false;
      ui.setUpdateAvailable(false);
    } finally {
      checkBusy = false;
    }
  };

  const updateAndRestart = async (): Promise<void> => {
    if (updateBusy || !updateAvailable) return;
    updateBusy = true;
    updateAttempt += 1;
    ui.setUpdateBusy(true);
    ui.showUpdateStarted();
    try {
      await api.updateAndRestart();
      // The native command hands control to the installer and normally terminates this process.
      // Keep the control busy if the command resolves so a delayed restart cannot trigger a second build.
    } catch (error) {
      updateBusy = false;
      updateAvailable = true;
      ui.setUpdateAvailable(true);
      ui.setUpdateBusy(false);
      ui.showError(messageOf(error));
    }
  };

  return { checkForUpdate, updateAndRestart };
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
