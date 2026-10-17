import { create } from "zustand";
import { Channel } from "@tauri-apps/api/core";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { commands, type ExportMode, type ExportResult, type Progress, type UploadMeta, type UploadResult } from "../bindings";
import { IpcError, errorMessage, unwrap } from "../lib/ipc";
import { isConfigured, useFireshare } from "./fireshare";
import { toast, type ToastAction } from "./toasts";

/** The selection of a recording to export. */
export type ExportSpec = {
  sessionId: string;
  inS: number;
  outS: number;
  /** `null`: the default name, chosen when the job starts (so queued jobs do not collide). */
  output: string | null;
  mode: ExportMode;
};

export type JobStatus =
  | { kind: "queued" }
  | { kind: "running"; phase: "export" | "upload"; fraction: number }
  /** `result` is `null` for an upload-only job. */
  | { kind: "done"; path: string; result: ExportResult | null; upload: UploadResult | null }
  /** `path`: the export worked but the upload failed, so it can be retried. */
  | { kind: "failed"; message: string; path: string | null }
  | { kind: "cancelled" };

export type Job = {
  jobId: string;
  gameName: string;
  /** Export this selection; `null` for an upload-only job. */
  export: ExportSpec | null;
  /** Upload this file (upload-only jobs). */
  path: string | null;
  /** Upload to Fireshare after the export. */
  upload: UploadMeta | null;
  /** The title the upload dialog suggests. */
  defaultTitle: string;
  /** "Fast", "Precise · <preset>" or "Upload". */
  label: string;
  status: JobStatus;
};

type ExportsState = {
  jobs: Job[];
  /** Bumped after each finished export or upload, so the recent list reloads. */
  recentSeq: number;
  enqueue: (job: Omit<Job, "jobId" | "status">) => void;
  cancel: (jobId: string) => void;
  clearFinished: () => void;
};

export const isActive = (j: Job) => j.status.kind === "queued" || j.status.kind === "running";

/** An export or upload is queued or running. The updater waits for this before it installs. */
export const selectBusy = (s: ExportsState) => s.jobs.some(isActive);

export const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** "Copy link" and "Open" for a share link. */
export function linkActions(url: string): ToastAction[] {
  return [
    { label: "Copy link", run: () => void copyLink(url) },
    { label: "Open", run: () => void openUrl(url).catch(toast.error) },
  ];
}

export async function copyLink(url: string) {
  try {
    await writeText(url);
    toast.info("Link copied");
  } catch (e) {
    toast.error(e);
  }
}

/**
 * Opens the upload dialog for a file; the upload then runs as a job of the queue. Without a
 * Fireshare setup the dialog shows the way to the settings.
 */
export function requestUpload(path: string, gameName: string, defaultTitle: string) {
  useFireshare.getState().openUpload({
    gameName,
    defaultTitle,
    submitLabel: "Upload",
    onSubmit: (upload) =>
      useExports.getState().enqueue({ gameName, export: null, path, upload, defaultTitle, label: "Upload" }),
  });
}

/**
 * The queue of the whole app. Jobs run one after another and keep running when another
 * recording is opened. A job exports, uploads, or exports and then uploads.
 */
export const useExports = create<ExportsState>((set, get) => {
  const update = (jobId: string, status: JobStatus) =>
    set((s) => ({ jobs: s.jobs.map((j) => (j.jobId === jobId ? { ...j, status } : j)) }));

  const runExport = (job: Job, spec: ExportSpec) => {
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => update(job.jobId, { kind: "running", phase: "export", fraction: p.fraction });
    const { sessionId: id, inS, outS, output, mode } = spec;
    return unwrap(commands.exportClip({ id, inS, outS, output, mode }, job.jobId, progress));
  };

  const runUpload = (job: Job, path: string, meta: UploadMeta) => {
    update(job.jobId, { kind: "running", phase: "upload", fraction: 0 });
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => update(job.jobId, { kind: "running", phase: "upload", fraction: p.fraction });
    return unwrap(commands.fireshareUpload(path, meta, job.jobId, progress));
  };

  const run = async (job: Job) => {
    let result: ExportResult | null = null;
    let path = job.path;
    try {
      if (job.export) {
        result = await runExport(job, job.export);
        path = result.path;
        set((s) => ({ recentSeq: s.recentSeq + 1 }));
      }
      if (!path) throw new Error("nothing to upload");
      if (!job.upload) {
        update(job.jobId, { kind: "done", path, result, upload: null });
        const exported = path;
        const actions: ToastAction[] = [{ label: "Show in folder", run: () => void revealItemInDir(exported).catch(toast.error) }];
        if (isConfigured(useFireshare.getState().config)) {
          actions.push({ label: "Upload to Fireshare", run: () => requestUpload(exported, job.gameName, job.defaultTitle) });
        }
        toast.success(`Exported ${fileName(path)}`, actions);
        return;
      }
      const upload = await runUpload(job, path, job.upload);
      update(job.jobId, { kind: "done", path, result, upload });
      set((s) => ({ recentSeq: s.recentSeq + 1 }));
      toast.success(
        upload.duplicate
          ? `${fileName(path)} is already on Fireshare`
          : `Uploaded ${fileName(path)}. Fireshare shows it once it has processed it (seconds to a minute).`,
        linkActions(upload.url),
      );
    } catch (e) {
      if (e instanceof IpcError && e.kind === "cancelled") {
        update(job.jobId, { kind: "cancelled" });
      } else {
        update(job.jobId, { kind: "failed", message: errorMessage(e), path });
        toast.error(e);
      }
    }
  };

  const pump = () => {
    const { jobs } = get();
    if (jobs.some((j) => j.status.kind === "running")) return;
    const next = jobs.find((j) => j.status.kind === "queued");
    if (!next) return;
    update(next.jobId, { kind: "running", phase: next.export ? "export" : "upload", fraction: 0 });
    void run(next).finally(pump);
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
