import { useState } from "react";
import { errorMessage } from "../lib/ipc";
import { installUpdate, waitForExports, type Update } from "../lib/updater";

type Phase = { kind: "ask" } | { kind: "waiting" } | { kind: "installing"; fraction: number | null } | { kind: "failed"; message: string };

/** Persistent toast offering a new release. "Later" hides it until the next start. */
export function UpdatePrompt({ update, onLater }: { update: Update; onLater: () => void }) {
  const [phase, setPhase] = useState<Phase>({ kind: "ask" });

  const install = async () => {
    setPhase({ kind: "waiting" });
    await waitForExports();
    setPhase({ kind: "installing", fraction: null });
    try {
      await installUpdate(update, (fraction) => setPhase({ kind: "installing", fraction }));
    } catch (e) {
      setPhase({ kind: "failed", message: errorMessage(e) });
    }
  };

  return (
    <div className="toast update" role="status">
      <div>SteamReplay {update.version} is available.</div>
      {phase.kind === "waiting" && <div className="muted">Waiting for exports to finish…</div>}
      {phase.kind === "installing" && (
        <div className="toast-progress" aria-label="Downloading update">
          <div
            className={phase.fraction === null ? "indeterminate" : undefined}
            style={phase.fraction === null ? undefined : { width: `${phase.fraction * 100}%` }}
          />
        </div>
      )}
      {phase.kind === "failed" && <div className="warn">Update failed: {phase.message}</div>}
      {(phase.kind === "ask" || phase.kind === "failed") && (
        <div className="buttons">
          <button type="button" className="primary" onClick={() => void install()}>
            {phase.kind === "failed" ? "Try again" : "Install and restart"}
          </button>
          <button type="button" onClick={onLater}>
            Later
          </button>
        </div>
      )}
    </div>
  );
}
