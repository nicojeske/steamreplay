# M4: Video preview synced with the timeline

> Built **before** M3 (see the build order in `00-overview.md`). This spec describes what was implemented.

## 1. Goal and user-visible outcome
A video player above the timeline shows the recording at the playhead. You can play, pause and scrub. Clicking the timeline or a spike (or pressing N/P) moves the player there. While playing, the timeline playhead moves along and the view pages with it. The preview is low-res (360 p high) and plays smoothly, and after a seek it starts in about 1.4 s (cold) or < 100 ms (cached).

## 2. Prerequisites
- M1 and M2: sessions, chunk inventory, timeline with playhead, `media/ffmpeg.rs`, job handling (`start_job`/`cancel_job`). M3 is **not** needed.
- `00-overview.md`: stack decision ("Why the preview needs transcoding") and DASH facts.

## 3. Scope / non-goals
**In scope:** `media/chunks.rs`, `media/preview/` (transcode, backends, cache, queue, `replay://` protocol), the `Player` component and `MsePlayer`, playhead sync, follow mode, playback keys, preview settings.

**Non-goals:**
- full-quality preview,
- playing the original HEVC in the webview (not reliable, see the risks),
- audio-only scrubbing sounds,
- **loop selection** (`Ctrl+L`): it needs the in/out selection, so it moved to M3.

## 4. Design

### Step-0 result: MSE in the webview
- **Linux (WebKitGTK 2.54.1, gst-plugins-bad 1.28.8, gst-libav 1.28.8, NVIDIA RTX 4080):** MSE works. `MediaSource.isTypeSupported('video/mp4; codecs="avc1.640028, mp4a.40.2"')` is true, `requestVideoFrameCallback` exists (but does not fire for MSE video, see Sync), and two appended 12 s segments play across the boundary with 0 dropped frames. NVENC and libx264 output both play. The ping-pong fallback was not needed.
  - One run of an early NVENC segment failed with `Failed to push buffer` (media error 4). It did not happen again in 4 later runs. `MsePlayer` rebuilds the MediaSource once on a media error.
- **Windows (WebView2):** still to be checked on a teammate's machine (see §7).

### WebKitGTK MSE quirks (found in step 0, handled by `MsePlayer`)
1. **Appends must go in increasing order.** Segments overlap by one frame, so appending segment *s* after *s+1* removed the first GOP of *s+1* (a 4 s hole). The player keeps **one contiguous run** and only ever appends the next segment of it.
2. **A seek outside the run starts a fresh `MediaSource`.** Removing all buffered data and reusing the SourceBuffer made the next seek land several seconds past the target (by the time played since the last seek). If the video was playing, playback resumes once the target is loaded.
3. **Buffered ranges start where audio and video both have data.** Steam's audio starts a few ms after the video, so a range starts at e.g. 600.04 for a segment whose first frame is at 600.00. Seek targets less than 0.25 s before a range are moved into it.

### Measurements (dev machine, RTX 4080, 5120×1440 HEVC, 1280×360 H.264 output)
| What | Time |
|---|---|
| CLI, 12 s segment, CUDA | ~1.4 s (incl. process start) |
| Integration test, 6 s clip, CUDA / software | 0.65–0.75 s / 1.3 s |
| App, cold seek, one job at a time | 1.36–1.49 s (first after warm-up 1.69 s) |
| App, cold seek, two concurrent jobs | ~2.5 s each (NVDEC-bound) |
| App, cached seek | 28–76 ms |
| Backend detection + open (first time) | ~0.7 s |

So the queue runs **one transcode at a time per session**, in hardware and in software mode. Two concurrent jobs do not add throughput, they only delay the segment under the playhead.

### Source facts
- Steam's video is HEVC **Main, 8-bit, full range (`yuvj420p`)**, ~60 fps. Every path converts to `yuv420p`/`nv12`.
- Each chunk's fragment carries the time since the recording started: chunk *n* starts at about `(n−1)·3 s` (pruned session: chunk 3418 at 10251.0 s; saved clip: chunk 1 at 8259.0 s). So `v0` = start time of the first chunk on disk is probed once per session (`chunks::probe_start`) and subtracted.

