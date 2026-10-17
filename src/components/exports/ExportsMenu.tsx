import { useEffect, useState } from "react";
import { Popover } from "radix-ui";
import { Download, Trash2 } from "lucide-react";
import { commands, type RecentEntry } from "../../bindings";
import { formatBytes, formatRelativeDate, formatTimecode } from "../../lib/format";
import { unwrap } from "../../lib/ipc";
import { isActive, useExports } from "../../state/exports";
import { defaultTitle } from "../../state/fireshare";
import { toast } from "../../state/toasts";
import { FireshareActions } from "../fireshare/FireshareActions";
import { PathActions } from "../PathActions";
import { Button, IconButton } from "../ui/Button";
import { JobRow } from "./JobRow";

/** Top-bar button with the export queue of the whole app and the recent exports. */
export function ExportsMenu() {
  const jobs = useExports((s) => s.jobs);
  const clearFinished = useExports((s) => s.clearFinished);
  const active = jobs.filter(isActive);
  const running = jobs.find((j) => j.status.kind === "running");
  const fraction = running?.status.kind === "running" ? running.status.fraction : null;
  const [open, setOpen] = useState(false);

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>
        <button type="button" className={`btn btn-ghost btn-md exports-button ${active.length ? "is-busy" : ""}`} aria-label="Exports">
          <span className="exports-ring" style={{ "--p": fraction ?? 0 } as React.CSSProperties}>
            <Download size={16} />
          </span>
          <span>Exports</span>
          {active.length > 0 && <span className="count-badge">{active.length}</span>}
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content className="popover exports-popover" align="end" sideOffset={6} collisionPadding={8}>
          <header className="popover-header">
            <h3>Exports</h3>
            <span className="spacer" />
            {jobs.length > active.length && (
              <Button size="sm" variant="ghost" icon={<Trash2 size={14} />} onClick={clearFinished}>
                Clear finished
              </Button>
            )}
          </header>
          {jobs.length === 0 ? (
            <p className="muted popover-empty">No exports in this session yet.</p>
          ) : (
            <ul className="job-list">
              {[...jobs].reverse().map((j) => (
                <JobRow key={j.jobId} job={j} showGame />
              ))}
            </ul>
          )}
          {open && <RecentExports />}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

/** The last exports of any session, kept across restarts. Missing files are greyed out. */
function RecentExports() {
  const seq = useExports((s) => s.recentSeq);
  const [entries, setEntries] = useState<RecentEntry[] | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    let active = true;
    unwrap(commands.getRecentExports())
      .then((e) => active && setEntries(e))
      .catch((e: unknown) => console.warn("recent exports", e));
    return () => {
      active = false;
    };
  }, [seq, reload]);

  if (!entries?.length) return null;

  const remove = async (path: string) => {
    try {
      await unwrap(commands.removeRecentExport(path));
      setReload((n) => n + 1);
    } catch (e) {
      toast.error(e);
    }
  };

  return (
    <section className="recent-exports">
      <h4>Recent</h4>
      <ul className="job-list">
        {entries.map(({ entry: e, exists }) => (
          <li
            key={e.path}
            className={`job ${exists ? "" : "job-missing"}`}
            title={exists ? e.path : "The file is gone or its drive is not connected"}
          >
            <div className="job-main">
              <div className="job-title truncate">{e.path.split(/[\\/]/).pop()}</div>
              <div className="job-meta muted">
                {e.gameName} · <span className="mono">{formatTimecode(e.inS)}</span> · {e.mode === "fast" ? "Fast" : "Precise"} ·{" "}
                {formatBytes(e.sizeBytes)} · {formatRelativeDate(e.finishedAt)}
              </div>
            </div>
            {exists ? (
              <>
                <FireshareActions
                  path={e.path}
                  url={e.fireshareUrl ?? null}
                  gameName={e.gameName}
                  defaultTitle={defaultTitle(e.gameName, new Date(e.finishedAt))}
                />
                <PathActions path={e.path} />
              </>
            ) : (
              <IconButton label="Remove from the list" size="sm" icon={<Trash2 size={15} />} onClick={() => void remove(e.path)} />
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
