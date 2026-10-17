import { useEffect, useState } from "react";
import { commands, type SpikeParams } from "../../bindings";
import { unwrap } from "../../lib/ipc";
import { sensitivityToThreshold, thresholdToSensitivity } from "../../lib/spikeTuning";
import { toast } from "../../state/toasts";
import { useUi } from "../../state/ui";
import { PathActions } from "../PathActions";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Segmented } from "../ui/Segmented";
import { Slider } from "../ui/Slider";

/** Default sensitivity for games without their own tuning, and resetting the game presets. */
export function SpikeSection() {
  const [defaults, setDefaults] = useState<SpikeParams | null>(null);
  const [cleared, setCleared] = useState(false);
  const labelling = useUi((s) => s.labelling);
  const [labelsDir, setLabelsDir] = useState<string | null>(null);

  useEffect(() => {
    unwrap(commands.getSpikeDefaults()).then(setDefaults, toast.error);
    unwrap(commands.labelsDir()).then(setLabelsDir, toast.error);
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
      <Field
        label="Labelling mode"
        description={
          <>
            For the detection experiments: the Moments tab shows the candidates from{" "}
            <span className="mono">experiments/moments</span> to rate with 1/2/3, and X marks a missed moment.
            {labelsDir && (
              <>
                {" "}
                Labels folder: <span className="mono break">{labelsDir}</span> {labelling && <PathActions path={labelsDir} />}
              </>
            )}
          </>
        }
      >
        <Segmented
          label="Labelling mode"
          value={labelling ? "on" : "off"}
          options={[
            { value: "off", label: "Off" },
            { value: "on", label: "On" },
          ]}
          onChange={(v) => useUi.getState().set({ labelling: v === "on" })}
        />
      </Field>
    </>
  );
}
