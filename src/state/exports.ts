import { create } from "zustand";
import { Channel } from "@tauri-apps/api/core";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands, type ExportMode, type ExportResult, type Progress } from "../bindings";
import { IpcError, errorMessage, unwrap } from "../lib/ipc";
import { toast } from "./toasts";

export type JobStatus =
  | { kind: "queued" }
  | { kind: "running"; fraction: number }
  | { kind: "done"; result: ExportResult }
  | { kind: "failed"; message: string }
  | { kind: "cancelled" };

export type Job = {
  jobId: string;
  sessionId: string;
  gameName: string;
  inS: number;
  outS: number;
  /** `null`: the default name, chosen when the job starts (so queued jobs do not collide). */
  output: string | null;
  mode: ExportMode;
  /** "Fast" or "Precise · <preset>". */
  label: string;
  status: JobStatus;
};

type ExportsState = {
  jobs: Job[];
  /** Bumped after each finished export, so the recent list reloads. */
  recentSeq: number;
  enqueue: (job: Omit<Job, "jobId" | "status">) => void;
  cancel: (jobId: string) => void;
  clearFinished: () => void;
};

export const isActive = (j: Job) => j.status.kind === "queued" || j.status.kind === "running";

/** An export is queued or running. The updater waits for this before it installs. */
export const selectBusy = (s: ExportsState) => s.jobs.some(isActive);

/**
 * The export queue of the whole app. Exports run one after another and keep running when
 * another recording is opened.
 */
export const useExports = create<ExportsState>((set, get) => {
  const update = (jobId: string, status: JobStatus) =>
    set((s) => ({ jobs: s.jobs.map((j) => (j.jobId === jobId ? { ...j, status } : j)) }));

  const pump = () => {
    const { jobs } = get();
    if (jobs.some((j) => j.status.kind === "running")) return;
    const next = jobs.find((j) => j.status.kind === "queued");
    if (!next) return;
    update(next.jobId, { kind: "running", fraction: 0 });
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => update(next.jobId, { kind: "running", fraction: p.fraction });
    const { sessionId: id, inS, outS, output, mode } = next;
    unwrap(commands.exportClip({ id, inS, outS, output, mode }, next.jobId, progress))
      .then((result) => {
        update(next.jobId, { kind: "done", result });
        set((s) => ({ recentSeq: s.recentSeq + 1 }));
        const name = result.path.split(/[\\/]/).pop() ?? result.path;
        toast.success(`Exported ${name}`, [
          { label: "Show in folder", run: () => void revealItemInDir(result.path).catch(toast.error) },
        ]);
      })
      .catch((e: unknown) => {
        if (e instanceof IpcError && e.kind === "cancelled") {
          update(next.jobId, { kind: "cancelled" });
        } else {
          update(next.jobId, { kind: "failed", message: errorMessage(e) });
          toast.error(e);
        }
      })
      .finally(pump);
  };

  return {
    jobs: [],
    recentSeq: 0,
    enqueue: (job) => {
      set((s) => ({ jobs: [...s.jobs, { ...job, jobId: crypto.randomUUID(), status: { kind: "queued" } }] }));
      pump();
    },
    cancel: (jobId) => {
      const job = get().jobs.find((j) => j.jobId === jobId);
      if (job?.status.kind === "queued") update(jobId, { kind: "cancelled" });
      else if (job?.status.kind === "running") void commands.cancelJob(jobId);
    },
    clearFinished: () => set((s) => ({ jobs: s.jobs.filter(isActive) })),
  };
});
