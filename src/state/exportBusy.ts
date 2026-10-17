import { create } from "zustand";

/** An export is queued or running. The updater waits for this before installing. */
export const useExportBusy = create<{ busy: boolean }>(() => ({ busy: false }));
