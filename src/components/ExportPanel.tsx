import { useEffect, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { commands, type ExportResult, type Progress, type Snap } from "../bindings";
import { IpcError, errorMessage, unwrap } from "../lib/ipc";
import { formatBytes, formatTimecode } from "../lib/format";
import { isTyping } from "../lib/keys";
import { useView } from "../state/view";

type Props = { id: string; onError: (e: unknown) => void };

type JobStatus =
  | { kind: "queued" }
  | { kind: "running"; fraction: number }
  | { kind: "done"; result: ExportResult }
  | { kind: "failed"; message: string }
  | { kind: "cancelled" };

type Job = {
  jobId: string;
  inS: number;
  outS: number;
  /** `null`: the default name, chosen when the job starts (so queued jobs do not collide). */
  output: string | null;
  status: JobStatus;
};

const SNAP_DEBOUNCE_MS = 150;

/** `I`/`O` set in/out at the playhead, `Esc` clears the selection, `Ctrl+L` toggles loop. */
function useSelectionKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.altKey || e.metaKey || isTyping(e.target) || document.querySelector("dialog[open]")) return;
      const view = useView.getState();
      const key = e.key.toLowerCase();
      if (e.ctrlKey) {
        if (key !== "l" || e.shiftKey) return;
        view.setLoop(!view.loop);
      } else if (e.shiftKey) {
        return;
      } else if (key === "i") {
        view.setIn(view.playheadS);
      } else if (key === "o") {
        view.setOut(view.playheadS);
      } else if (e.key === "Escape" && view.inS !== null) {
        view.clearSelection();
      } else {
        return;
      }
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

