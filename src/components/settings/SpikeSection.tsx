import { useEffect, useState } from "react";
import { commands, type SpikeParams } from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { sensitivityToThreshold, thresholdToSensitivity } from "../../lib/spikeTuning";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Slider } from "../ui/Slider";

/** Default sensitivity for games without their own tuning, and resetting the game presets. */
export function SpikeSection() {
  const [defaults, setDefaults] = useState<SpikeParams | null>(null);
  const [cleared, setCleared] = useState(false);

  useEffect(() => {
    unwrap(commands.getSpikeDefaults()).then(setDefaults, toast.error);
  }, []);

  const save = async (p: SpikeParams | null) => {
    try {
      await unwrap(commands.setSpikeDefaults(p));
      setDefaults(await unwrap(commands.getSpikeDefaults()));
    } catch (e) {
      toast.error(e);
    }
  };

  const clearPresets = async () => {
    try {
      await unwrap(commands.clearSpikePresets());
      setCleared(true);
    } catch (e) {
      toast.error(e);
    }
  };

  if (!defaults) return <p className="muted">Loading…</p>;
  const sensitivity = Math.round(thresholdToSensitivity(defaults.thresholdDb));
  return (
    <>
      <Field label="Default sensitivity" description="For games you have not tuned in the Moments tab." stacked>
        <div className="slider-row main">
          <Slider
            label="Default sensitivity"
            value={sensitivity}
            min={0}
            max={100}
            onChange={(v) => setDefaults({ ...defaults, thresholdDb: sensitivityToThreshold(v) })}
            onCommit={(v) => void save({ ...defaults, thresholdDb: sensitivityToThreshold(v) })}
          />
          <span className="slider-value mono">
            {sensitivity} <span className="faint">({defaults.thresholdDb.toFixed(1)} dB)</span>
          </span>
          <Button size="sm" variant="ghost" onClick={() => void save(null)}>
            Built-in
          </Button>
        </div>
      </Field>
      <Field label="Game presets" description="Games you tuned keep their own values until you reset them.">
        <Button onClick={() => void clearPresets()} disabled={cleared}>
          {cleared ? "Reset ✓" : "Reset all games"}
        </Button>
      </Field>
    </>
  );
}
