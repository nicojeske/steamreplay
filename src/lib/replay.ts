import { convertFileSrc } from "@tauri-apps/api/core";

/** `replay://localhost/` on Linux, `http://replay.localhost/` on Windows. */
const BASE = convertFileSrc("", "replay");

/** URL of preview segment `s` (served by `media/preview/protocol.rs`). */
export function previewUrl(id: string, s: number): string {
  return `${BASE}preview/${encodeURIComponent(id)}/${s}.mp4`;
}