/** Selection summary, output path, Export button and the list of exports of this session. */
export function ExportPanel({ id, onError }: Props) {
  useSelectionKeys();
  const inS = useView((s) => s.inS);
  const outS = useView((s) => s.outS);
  const loop = useView((s) => s.loop);
  const [snap, setSnap] = useState<Snap | null>(null);
  const [path, setPath] = useState("");
  /** The user typed or browsed a path; stop replacing it with the default. */
  const [edited, setEdited] = useState(false);
  const [jobs, setJobs] = useState<Job[]>([]);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onErrorRef.current = onError;
  });

  // Snapped cut, size estimate and default file name follow the selection.
  useEffect(() => {
    if (inS === null || outS === null) return;
    let active = true;
    const timer = window.setTimeout(() => {
      unwrap(commands.exportSnap(id, inS, outS))
        .then((s) => active && setSnap(s))
        .catch(() => active && setSnap(null));
      if (!edited) {
        unwrap(commands.defaultExportPath(id, inS))
          .then((p) => active && setPath(p))
          .catch(() => {});
      }
    }, SNAP_DEBOUNCE_MS);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [id, inS, outS, edited]);

  // Runs queued exports one after another.
  const running = jobs.some((j) => j.status.kind === "running");
  const next = running ? undefined : jobs.find((j) => j.status.kind === "queued");
  const started = useRef(new Set<string>());
  useEffect(() => {
    if (!next || started.current.has(next.jobId)) return;
    started.current.add(next.jobId);
    const update = (status: JobStatus) =>
      setJobs((js) => js.map((j) => (j.jobId === next.jobId ? { ...j, status } : j)));
    update({ kind: "running", fraction: 0 });
    const progress = new Channel<Progress>();
    progress.onmessage = (p) => update({ kind: "running", fraction: p.fraction });
    unwrap(commands.exportClip({ id, inS: next.inS, outS: next.outS, output: next.output }, next.jobId, progress))
      .then((result) => update({ kind: "done", result }))
      .catch((e: unknown) => {
        if (e instanceof IpcError && e.kind === "cancelled") {
          update({ kind: "cancelled" });
        } else {
          update({ kind: "failed", message: errorMessage(e) });
          onErrorRef.current(e);
        }
      });
  }, [id, next]);

  // Leaving the session cancels its exports.
  const jobsRef = useRef(jobs);
  useEffect(() => {
    jobsRef.current = jobs;
  });
  useEffect(
    () => () => {
      for (const j of jobsRef.current) if (j.status.kind === "running") void commands.cancelJob(j.jobId);
    },
    [],
  );

  const hasSelection = inS !== null && outS !== null;

  const browse = async () => {
    try {
      const chosen = await save({ defaultPath: path || undefined, filters: [{ name: "MP4 video", extensions: ["mp4"] }] });
      if (chosen) {
        setPath(chosen);
        setEdited(true);
      }
    } catch (e) {
      onError(e);
    }
  };

  const startExport = () => {
    if (inS === null || outS === null) return;
    const output = edited && path.trim() ? path.trim() : null;
    setJobs((js) => [...js, { jobId: crypto.randomUUID(), inS, outS, output, status: { kind: "queued" } }]);
    // The next export gets a fresh default name again.
    setEdited(false);
  };

  const playSelection = () => {
    if (inS === null) return;
    const view = useView.getState();
    view.setLoop(true);
    view.seek(inS);
    view.requestPlay();
  };

  return (
    <div className="export-panel">
      <div className="export-row">
        {hasSelection ? (
          <span className="mono">
            {formatTimecode(inS, 1)} – {formatTimecode(outS, 1)} · {(outS - inS).toFixed(1)} s
            {snap && (
              <span className="muted">
                {" "}
                → cut {formatTimecode(snap.actualInS)} – {formatTimecode(snap.actualOutS)} ·{" "}
                {(snap.actualOutS - snap.actualInS).toFixed(0)} s · ~{formatBytes(snap.estBytes)}
              </span>
            )}
          </span>
        ) : (
          <span className="muted">No selection. I / O at the playhead, Shift+drag on the timeline, or “Select” on a loud moment.</span>
        )}
        <span className="spacer" />
        <label className="loop-toggle" title="Loop playback inside the selection (Ctrl+L)">
          <input type="checkbox" checked={loop} onChange={(e) => useView.getState().setLoop(e.target.checked)} /> Loop
        </label>
        <button type="button" onClick={playSelection} disabled={!hasSelection}>
          Play selection
        </button>
        <button type="button" onClick={() => useView.getState().clearSelection()} disabled={!hasSelection} title="Esc">
          Clear
        </button>
      </div>
      <div className="export-row">
        <input
          className="export-path mono"
          value={path}
          placeholder="Output file"
          spellCheck={false}
          onChange={(e) => {
            setPath(e.target.value);
            setEdited(true);
          }}
        />
        <button type="button" onClick={() => void browse()}>
          Browse…
        </button>
        <button
          type="button"
          className="primary"
          onClick={startExport}
          disabled={!hasSelection}
          title="Original quality (HEVC + AAC, no re-encode), cut at the 3 s chunk boundaries. Some apps (Discord, WhatsApp, Films & TV without the HEVC extension) cannot play HEVC; use VLC."
        >
          Export
        </button>
      </div>
      {jobs.length > 0 && (
        <ul className="export-jobs">
          {[...jobs].reverse().map((j) => (
            <JobRow key={j.jobId} job={j} onError={onError} />
          ))}
        </ul>
      )}
    </div>
  );
}

function JobRow({ job, onError }: { job: Job; onError: (e: unknown) => void }) {
  const s = job.status;
  const range = `${formatTimecode(job.inS)} – ${formatTimecode(job.outS)}`;
  return (
    <li>
      <span className="mono">{range}</span>
      {s.kind === "queued" && <span className="muted">Waiting…</span>}
      {s.kind === "running" && (
        <>
          <progress max={1} value={s.fraction} />
          <button type="button" onClick={() => void commands.cancelJob(job.jobId)}>
            Cancel
          </button>
        </>
      )}
      {s.kind === "done" && (
        <>
          <span className="mono export-file" title={s.result.path}>
            {s.result.path}
          </span>
          <span className="muted">
            {(s.result.actualOutS - s.result.actualInS).toFixed(0)} s · {formatBytes(s.result.sizeBytes)}
          </span>
          <button type="button" onClick={() => revealItemInDir(s.result.path).catch(onError)}>
            Show in folder
          </button>
          <button type="button" onClick={() => writeText(s.result.path).catch(onError)}>
            Copy path
          </button>
        </>
      )}
      {s.kind === "failed" && <span className="warn">Failed: {s.message}</span>}
      {s.kind === "cancelled" && <span className="muted">Cancelled</span>}
    </li>
  );
}
