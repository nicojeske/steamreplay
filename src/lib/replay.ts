import { convertFileSrc } from "@tauri-apps/api/core";

/** `replay://localhost/` on Linux, `http://replay.localhost/` on Windows. */
const BASE = convertFileSrc("", "replay");

/** URL of preview segment `s` (served by `media/preview/protocol.rs`). */
export function previewUrl(id: string, s: number): string {
  return `${BASE}preview/${encodeURIComponent(id)}/${s}.mp4`;
}

/** URL of a raw Steam DASH file: video or audio, init segment or chunk number (direct playback). */
export function chunkUrl(id: string, kind: "v" | "a", chunk: number | "init"): string {
  return `${BASE}chunk/${encodeURIComponent(id)}/${kind}/${chunk}.m4s`;
}

/** URL of the library thumbnail of a session (served by `media/thumbs.rs`). */
export function thumbnailUrl(id: string): string {
  return `${BASE}thumb/${encodeURIComponent(id)}.jpg`;
}

/** URL of the hover frame of chunk index `k` (from video time 0) of an open session (`media/thumbs.rs`). */
export function frameUrl(id: string, k: number): string {
  return `${BASE}frame/${encodeURIComponent(id)}/${k}.jpg`;
}
