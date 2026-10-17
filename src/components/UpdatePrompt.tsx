import { useState } from "react";
import { Sparkles } from "lucide-react";
import { errorMessage } from "../lib/ipc";
import { installUpdate, waitForExports, type Update } from "../lib/updater";
import { Button } from "./ui/Button";
import { ProgressBar } from "./ui/misc";

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
    <div className="toast toast-update" role="status">
      <Sparkles size={18} className="toast-icon" aria-hidden />
      <div className="toast-body">
        <div className="toast-text">
          <strong>SteamReplay {update.version}</strong> is available.
        </div>
        {phase.kind === "waiting" && <div className="muted">Waiting for exports to finish…</div>}
        {phase.kind === "installing" && <ProgressBar value={phase.fraction} label="Downloading update" />}
        {phase.kind === "failed" && <div className="danger">Update failed: {phase.message}</div>}
        {(phase.kind === "ask" || phase.kind === "failed") && (
          <div className="toast-actions">
            <Button variant="primary" size="sm" onClick={() => void install()}>
              {phase.kind === "failed" ? "Try again" : "Install and restart"}
            </Button>
            <Button variant="ghost" size="sm" onClick={onLater}>
              Later
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
