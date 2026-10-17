import { open } from "@tauri-apps/plugin-dialog";
import { Monitor, Moon, Sun } from "lucide-react";
import { commands, type DiscoveryInfo } from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { toast } from "../../state/toasts";
import { useUi, type Theme } from "../../state/ui";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Segmented } from "../ui/Segmented";

const THEMES: { value: Theme; label: React.ReactNode }[] = [
  {
    value: "system",
    label: (
      <>
        <Monitor size={14} /> System
      </>
    ),
  },
  {
    value: "dark",
    label: (
      <>
        <Moon size={14} /> Dark
      </>
    ),
  },
  {
    value: "light",
    label: (
      <>
        <Sun size={14} /> Light
      </>
    ),
  },
];

/** Theme and the recordings folder override. */
export function GeneralSection({ info, onChanged }: { info: DiscoveryInfo | null; onChanged: () => void }) {
  const theme = useUi((s) => s.theme);

  const setOverride = async (path: string | null) => {
    try {
      await unwrap(commands.setRecordingsRootOverride(path));
      onChanged();
    } catch (e) {
      toast.error(e);
    }
  };

  const pick = async () => {
    const dir = await open({ directory: true, title: "Choose the gamerecordings folder" });
    if (typeof dir === "string") await setOverride(dir);
  };

  return (
    <>
      <Field label="Theme" description="System follows the light or dark setting of your OS.">
        <Segmented label="Theme" value={theme} options={THEMES} onChange={(t) => useUi.getState().set({ theme: t })} />
      </Field>
      <Field
        label="Recordings folder"
        description={
          info?.recordingsRootOverride ? (
            <span className="mono break">{info.recordingsRootOverride}</span>
          ) : (
            "Found automatically in Steam's folders. Choose one if Steam records somewhere else."
          )
        }
      >
        <div className="row">
          <Button onClick={() => void pick()}>Choose folder…</Button>
          {info?.recordingsRootOverride && (
            <Button variant="ghost" onClick={() => void setOverride(null)}>
              Automatic
            </Button>
          )}
        </div>
      </Field>
    </>
  );
}
