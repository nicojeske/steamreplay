import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, type ExportPreset, type ExportPrefs } from "../../bindings";
import { PRESETS, PRESET_ORDER, presetLabel } from "../../lib/exportPresets";
import { unwrap } from "../../lib/ipc";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Segmented } from "../ui/Segmented";

/** Default export folder, and the mode and preset the next export starts with. */
export function ExportSection() {
  const [dir, setDir] = useState<string | null | undefined>(undefined);
  const [prefs, setPrefs] = useState<ExportPrefs | null>(null);

  useEffect(() => {
    unwrap(commands.getExportDirOverride()).then(setDir, toast.error);
    unwrap(commands.getExportPrefs()).then(setPrefs, toast.error);
  }, []);

  const saveDir = async (path: string | null) => {
    try {
      await unwrap(commands.setExportDirOverride(path));
      setDir(path);
    } catch (e) {
      toast.error(e);
    }
  };

  const pickDir = async () => {
    const chosen = await open({ directory: true, title: "Choose the export folder" });
    if (typeof chosen === "string") await saveDir(chosen);
  };

  const savePrefs = async (p: ExportPrefs) => {
    setPrefs(p);
    try {
      await unwrap(commands.setExportPrefs(p));
    } catch (e) {
      toast.error(e);
    }
  };

  const mode = prefs?.mode ?? "fast";
  const preset = prefs?.precise?.preset ?? "discord";
  const choosePreset = (p: ExportPreset) => {
    if (!prefs) return;
    const opts = p === "custom" ? (prefs.precise?.opts ?? PRESETS.discord.opts) : PRESETS[p].opts;
    void savePrefs({ ...prefs, precise: { preset: p, opts } });
  };

  return (
    <>
      <Field
        label="Export folder"
        description={dir ? <span className="mono break">{dir}</span> : "Steam's export folder (from Steam's recording settings)."}
      >
        <div className="row">
          <Button onClick={() => void pickDir()}>Choose folder…</Button>
          {dir && (
            <Button variant="ghost" onClick={() => void saveDir(null)}>
              Use Steam's
            </Button>
          )}
        </div>
      </Field>
      {prefs && (
        <>
          <Field label="Mode" description="The Export tab remembers the last choice; this edits the same setting.">
            <Segmented
              label="Export mode"
              value={mode}
              options={[
                { value: "fast", label: "Fast" },
                { value: "precise", label: "Precise" },
              ]}
              onChange={(m) => void savePrefs({ ...prefs, mode: m })}
            />
          </Field>
          <Field label="Precise preset">
            <select className="select" value={preset} onChange={(e) => choosePreset(e.target.value as ExportPreset)} aria-label="Preset">
              {PRESET_ORDER.map((p) => (
                <option key={p} value={p}>
                  {presetLabel(p)}
                </option>
              ))}
            </select>
          </Field>
        </>
      )}
    </>
  );
}
