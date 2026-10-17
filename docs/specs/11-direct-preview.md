# M11: Original-quality preview

## 1. Goal and user-visible outcome
The M4 preview was a 360p H.264 transcode, so a 5120×1440 recording looked blurry, especially on HiDPI screens. After M11 the player shows **the original video**: Steam's HEVC chunks go straight into MSE, with no ffmpeg involved. You get full resolution, and a cold seek takes about 50 ms instead of about 1.4 s. If the webview cannot decode HEVC, the player falls back to the transcoded preview, which is now **720p** at a higher quality.

## 2. Prerequisites
- M4 (preview, `MsePlayer`, the `replay://` protocol, the transcode queue). M9 for the Info tab and the settings dialog.
- Steam's recordings are MPEG-DASH: `init-stream0.m4s` + `chunk-stream0-NNNNN.m4s` (HEVC Main, `hev1.2.4.L123.B0` in `session.mpd`, `yuvj420p`, ~60 fps) and `init-stream1.m4s` + `chunk-stream1-NNNNN.m4s` (AAC-LC). These are already fragmented MP4, so a `SourceBuffer` can take them byte for byte. A 5K chunk (3 s) is about 0.4–0.9 MB.

## 3. Scope / non-goals
**In scope:** the raw-chunk route in `replay://`, `DirectInfo` in `PreviewInfo`, lazy transcoding (`preview_fallback`), an `MsePlayer` that works with any source (several SourceBuffers, init segments, `timestampOffset`), the playback-mode setting, the 720p transcode, and gap and recovery fixes found while testing.

**Non-goals:** a native video surface outside the webview (libmpv and similar), HEVC export changes, and choosing the preview resolution in the settings.

## 4. Design

### Step-0 result: HEVC through MSE (Linux, WebKitGTK 2.54.1, gst 1.28.8, RTX 4080)
Checked with a standalone WebKitGTK window (same engine, same `WEBKIT_DISABLE_DMABUF_RENDERER=1` as the app) that runs the real `MsePlayer` bundle against `bg_1445790_20261015_210120`:
- `MediaSource.isTypeSupported` is true for `hev1.2.4.L123.B0`, `hvc1.1.6.L153.B0` and `mp4a.40.2`.
- WebKit decodes with **`nvh265dec`** (NVDEC): about 13 % decoder load, **0 dropped frames** at 5120×1440 60 fps.
- Cold seek (fetch + append + decode) is 47–57 ms, an in-buffer seek ~0 ms, and a far seek or a seek backwards while playing 30–56 ms, after which it keeps playing.
- The chunks carry the time since the recording started (chunk 1200 → 3597.0 s), exactly as in M4, so `timestampOffset = -v0` gives video time.
- **WebKitGTK quirk:** when playback starts right after the first append, the pipeline sometimes fails with `Failed to push buffer` (GStreamer: `streamsynchronizer0: could not send sticky events` while the HEVC decoder is still being created). It happened in 3 of 4 runs in the harness. The player reloads the MediaSource and **plays on** (about 0.2 s lost); see Recovery.
- **Windows (WebView2):** still to be checked on a teammate's machine (see §7). Chromium decodes HEVC through MSE with a hardware decoder (NVIDIA ≥ Maxwell gen 2, recent Intel/AMD). Without one, `isTypeSupported` is false and the 720p preview is used.

### Backend
- **`replay://localhost/chunk/<urlencoded id>/<v|a>/<init|n>.m4s`** (`protocol.rs`, `parse_chunk_path`, `ChunkFile`) serves the raw file from the session folder of an **open** preview session. The file name is built from the parsed kind and number (`chunks::{VIDEO_INIT, AUDIO_INIT, video_chunk, audio_chunk}`), so a path can never name another file. It returns 200/206 (`Range`), 404 when the file is missing, and `Content-Type` `video/mp4`/`audio/mp4`. `file_response` is shared with the segment route.
- **`PreviewInfo`** gains:
  - `mode: PreviewMode` (`auto` | `original` | `transcode`, setting `previewMode`, default `auto`),
  - `direct: Option<DirectInfo { videoMime, audioMime?, first, count, missing, chunkS, v0 }>`, set in modes `auto`/`original` when `session.mpd` has codecs (`preview::direct_info`),
  - `backend: Option<Backend>`, which is `None` while transcoding is off.
