import { Collapsible } from "radix-ui";
import { ChevronDown, RotateCcw, SlidersHorizontal } from "lucide-react";
import type { SpikeParams } from "../../bindings";
import { ADVANCED, sensitivityToThreshold, thresholdToSensitivity } from "../../lib/spikeTuning";
import { useUi } from "../../state/ui";
import { Button } from "../ui/Button";
import { Slider } from "../ui/Slider";

type Props = {
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

/** Sliders for the loud-moment detection. Changes apply live and are saved for this game. */
export function Detection({ params, onChange, onReset, count, busy, gameName }: Props) {
  const open = useUi((s) => s.detectionOpen);
  const sensitivity = params ? Math.round(thresholdToSensitivity(params.thresholdDb)) : null;
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
                onChange={(v) => onChange({ ...params, thresholdDb: sensitivityToThreshold(v) })}
              />
              <span className="slider-value mono">{sensitivity}</span>
            </div>
            <p className="detection-status muted">
              {count} found{count >= params.maxResults && ` (top ${params.maxResults})`} · threshold {params.thresholdDb.toFixed(1)} dB
              {busy && " · recalculating…"}
            </p>
            <Collapsible.Root className="detection-advanced">
              <Collapsible.Trigger className="link-button">
                Advanced <ChevronDown size={13} className="chevron" aria-hidden />
              </Collapsible.Trigger>
              <Collapsible.Content className="advanced-grid">
                {ADVANCED.map((a) => (
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
