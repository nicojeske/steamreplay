import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { commands } from "../bindings";
import { useExportBusy } from "../state/exportBusy";

export type { Update };

/** The NSIS install and the AppImage can update themselves; `.deb` and `tauri dev` cannot. */
export const updaterSupported = (): Promise<boolean> => commands.updaterSupported();

/** The newer release, or null when up to date or when this build cannot update. Throws when offline. */
export async function checkForUpdate(): Promise<Update | null> {
  if (!(await updaterSupported())) return null;
  return check();
}

/** Resolves once no export is queued or running. */
export function waitForExports(): Promise<void> {
  if (!useExportBusy.getState().busy) return Promise.resolve();
  return new Promise((resolve) => {
    const unsubscribe = useExportBusy.subscribe((s) => {
      if (s.busy) return;
      unsubscribe();
      resolve();
    });
  });
}

/** Downloads and installs `update`, then restarts the app. `onProgress` gets 0–1, or null when the size is unknown. */
export async function installUpdate(update: Update, onProgress: (fraction: number | null) => void): Promise<void> {
  let total = 0;
  let received = 0;
  await update.downloadAndInstall((e) => {
    if (e.event === "Started") {
      total = e.data.contentLength ?? 0;
      onProgress(total > 0 ? 0 : null);
    } else if (e.event === "Progress") {
      received += e.data.chunkLength;
      onProgress(total > 0 ? Math.min(1, received / total) : null);
    }
  });
  // On Windows the passive installer has already closed the app; this covers the AppImage.
  await relaunch();
}