- **Lazy transcoding:** when `direct` is set, `get_preview_info` does **not** detect a backend and does not warm up any segments. `PreviewSession::transcoding` is off, so `set_playhead` (and `PreviewState::set_backend`) do nothing. **`preview_fallback(id, t)`** detects or reads the backend (`preview_backend`, which now takes `dir` + `first_chunk`), calls `enable_transcoding(b)`, queues around `t` and returns the new info.
- An open session is reused only when its chunk range **and** mode match (`PreviewState::open`, `get_preview_info`), so a mode change applies the next time a recording opens.

### Transcode (fallback)
- **720p** instead of 360p (`-2:720`; 5120×1440 → 2560×720), `-level 5.1`, `MIME` = `avc1.640033`.
- Quality: NVENC `-cq 23`, libx264 `-crf 22`, QSV `-global_quality 23`, VAAPI `-qp 22`, AMF `-b:v 6M`.
- Cost: the CUDA integration test (6 s clip) takes 0.69 s, the same as 360p (0.65–0.75 s). NVDEC is still the bottleneck.
- Color: CUDA and software output stay tagged full range (`yuvj420p`, `pc`), so no range conversion is needed.
- Cache folders are now `<sha1>-<first>-720`. Opening a session removes its old 360p folder (`remove_stale_sessions`); other old folders leave by LRU.

### Frontend
- **`MsePlayer` takes a `Source`** (`src/lib/msePlayer.ts`): `{ kind, segS, count, mimes[], init[] | null, offset, urls(s) → string[] | null }`.
  - `originalSource(id, direct)`: one segment = one chunk, two SourceBuffers (video and audio; one if the session has no audio), init segments fetched once and appended to every new MediaSource, `timestampOffset = -v0`, and missing chunks give `null` (skipped without a request).
  - `transcodeSource(id, info)`: the M4 behavior.
  - All M4 quirk handling stays: one contiguous run, a fresh MediaSource for a seek outside it, snapping into a range, the quota retry, and trimming behind the playhead (across all buffers). A 404 on the video URL skips the segment; a 404 on audio only leaves that buffer empty.
  - `previewPlayhead` is only sent for `transcode` sources.
- **Gaps:** a stall in front of a buffered range is jumped when the range is less than `3 s + segS` away (one missing chunk is a 3.03 s hole, more than the old limit). The check also runs after every append while playing, because `waiting` fires before the data behind the gap arrives.
- **Recovery:** a media error reloads the MediaSource once and **resumes playback** when the video was playing (`resume = !paused`; no `pause` event fires before the error).
- **Choosing the source (`Player.tsx`):** original when `info.direct` is set and either the mode is `original` or `canPlayOriginal(direct)` (`isTypeSupported` for both mimes) holds and the session has not failed before. Otherwise `preview_fallback` is called and the transcode source used. In `auto` mode, an error status from an original-source player (after its one recovery) calls `preview_fallback` and rebuilds the player with the transcode source at the playhead. The session id is remembered in `fellBack` until restart.
- **Info tab:** Preview shows "Original video" or "720p preview, <backend>".
- **Settings → Video preview:** "Playback" (Original video when possible / Always original video / Always 720p preview). The decoder setting now says it is for the 720p preview.

## 5. Edge cases
- **Missing chunk:** skipped without a request, and the 3 s hole is jumped (checked: chunk 21 missing, playback went 55 → 70 s).
- **Pruned session:** `first` and `v0` come from the first chunk on disk, as in M4.
- **No audio stream** (empty mpd codecs): a single SourceBuffer.
- **Ragged recording end:** Steam's last chunk can have a hole between audio and video (`bg_1445790…`: `3715.96–3716.08, 3716.63–3717.26`). The player jumps the hole, but the last ~0.6 s may not play (no keyframe after the gap), and `ended` does not fire. Harmless; left as is.
- **Webview without HEVC:** `isTypeSupported` is false, so the player uses the transcode source straight away.
- **HEVC claimed but undecodable:** it fails twice, then falls back (`auto`) or shows the error (`original`).
- **Live session:** same as M4 (the info is not refreshed).

