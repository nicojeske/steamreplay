import { CircleAlert, CircleCheck, Clock, X } from "lucide-react";
import { formatBytes, formatTimecode } from "../../lib/format";
import { useExports, type Job } from "../../state/exports";
import { PathActions } from "../PathActions";
import { IconButton } from "../ui/Button";
import { ProgressBar, Spinner } from "../ui/misc";

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

/** One export of the queue. `showGame` for the app-wide list. */
export function JobRow({ job, showGame }: { job: Job; showGame?: boolean }) {
  const cancel = useExports((s) => s.cancel);
  const s = job.status;
  const range = `${formatTimecode(job.inS)} – ${formatTimecode(job.outS)}`;
  return (
    <li className={`job job-${s.kind}`}>
      <span className="job-icon" aria-hidden>
        {s.kind === "queued" && <Clock size={15} />}
        {s.kind === "running" && <Spinner size={15} />}
        {s.kind === "done" && <CircleCheck size={15} />}
        {(s.kind === "failed" || s.kind === "cancelled") && <CircleAlert size={15} />}
      </span>
      <div className="job-main">
        <div className="job-title">
          {s.kind === "done" ? (
            <span className="truncate" title={s.result.path}>
              {fileName(s.result.path)}
            </span>
          ) : (
            <span className="truncate">{showGame ? job.gameName : range}</span>
          )}
        </div>
        <div className="job-meta muted">
          {showGame && s.kind !== "done" && <span className="mono">{range} · </span>}
          {s.kind === "done" ? (
            <>
              {(s.result.actualOutS - s.result.actualInS).toFixed(job.mode.kind === "fast" ? 0 : 1)} s · {formatBytes(s.result.sizeBytes)} ·{" "}
              {s.result.encoder}
              {s.result.fellBack && (
                <span className="warn" title="The hardware encoder failed (e.g. the GPU's session limit while Steam records)">
                  {" "}
                  · software fallback
                </span>
              )}
            </>
          ) : (
            <>
              {job.label}
              {s.kind === "queued" && " · waiting"}
              {s.kind === "cancelled" && " · cancelled"}
            </>
          )}
        </div>
        {s.kind === "running" && <ProgressBar value={s.fraction} label="Export progress" />}
        {s.kind === "failed" && <div className="job-error danger">{s.message}</div>}
      </div>
      {(s.kind === "queued" || s.kind === "running") && (
        <IconButton label="Cancel export" size="sm" icon={<X size={15} />} onClick={() => cancel(job.jobId)} />
      )}
      {s.kind === "done" && <PathActions path={s.result.path} />}
    </li>
  );
}