### Preview segment
- Segment *s* covers chunks `first + 4s … first + 4s + 3` (fewer at the end), video time `[12s, 12s+12)` (`segment_plan`, `segment_count`). Missing chunks are left out, so the output has a gap in its timestamps.
- **Inputs:** video `init + chunks` is streamed into ffmpeg's stdin. Audio `init + chunks` (about 50 KB per chunk) is written to `preview/tmp/<pid>-<n>-a.mp4` and deleted afterwards.
- **Command (CUDA):**
  ```
  ffmpeg -hide_banner -nostdin -v error -copyts
         -hwaccel cuda -hwaccel_output_format cuda -i pipe:0 -i <audio>
         -map 0:v:0 -map 1:a:0
         -vf scale_cuda=-2:360:format=yuv420p -c:v h264_nvenc -preset p2 -cq 28
         -g 60 -bf 0 -profile:v high -level 4.0
         -c:a copy
         -output_ts_offset -<v0>
         -avoid_negative_ts disabled
         -movflags +frag_keyframe+empty_moov+default_base_moof+frag_discont
         -f mp4 -y <out>.mp4.part
  ```
  - `-copyts` + `-output_ts_offset -v0` give video time **and** keep the audio's small lead-in. Without `-copyts`, ffmpeg shifts each input to 0 on its own and loses that offset.
  - `+frag_discont` + `-avoid_negative_ts disabled` are required. Without them every fragment started at 0 again.
  - **`-c:a copy` instead of re-encoding the audio.** AAC re-encoding added 1024 priming samples (21 ms) at every segment start. The original AAC chunks line up exactly (segment 0 ends at 12.0059 s, segment 1 starts at 12.0059 s).
  - `-profile:v high -level 4.0` fixes the codec string to `avc1.640028` (`preview::MIME`). Level 3.1 is too low for 1280×360 at 60 fps.
  - `.part` + rename, so a cancelled job never leaves a broken segment in the cache.
- **Backends** (`transcode::Backend`, filter and encoder per backend in `encode_args`):
  | Backend | Decode | Scale | Encode |
  |---|---|---|---|
  | `cuda` | `-hwaccel cuda` (GPU frames) | `scale_cuda=-2:360:format=yuv420p` | `h264_nvenc -preset p2 -cq 28` |
  | `d3d11vaAmf` (Windows) | `-hwaccel d3d11va` | `scale=-2:360,format=yuv420p` | `h264_amf -quality speed -b:v 2M` |
  | `d3d11vaQsv` (Windows) | `-hwaccel d3d11va` | `scale=-2:360,format=nv12` | `h264_qsv -preset veryfast -global_quality 28` |
  | `vaapi` (Linux) | `-hwaccel vaapi` (GPU frames) | `scale_vaapi=w=-2:h=360:format=nv12` | `h264_vaapi -qp 26` |
  | `qsv` | `-hwaccel qsv` | `scale=-2:360,format=nv12` | `h264_qsv …` |
  | `software` | CPU | `scale=-2:360,format=yuv420p` | `libx264 -preset veryfast -crf 26` |

  Only `cuda` and `software` are tested. The others are best effort.
- **Choosing a backend** (`transcode::detect`): candidates whose hwaccel and encoder are listed by `ffmpeg -hwaccels`/`-encoders`, in the order Windows: cuda → d3d11va+amf → d3d11va+qsv → software; Linux: cuda → vaapi → qsv → software. Each one gets a 1-chunk test transcode (30 s timeout), and the first that works is stored in the settings store (`previewBackendDetected`). The user can override it in Settings (`previewBackend`). Choosing "Automatic" clears the detected value, so detection runs again the next time a preview opens.
- **Runtime fallback:** a failed hardware transcode (for example NVENC `OpenEncodeSessionEx failed`) is retried in software. After 2 hardware failures in a row the session switches to software.
- **Warm-up:** opening a preview (`get_preview_info`) queues segments 0, 1, 2. That also warms up CUDA.

### Cache (`preview/cache.rs`)
- `app_cache_dir()/steamreplay/preview/<sha1(id)>-<first>/<s>.mp4`. The last segment, if it has fewer than 4 chunks, is named `<s>_<lastchunk>.mp4` so it is redone when a live session grows.
- LRU by total size: the mtime is the last use (cache hits touch the file). After every new segment, the oldest files are removed until the total fits the limit (settings `previewCacheMb`, default 2048, minimum 100). Empty folders are removed. `.part` files are never counted.
- Opening a session removes its folders with another `first` (pruned sessions). `preview/tmp/` is emptied at startup.
- Segments are 1–2.6 MB each (CUDA, `-cq 28`).

