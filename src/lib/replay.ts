import { convertFileSrc } from "@tauri-apps/api/core";

/** `replay://localhost/` on Linux, `http://replay.localhost/` on Windows. */
const BASE = convertFileSrc("", "replay");

/** URL of preview segment `s` (served by `media/preview/protocol.rs`). */
export function previewUrl(id: string, s: number): string {
  return `${BASE}preview/${encodeURIComponent(id)}/${s}.mp4`;
}

/** URL of the library thumbnail of a session (served by `media/thumbs.rs`). */
export function thumbnailUrl(id: string): string {
  return `${BASE}thumb/${encodeURIComponent(id)}.jpg`;
}
