import { CircleAlert, CircleCheck, Clock, RotateCcw, X } from "lucide-react";
import { formatBytes, formatTimecode } from "../../lib/format";
import { fileName, requestUpload, useExports, type Job } from "../../state/exports";
import { FireshareActions } from "../fireshare/FireshareActions";
import { PathActions } from "../PathActions";
import { IconButton } from "../ui/Button";
import { ProgressBar, Spinner } from "../ui/misc";

/** One job of the queue. `showGame` for the app-wide list. */
export function JobRow({ job, showGame }: { job: Job; showGame?: boolean }) {
  const cancel = useExports((s) => s.cancel);
  const s = job.status;
  const spec = job.export;
  const range = spec ? `${formatTimecode(spec.inS)} – ${formatTimecode(spec.outS)}` : null;
  const title = s.kind === "done" ? fileName(s.path) : showGame || !range ? job.gameName : range;
  const label = job.upload && spec ? `${job.label} + upload` : job.label;
  const retryPath = s.kind === "failed" && job.upload ? s.path : null;
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
          <span className="truncate" title={s.kind === "done" ? s.path : (job.path ?? undefined)}>
            {title}
          </span>
        </div>
        <div className="job-meta muted">
          {showGame && s.kind !== "done" && range && <span className="mono">{range} · </span>}
          {s.kind === "done" ? (
            <>
              {s.result && (
                <>
                  {(s.result.actualOutS - s.result.actualInS).toFixed(spec?.mode.kind === "fast" ? 0 : 1)} s · {formatBytes(s.result.sizeBytes)} ·{" "}
                  {s.result.encoder}
                  {s.result.fellBack && (
                    <span className="warn" title="The hardware encoder failed (e.g. the GPU's session limit while Steam records)">
                      {" "}
                      · software fallback
                    </span>
                  )}
                </>
              )}
              {s.upload && (
                <>
                  {s.result && " · "}
                  {s.upload.duplicate ? "already on Fireshare" : "on Fireshare"}
                </>
              )}
            </>
          ) : (
            <>
              {label}
              {s.kind === "queued" && " · waiting"}
              {s.kind === "running" && (s.phase === "export" ? " · exporting…" : " · uploading…")}
              {s.kind === "cancelled" && " · cancelled"}
            </>
          )}
        </div>
        {s.kind === "running" && (
          <ProgressBar value={s.fraction} label={s.phase === "export" ? "Export progress" : "Upload progress"} />
        )}
        {s.kind === "failed" && <div className="job-error danger">{s.message}</div>}
      </div>
      {(s.kind === "queued" || s.kind === "running") && (
        <IconButton label={s.kind === "running" && s.phase === "upload" ? "Cancel upload" : "Cancel export"} size="sm" icon={<X size={15} />} onClick={() => cancel(job.jobId)} />
      )}
      {retryPath && (
        <IconButton
          label="Retry upload"
          size="sm"
          icon={<RotateCcw size={15} />}
          onClick={() => requestUpload(retryPath, job.gameName, job.upload?.title ?? job.defaultTitle)}
        />
      )}
      {s.kind === "done" && (
        <>
          <FireshareActions path={s.path} url={s.upload?.url ?? null} gameName={job.gameName} defaultTitle={job.defaultTitle} />
          <PathActions path={s.path} />
        </>
      )}
    </li>
  );
}
