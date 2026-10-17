import { useCallback, useEffect, useState } from "react";
import { commands, type Backend, type PreviewSettings } from "../../bindings";
import { BACKEND_NAMES } from "../../lib/backends";
import { formatBytes } from "../../lib/format";
import { unwrap } from "../../lib/ipc";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";

/** Preview backend and cache size. */
export function PreviewSection() {
  const [settings, setSettings] = useState<PreviewSettings | null>(null);
  const [cacheMb, setCacheMb] = useState("");

  const reload = useCallback(
    () =>
      unwrap(commands.getPreviewSettings())
        .then((s) => {
          setSettings(s);
          setCacheMb(String(s.cacheMb));
        })
        .catch(toast.error),
    [],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  if (!settings) return <p className="muted">Loading…</p>;

  const save = async (backend: Backend | null, mb: number) => {
    try {
      await unwrap(commands.setPreviewSettings(backend, mb));
      await reload();
    } catch (e) {
      toast.error(e);
    }
  };

  const mb = Number.parseInt(cacheMb, 10);
  return (
    <>
      <Field label="Decoder" description="Automatic tries the GPU first. Choosing Automatic again re-detects on the next preview.">
        <select
          className="select"
          value={settings.backend ?? "auto"}
          onChange={(e) => void save(e.target.value === "auto" ? null : (e.target.value as Backend), settings.cacheMb)}
        >
          <option value="auto">Automatic{settings.detected ? ` (${BACKEND_NAMES[settings.detected]})` : ""}</option>
          {(Object.keys(BACKEND_NAMES) as Backend[]).map((b) => (
            <option key={b} value={b}>
              {BACKEND_NAMES[b]}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Cache limit" description={`${formatBytes(settings.cacheBytes)} used. Old preview segments are removed above this.`}>
        <div className="row">
          <input className="input num-input" type="number" min={100} step={100} value={cacheMb} onChange={(e) => setCacheMb(e.target.value)} />
          <span className="muted">MB</span>
          <Button disabled={!Number.isFinite(mb) || mb === settings.cacheMb} onClick={() => void save(settings.backend, mb)}>
            Apply
          </Button>
        </div>
      </Field>
    </>
  );
}
