import type { ExportPreset, PreciseOpts, VideoCodec } from "../bindings";

/** Presets of a precise export. `custom` takes the options from "Advanced". */
export const PRESETS: Record<Exclude<ExportPreset, "custom">, { label: string; opts: PreciseOpts }> = {
  discord: {
    label: "Discord (10 MB)",
    opts: { codec: "h264", maxHeight: 720, fps: 60, crop16x9: false, target: { kind: "maxSizeMb", mb: 10 } },
  },
  discordNitro: {
    label: "Discord Nitro (500 MB)",
    opts: { codec: "h264", maxHeight: 1080, fps: 60, crop16x9: false, target: { kind: "maxSizeMb", mb: 500 } },
  },
  original: {
    label: "Original quality",
    opts: { codec: "hevc", maxHeight: null, fps: null, crop16x9: false, target: { kind: "crf", q: 22 } },
  },
};

export const PRESET_ORDER: ExportPreset[] = ["discord", "discordNitro", "original", "custom"];

export function presetLabel(p: ExportPreset): string {
  return p === "custom" ? "Custom" : PRESETS[p].label;
}

export const CODEC_LABELS: Record<VideoCodec, string> = { h264: "H.264", hevc: "HEVC", av1: "AV1" };

/** Mirrors `encoders::AUDIO_KBPS` and `MIN_VIDEO_KBPS` in Rust. */
const AUDIO_KBPS = 160;
export const MIN_VIDEO_KBPS = 300;

/** Video bitrate that keeps `mb` megabytes for `durS`. Mirrors `encoders::size_kbps`. */
export function videoKbps(mb: number, durS: number): number {
  return (mb * 8 * 1000 * 0.95) / Math.max(durS, 0.001) - AUDIO_KBPS;
}

/** More than 1 % wider than 16:9, so a centre crop makes sense. Mirrors `export::filter_chain`. */
export function isWiderThan16x9(w: number | null | undefined, h: number | null | undefined): boolean {
  return !!w && !!h && w * 9 > h * 16 * 1.01;
}