## 6. Acceptance criteria
- [x] Step 0 on Linux: HEVC MSE playback at 5K60 with 0 dropped frames (harness, see §4).
- [ ] Step 0 on Windows (WebView2), on a teammate's machine.
- [x] Cold seek < 300 ms with the original video (measured 47–57 ms in the harness).
- [x] Playback across chunk boundaries, a far seek while playing, and missing chunks (harness).
- [~] In the app: the Info tab says "Original video" and the image is sharp at 5K; clap check (audio/video sync with two SourceBuffers). Playback is choppy on Linux in large windows (see §8).
- [ ] In the app: "Always 720p preview" plays the transcode; switching back works on the next open.
- [x] 720p transcode: `h264 High 720 level 51` with AAC (integration test), CUDA time unchanged.
- [x] clippy, tests, tsc and lint are clean.

## 7. Verification steps
- **Unit tests:** `protocol::tests::chunk_paths` (paths, kinds, numbers, no traversal), `preview::tests::direct_chunks`, and `transcode::tests::args_per_backend` (720, level 5.1).
- **Integration test:** `STEAMREPLAY_SESSION_DIR=<clip bg dir> cargo test preview_fixture -- --ignored --nocapture` expects `h264,High,720,51`.
- **Harness** (Linux, no app needed): bundle `src/lib/msePlayer.ts` with `../bindings` and `./replay` stubbed (chunk URLs pointing to a local `http.server` over the session folder), and open it in a WebKitGTK window via PyGObject with `WEBKIT_DISABLE_DMABUF_RENDERER=1`. Scenarios: cold seek, play 8 s, a far seek while playing, a seek backwards, the missing-chunk gap, and the end. `GST_DEBUG=2,GST_ELEMENT_FACTORY:4` shows the decoder.
- **Manual** (`pnpm tauri dev`, `bg_1445790_20261015_210120`): sharpness, seek to spikes, play across chunks, the clap check, the pruned session `bg_1623730_20261010_164846`, mode "Always 720p preview", and `pgrep ffmpeg` (nothing should run while the original plays).
- **Windows** (teammate): `isTypeSupported('video/mp4; codecs="hev1.2.4.L123.B0"')`, original playback on NVIDIA, the Info tab label, and the fallback on a machine without HEVC decode.

## 8. Open questions / risks
- **5K frame rate on Linux.** The app runs WebKitGTK with `WEBKIT_DISABLE_DMABUF_RENDERER=1` (`main.rs`; the DMA-BUF renderer crashes on NVIDIA + Wayland with `Error 71 (Protocol error)`). Without it, every decoded frame is copied to the compositor (5120×1440 ≈ 11 MB per frame). Measured in the harness: 57 fps in a 1280×400 window, but only **~40 fps in a 2560×1400 window** (rAF drops to 41/s, `droppedVideoFrames` stays 0). 720p stays at 60 fps in the same window. Page code cannot make WebKit downscale before the copy. Decision: keep the original as the default, and use "Always 720p preview" when it's choppy. Next step: find a way to use the DMA-BUF renderer without the crash (separate investigation). Not expected on Windows (WebView2 composites hardware-decoded video without copying).
- **WebView2 HEVC** depends on a hardware decoder and the GPU driver. The fallback covers machines without one, but this is untested on Windows.
- **`Failed to push buffer`** happens often at the first play with HEVC on WebKitGTK. The recovery hides it (~0.2 s). If it shows up mid-playback in real use, wait for `canplay` before calling `play()` after a fresh MediaSource.
- **Memory:** about 2–3 MB/s for 5K video, so the 60 s kept behind the playhead stays small. Higher-bitrate recordings would raise this proportionally.
