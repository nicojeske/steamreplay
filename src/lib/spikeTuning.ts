import type { SpikeParams } from "../bindings";

/** Sensitivity 0 → 20 dB above the baseline, 100 → 4 dB (the default 10 dB is at 62.5). */
const SENS_MAX_DB = 20;
const SENS_MIN_DB = 4;

export function sensitivityToThreshold(s: number): number {
  return SENS_MAX_DB - ((SENS_MAX_DB - SENS_MIN_DB) * s) / 100;
}

export function thresholdToSensitivity(db: number): number {
  return Math.min(100, Math.max(0, ((SENS_MAX_DB - db) * 100) / (SENS_MAX_DB - SENS_MIN_DB)));
}

/** Laughter (M15): sensitivity 0 → probability 0.25, 100 → 0.0004, on a log scale, so the default 0.01
 * is at 50. The same range as `LAUGH_THRESHOLD_MIN/MAX` (spikes.rs). */
const LAUGH_SENS_MAX = 0.25;
/** `DEFAULT_LAUGH_THRESHOLD` (spikes.rs). */
export const LAUGH_THRESHOLD_BUILT_IN = 0.01;
const LAUGH_SENS_MIN = 0.0004;
const LAUGH_LOG_SPAN = Math.log(LAUGH_SENS_MIN / LAUGH_SENS_MAX);

export function laughSensitivityToThreshold(s: number): number {
  return LAUGH_SENS_MAX * Math.exp((LAUGH_LOG_SPAN * s) / 100);
}

export function laughThresholdToSensitivity(p: number): number {
  return Math.min(100, Math.max(0, (Math.log(p / LAUGH_SENS_MAX) * 100) / LAUGH_LOG_SPAN));
}

/** A laughter probability for display: "1.0 %", "0.04 %", "25 %". */
export function formatLaughThreshold(p: number): string {
  const pct = p * 100;
  return `${pct >= 10 ? pct.toFixed(0) : pct.toPrecision(2)} %`;
}

type NumKey = "thresholdDb" | "baselineS" | "minLenMs" | "mergeGapS" | "padS";

/** The "Advanced" sliders. Must stay inside `SpikeParams::clamped` (spikes.rs). */
export const ADVANCED: { key: NumKey; label: string; min: number; max: number; step: number; unit: string }[] = [
  { key: "thresholdDb", label: "Threshold", min: 3, max: 25, step: 0.5, unit: "dB" },
  { key: "baselineS", label: "Baseline window", min: 5, max: 120, step: 5, unit: "s" },
  { key: "minLenMs", label: "Min length", min: 100, max: 2000, step: 50, unit: "ms" },
  { key: "mergeGapS", label: "Merge gap", min: 0, max: 20, step: 0.5, unit: "s" },
  { key: "padS", label: "Padding", min: 0, max: 5, step: 0.25, unit: "s" },
];

/** The "Advanced" sliders that apply to laughter: the rest only tunes the loudness detector. */
export const LAUGH_ADVANCED = ADVANCED.filter((a) => a.key === "mergeGapS" || a.key === "padS");

export function paramsEqual(a: SpikeParams, b: SpikeParams): boolean {
  return (Object.keys(a) as (keyof SpikeParams)[]).every((k) => a[k] === b[k]);
}