### Queue (`preview/mod.rs`)
- One `PreviewSession` per open session, with one worker task. The queue is ordered: explicitly requested segments first (from the protocol), then the prefetch order, then older prefetch.
- `set_playhead(t)` (command `preview_playhead`): prefetch order is `s, s+1, s+2, s−1` (hardware) or `s, s+1` (software), leaving out cached segments. Queued segments more than 3 away from the playhead are dropped (their waiters get `Cancelled` → HTTP 503), and running ones are cancelled.
- Closing a session (`preview_close`, when the session changes) cancels its token, which kills its ffmpeg. On app exit (`RunEvent::Exit`), `PreviewState::shutdown` cancels everything and waits up to 2 s until the jobs are gone. This matters on Windows, where child processes outlive the parent.

### URI protocol (`preview/protocol.rs`)
- `register_asynchronous_uri_scheme_protocol("replay", …)`. `replay://localhost/preview/<urlencoded id>/<s>.mp4` returns the segment of an **open** preview session, transcoding it first when needed.
- Status: 200 with the bytes; 206 for `Range` requests; 404 for an unknown session or a segment with no chunks; 503 when cancelled; 500 with ffmpeg's error.
- Headers: `Content-Type: video/mp4; codecs="avc1.640028, mp4a.40.2"`, `Access-Control-Allow-Origin: *` (the page's origin is never `replay://`), `Accept-Ranges: bytes`.
- Frontend URL: `src/lib/replay.ts` takes its base from `convertFileSrc("", "replay")`, which is `replay://localhost/` on Linux and `http://replay.localhost/` on Windows.

### Commands
```rust
get_preview_info(id) -> PreviewInfo { segment_s, count, mime, fps /* 60 */, backend }  // opens the session
preview_playhead(id, t)
preview_close(id)
get_preview_settings() -> PreviewSettings { backend: Option<Backend>, detected, cache_mb, cache_bytes }
set_preview_settings(backend: Option<Backend>, cache_mb)
clear_preview_cache()
```
`get_preview_info` is a typed command, not a `replay://preview-info` URL, so the frontend gets generated types.

### Frontend
- **`src/lib/msePlayer.ts`:** one `<video>`, one `MediaSource` and one `SourceBuffer` (`mode = "segments"`), with a serial operation chain and a generation counter that stops stale async work.
  - `seek(t)`: if `t` is buffered, set `currentTime`. If the segment comes right after the run, append it. Otherwise start a new run in a fresh MediaSource (see the quirks). It also calls `preview_playhead`.
  - On `timeupdate`: append the next segment when less than 8 s is buffered ahead, and remove data more than 60 s behind the playhead. On `QuotaExceededError`, remove data behind the playhead and retry.
  - On `waiting`: jump gaps shorter than 3 s. A 404 segment (no chunks) is skipped.
  - After the last segment, `endOfStream()`.
- **`src/components/Player.tsx`** (above the timeline in `WaveformPanel`): a video with `object-fit: contain` and a height of `min(360px, 42vh)`, a control row (play/pause, timecode, rate, key hint, backend), and overlays for "Preparing preview…", "Loading…" and errors.
- **Sync (`state/view.ts`):**
  - `seek(t)` means the user wants to jump: it bumps `seekSeq`, and the Player follows it. `setPlayhead(t)` is used by the video and never seeks.
  - The timeline click and `SpikeList.goTo` use `seek`.
  - While playing, a `requestAnimationFrame` loop updates the playhead at most 30 times per second. Not `requestVideoFrameCallback`: WebKitGTK has it, but it never fires for MSE video, so the playhead only moved on pause. The loop also calls `preview_playhead` whenever the segment changes. While a seek is still loading (`MsePlayer.seeking`), the loop does not write video time into the playhead: the video still shows the old position, or is empty at 0 s after a reset, and writing that back undid clicks made during playback.
  - **Follow mode:** while playing, the view pages so the playhead sits at 10 % once it passes 90 %. Panning (drag, horizontal or Shift wheel, the overview) turns it off. A seek or pressing play turns it back on.
- **Keys:**
  - Space = play/pause,
  - J/K/L: J = simulated reverse at 2×, then 4× and 8× (MSE has no negative `playbackRate`; it steps `currentTime` back every 100 ms), K = pause, L = play, and repeated L steps the rate 1× → 2× → 4×,
  - ←/→ = ±1 s, Shift+←/→ = ±5 s, `,`/`.` = ±1 frame at 60 fps.
  - `isTyping` moved to `src/lib/keys.ts`.
- **Settings:** a "Video preview" section with the backend (Automatic, showing which one was detected, or one of the backends), the cache limit, the space used and "Clear cache".

## 5. Edge cases
- **Pruned session:** segments are numbered from `first`, and the cache folder includes `first`, so old segments are never reused and their folder is removed on open. Checked: `bg_1623730_20261010_164846` (chunk 3418 starts at 10251.0 s, which maps to video time 0).
- **Live session:** a partial last segment is cached under a name with its last chunk. Refreshing the info when the inventory changes is not planned (M7 was dropped).
- **Gap chunks:** left out of the segment, so the timestamps have a gap. The player jumps gaps < 3 s, and a segment with no chunks at all gives 404 and is skipped.
- **NVENC session limits / failures:** one job at a time per session, a software retry, and a switch to software after 2 failures in a row.
- **Different source resolutions:** `-2:360` keeps the aspect ratio (5120×1440 → 1280×360, 16:9 → 640×360), and the player uses `object-fit: contain`.
- **Session switch:** the Player's cleanup destroys the MsePlayer and calls `preview_close`, which kills its ffmpeg.

## 6. Acceptance criteria
- [x] **Step 0:** MSE plays appended fMP4 H.264 segments in WebKitGTK on Linux (NVIDIA). See §4.
- [ ] Step 0 on Windows (WebView2), on a teammate's machine.
- [~] Clicking a spike starts playback in < 1.5 s (cold) and < 300 ms (cached). Measured: cold 1.36–1.49 s, with one at 1.69 s; cached 28–76 ms.
- [ ] 5 minutes of continuous playback with no stalls in hardware mode (not yet run in the app; segments are produced at about 9× realtime). In software mode, stalls only while seeking.
- [ ] The playhead matches the video within 1 frame (clap check: the waveform peak and the visual moment match).
- [x] The cache stays under the configured size, oldest first (unit test `evicts_oldest_first`; check in the app with a 100 MB limit).
- [ ] No orphaned ffmpeg after closing the session or the app (`pgrep ffmpeg`).
- [x] clippy, tests, tsc and lint are clean.

## 7. Verification steps
- **Unit tests:**
  - `preview::tests`: segments from `first ≠ 1`, a partial last segment, gaps, prefetch order and staleness;
  - `cache::tests`: LRU eviction, `.part` files ignored, stale sessions;
  - `transcode::tests::args_per_backend`;
  - `protocol::tests`: paths and ranges.
- **Integration test** (`#[ignore]`):
  `STEAMREPLAY_SESSION_DIR=<…/clips/clip_1478500_…/video/bg_…> cargo test preview_fixture -- --ignored --nocapture`
  Transcodes segment 0 with software (and CUDA when available), shifted by 0 and by 12 s. Checks h264 High, level 40, 360 p, start ≈ 0 / 12, ≈ 6 s covered, and an AAC track. It uses an env var because a 2-chunk video fixture would be about 16 MB.
- **Manual** (`pnpm tauri dev`, `bg_1445790_20261015_210120`):
  - seek to random positions and all spikes;
  - play across segment boundaries and listen for clicks;
  - jump far while playing (it must keep playing);
  - play 5 minutes;
  - clap check;
  - the pruned session `bg_1623730_20261010_164846`;
  - set the cache limit to 100 MB and watch it shrink;
  - switch sessions and quit, then `pgrep ffmpeg` must return nothing;
  - `nvidia-smi` while Steam records.
- **Windows** (teammate, CI build): WebView2 MSE playback, the detected backend (must be `cuda` on NVIDIA), GPU vendor, driver version and cold-seek time. Record the results here.

## 8. Open questions / risks
- **WebView2** has not been tested yet. Chromium MSE is expected to work, and the `http://replay.localhost/` URL scheme comes from `convertFileSrc`.
- **The `Failed to push buffer` error seen once in WebKitGTK.** If it shows up again, check which GStreamer decoder WebKit picks (`GST_DEBUG=2`); `nvh264dec`, `avdec_h264` and `vulkanh264dec` all decode the segments as plain files.
- **AMD/Intel backends are untested.** Without such hardware, a teammate reporting the detected backend is the only check.
- **The cold seek is right at the 1.5 s limit.** Decode is the bottleneck. Options if needed: 2-chunk first segments, or NVDEC's built-in resize (`-c:v hevc_cuvid -resize`).
