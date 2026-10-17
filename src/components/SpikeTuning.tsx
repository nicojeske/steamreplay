import type { SpikeParams } from "../bindings";
import { ADVANCED, sensitivityToThreshold, thresholdToSensitivity } from "../lib/spikeTuning";

type Props = {
  params: SpikeParams;
  onChange: (p: SpikeParams) => void;
  onReset: () => void;
  count: number;
  /** A request with the current params is still running. */
  busy: boolean;
  lane: boolean;
  onLane: (on: boolean) => void;
};

/** As many decimals as the slider step has. */
function formatValue(v: number, step: number, unit: string): string {
  const decimals = (String(step).split(".")[1] ?? "").length;
  return `${v.toFixed(decimals)} ${unit}`;
}

/** Sliders for the loud-part detection. Changes apply live and are saved for this game. */
export function SpikeTuning({ params, onChange, onReset, count, busy, lane, onLane }: Props) {
  const sensitivity = thresholdToSensitivity(params.thresholdDb);
  return (
    <div className="spike-tuning">
      <div className="spike-tuning-row">
        <label className="spike-slider main">
          <span>Sensitivity</span>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            value={Math.round(sensitivity)}
            onChange={(e) => onChange({ ...params, thresholdDb: sensitivityToThreshold(Number(e.target.value)) })}
          />
          <span className="mono">
            {Math.round(sensitivity)} <span className="muted">({params.thresholdDb.toFixed(1)} dB)</span>
          </span>
        </label>
        <span className={busy ? "muted" : undefined}>
          {count} loud parts
          {count >= params.maxResults && <span className="muted"> · showing top {params.maxResults}</span>}
          {busy && " · recalculating…"}
        </span>
        <span className="spacer" />
        <label className="loop-toggle" title="Loudness above the baseline under the waveform, with the threshold line">
          <input type="checkbox" checked={lane} onChange={(e) => onLane(e.target.checked)} /> Loudness lane
        </label>
        <button type="button" onClick={onReset} title="Back to the defaults (Settings → Loud parts)">
          Reset
        </button>
      </div>
      <details className="spike-advanced">
        <summary>Advanced</summary>
        <div className="spike-advanced-grid">
          {ADVANCED.map((a) => (
            <label key={a.key} className="spike-slider">
              <span>{a.label}</span>
              <input
                type="range"
                min={a.min}
                max={a.max}
                step={a.step}
                value={params[a.key]}
                onChange={(e) => onChange({ ...params, [a.key]: Number(e.target.value) })}
              />
              <span className="mono">{formatValue(params[a.key], a.step, a.unit)}</span>
            </label>
          ))}
        </div>
      </details>
    </div>
  );
}
