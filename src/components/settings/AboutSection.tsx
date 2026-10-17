import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Check, X } from "lucide-react";
import { commands, type DiscoveryInfo } from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { checkForUpdate, updaterSupported, type Update } from "../../lib/updater";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";

type UpdateCheck = "idle" | "checking" | "upToDate" | "failed";

/** Version and updates, the shortcut list, and what the app found on disk. */
export function AboutSection({
  info,
  onUpdate,
  onShowShortcuts,
}: {
  info: DiscoveryInfo | null;
  onUpdate: (update: Update) => void;
  onShowShortcuts: () => void;
}) {
  const [version, setVersion] = useState<string | null>(null);
  const [supported, setSupported] = useState(false);
  const [state, setState] = useState<UpdateCheck>("idle");

  useEffect(() => {
    void getVersion().then(setVersion);
    void updaterSupported().then(setSupported);
  }, []);

  const checkNow = async () => {
    setState("checking");
    try {
      const update = await checkForUpdate();
      if (update) onUpdate(update);
      else setState("upToDate");
    } catch (e) {
      console.warn("update check failed:", e);
      setState("failed");
    }
  };

  const copyDiagnostics = async () => {
    try {
      await writeText(await unwrap(commands.getDiagnostics()));
      toast.info("Diagnostics copied");
    } catch (e) {
      toast.error(e);
    }
  };

  return (
    <>
      <Field
        label={`SteamReplay ${version ?? ""}`}
        description={
          supported
            ? state === "upToDate"
              ? "Up to date."
              : state === "failed"
                ? "Could not check for updates."
                : "Updates come from GitHub Releases."
            : "This build does not update itself (only the Windows installer and the AppImage do)."
        }
      >
        {supported && (
          <Button onClick={() => void checkNow()} disabled={state === "checking"}>
            {state === "checking" ? "Checking…" : "Check for updates"}
          </Button>
        )}
      </Field>
      <Field label="Keyboard shortcuts" description="Press ? anywhere to see them.">
        <Button onClick={onShowShortcuts}>Show shortcuts</Button>
      </Field>
      <Field label="Diagnostics" description="Paste this into a bug report.">
        <Button onClick={() => void copyDiagnostics()}>Copy diagnostics</Button>
      </Field>

      <h4 className="subheading">Steam folders</h4>
      <ul className="checks">
        {info?.checkedRoots.map((c) => (
          <li key={c.path} className={c.exists ? "" : "faint"}>
            {c.exists ? <Check size={14} className="ok" /> : <X size={14} />}
            <span className="mono break">{c.path}</span>
            {c.canonical && c.canonical !== c.path && <span className="mono faint break"> → {c.canonical}</span>}
          </li>
        ))}
      </ul>
      <h4 className="subheading">Accounts</h4>
      {info?.accounts.length ? (
        <ul className="checks">
          {info.accounts.map((a) => (
            <li key={a.accountId + a.recordingsRoot}>
              <span>{a.accountId}</span>
              <span className="mono faint break">{a.recordingsRoot}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No account with recordings found.</p>
      )}
    </>
  );
}
