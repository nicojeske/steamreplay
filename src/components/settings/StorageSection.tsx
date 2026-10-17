import { useCallback, useEffect, useState } from "react";
import { Trash2 } from "lucide-react";
import { commands, type CacheSizes } from "../../bindings";
import { formatBytes } from "../../lib/format";
import { unwrap } from "../../lib/ipc";
import { toast } from "../../state/toasts";
import { useView } from "../../state/view";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";

/** Cache sizes and one button to clear them all. */
export function StorageSection() {
  const [sizes, setSizes] = useState<CacheSizes | null>(null);
  const [clearing, setClearing] = useState(false);

  const reload = useCallback(() => unwrap(commands.getCacheSizes()).then(setSizes, toast.error), []);

  useEffect(() => {
    void reload();
  }, [reload]);

  const clear = async () => {
    setClearing(true);
    try {
      const r = await unwrap(commands.clearCaches());
      toast.info(r.skipped > 0 ? "Caches cleared. Some files were in use and were kept." : "Caches cleared.");
      useView.getState().bumpCache();
    } catch (e) {
      toast.error(e);
    } finally {
      setClearing(false);
      await reload();
    }
  };

  const rows: [string, keyof CacheSizes][] = [
    ["Waveforms", "waveform"],
    ["Thumbnails", "thumbs"],
    ["Video previews", "preview"],
    ["Export temp files", "export"],
  ];
  const total = sizes ? Object.values(sizes).reduce((a, b) => a + b, 0) : 0;

  return (
    <>
      <dl className="facts storage-facts">
        {rows.map(([label, key]) => (
          <div key={key} className="fact-row">
            <dt>{label}</dt>
            <dd className="num">{sizes ? formatBytes(sizes[key]) : "…"}</dd>
          </div>
        ))}
      </dl>
      <Field label="Clear caches" description="Everything is rebuilt when a recording is opened again.">
        <Button variant="danger" icon={<Trash2 size={14} />} onClick={() => void clear()} disabled={clearing}>
          {clearing ? "Clearing…" : `Clear ${sizes ? formatBytes(total) : ""}`}
        </Button>
      </Field>
    </>
  );
}
