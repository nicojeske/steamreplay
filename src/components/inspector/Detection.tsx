import { Collapsible } from "radix-ui";
import { ChevronDown, RotateCcw, SlidersHorizontal } from "lucide-react";
import type { SpikeParams } from "../../bindings";
import {
  ADVANCED,
  LAUGH_ADVANCED,
  formatLaughThreshold,
  laughSensitivityToThreshold,
  laughThresholdToSensitivity,
  sensitivityToThreshold,
  thresholdToSensitivity,
} from "../../lib/spikeTuning";
import type { Detector } from "../editor/useAnalysis";
import { useUi } from "../../state/ui";
import { Button } from "../ui/Button";
import { Slider } from "../ui/Slider";

type Props = {
  /** Laughter tunes `laughThreshold` plus merge gap and padding; loudness tunes everything else. */
  detector: Detector;
  params: SpikeParams | null;
  onChange: (p: SpikeParams) => void;
  onReset: () => void;
  count: number;
  /** A request with the current params is still running. */
  busy: boolean;
  gameName: string;
};

/** As many decimals as the slider step has. */
function formatValue(v: number, step: number, unit: string): string {
  const decimals = (String(step).split(".")[1] ?? "").length;
  return `${v.toFixed(decimals)} ${unit}`;
}

/** Sliders for the moment detection. Changes apply live and are saved for this game. */
export function Detection({ detector, params, onChange, onReset, count, busy, gameName }: Props) {
  const open = useUi((s) => s.detectionOpen);
  const laughs = detector === "laughter";
  const sensitivity = params
    ? Math.round(laughs ? laughThresholdToSensitivity(params.laughThreshold) : thresholdToSensitivity(params.thresholdDb))
    : null;
  const withSensitivity = (p: SpikeParams, v: number): SpikeParams =>
    laughs ? { ...p, laughThreshold: laughSensitivityToThreshold(v) } : { ...p, thresholdDb: sensitivityToThreshold(v) };
  const advanced = laughs ? LAUGH_ADVANCED : ADVANCED;
  return (
    <Collapsible.Root className="detection" open={open} onOpenChange={(o) => useUi.getState().set({ detectionOpen: o })}>
      <Collapsible.Trigger className="detection-trigger">
        <SlidersHorizontal size={15} aria-hidden />
        <span>Detection</span>
        <span className="faint num">{sensitivity !== null && `sensitivity ${sensitivity}`}</span>
        <span className="spacer" />
        <ChevronDown size={15} className="chevron" aria-hidden />
      </Collapsible.Trigger>
      <Collapsible.Content className="detection-content">
        {params && sensitivity !== null ? (
          <>
            <div className="slider-row main">
              <span className="slider-label">Sensitivity</span>
              <Slider
                label="Sensitivity"
                value={sensitivity}
                min={0}
                max={100}
                onChange={(v) => onChange(withSensitivity(params, v))}
              />
              <span className="slider-value mono">{sensitivity}</span>
            </div>
            <p className="detection-status muted">
              {count} {laughs ? "laughs" : "loud moments"} found{count >= params.maxResults && ` (top ${params.maxResults})`} · threshold{" "}
              {laughs ? formatLaughThreshold(params.laughThreshold) : `${params.thresholdDb.toFixed(1)} dB`}
              {busy && " · recalculating…"}
            </p>
            <Collapsible.Root className="detection-advanced">
              <Collapsible.Trigger className="link-button">
                Advanced <ChevronDown size={13} className="chevron" aria-hidden />
              </Collapsible.Trigger>
              <Collapsible.Content className="advanced-grid">
                {advanced.map((a) => (
                  <div key={a.key} className="slider-row">
                    <span className="slider-label">{a.label}</span>
                    <Slider
                      label={a.label}
                      value={params[a.key]}
                      min={a.min}
                      max={a.max}
                      step={a.step}
                      onChange={(v) => onChange({ ...params, [a.key]: v })}
                    />
                    <span className="slider-value mono">{formatValue(params[a.key], a.step, a.unit)}</span>
                  </div>
                ))}
              </Collapsible.Content>
            </Collapsible.Root>
            <div className="detection-footer">
              <span className="faint truncate">Saved for {gameName}</span>
              <Button size="sm" variant="ghost" icon={<RotateCcw size={13} />} onClick={onReset} title="Back to the defaults (Settings → Loud moments)">
                Reset
              </Button>
            </div>
          </>
        ) : (
          <p className="muted detection-status">Available once the audio is analysed.</p>
        )}
      </Collapsible.Content>
    </Collapsible.Root>
  );
}
