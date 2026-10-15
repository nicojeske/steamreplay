# M4: Video preview synced with the timeline

## 1. Goal and user-visible outcome
A video player above the timeline shows the recording at the playhead. You can play, pause and scrub, and clicking a spike or marker jumps the player there. While playing, the timeline playhead moves along. The preview is low-res (360 p high) but smooth, and playback starts in under 1.5 s after a seek.

## 2. Prerequisites
- M1–M3 done: sessions, timeline with playhead and selection, ffmpeg runner, job handling.
- `00-overview.md`: stack decision ("Why the preview needs transcoding") and DASH facts.

## 3. Scope / non-goals
**In scope:** `media/preview.rs` (transcode, cache, prefetch), the `replay://` URI protocol, the `Player` component (MediaSource), playhead sync, playback keys.

**Non-goals:**
- full-quality preview,
- playing the original HEVC in the webview (not reliable, see the risks),
- audio-only scrubbing sounds.

## 4. Design

### Measurements (dev machine, RTX 4080, 5120×1440 HEVC source, 1280×360 H.264 output)
| Method | 3 s (1 chunk) | 60 s (20 chunks) | CPU |
|---|---|---|---|
| Software decode + libx264 veryfast | 1.16 s | 16.8 s (3.6× realtime) | ~10 cores busy |
| NVDEC + scale_cuda + h264_nvenc | 5.2 s (first run, CUDA warm-up) | **5.6 s (10.7× realtime)** | < 1 core |

Conclusions:
- Use hardware decoding when it is available.
- Make **preview segments of 4 chunks (12 s)** per ffmpeg process. That spreads the start-up cost, and the first segment is still ready fast.
- Prefetch ahead of the playhead.
- Software fallback works but uses a lot of CPU. In that mode, prefetch only one segment ahead.

### Preview segment
- Segment *s* covers chunks `first + 4s … first + 4s + 3` (fewer at the end), video time `[12s, 12s+12)`.
- **Command, hardware path (NVIDIA):**
  ```
  ffmpeg -hide_banner -nostdin -v error
         -hwaccel cuda -hwaccel_output_format cuda -i v.mp4 -i a.mp4
         -map 0:v:0 -map 1:a:0
         -vf scale_cuda=-2:360 -c:v h264_nvenc -preset p2 -cq 28 -g 60 -bf 0
         -c:a aac -b:a 128k -ac 2
         -output_ts_offset <12*s>
         -f mp4 -movflags +frag_keyframe+empty_moov+default_base_moof pipe:1
  ```
  `v.mp4` and `a.mp4` are the joined `init + chunks` bytes, written to a temp file or fed through stdin pipes, the same way as in M3.
- **Other GPU paths:**
  - **Windows (main target, mostly NVIDIA):** the same CUDA command as above (`-hwaccel cuda … scale_cuda … h264_nvenc`); the BtbN build includes it. Best effort for AMD/Intel: `-hwaccel d3d11va` decode with CPU `scale`, encoding with `h264_amf` / `h264_qsv`. These are probed but not tested on real hardware unless a teammate has one.
  - **Linux:** VAAPI (`-hwaccel vaapi … scale_vaapi … h264_vaapi`) for AMD/Intel.
- **Fallback:** software (`scale=-2:360`, `libx264 -preset veryfast -crf 26`).
- **Choosing a path:** on first use, run a 1-chunk probe transcode for each candidate in order (Windows: cuda → d3d11va+amf/qsv → software; Linux: cuda → vaapi → qsv → software). Store the first one that works in settings (`preview_backend`). It can be overridden in settings.
- `-output_ts_offset` makes the fragments carry real video time, so the frontend can append them to one MSE `SourceBuffer` in `segments` mode with no offset math.
- **Cache:** `app_cache_dir()/steamreplay/preview/<sha1(id)>-<first>/<s>.mp4`, LRU by total size (default 2 GB, set in settings). Each segment is about 1–1.5 MB.
- **Queue:** one worker per session, with jobs ordered by priority (the segment under the playhead first, then +1, +2, then −1). Cancel queued jobs that fall more than 3 segments away from a new playhead. At most 1 ffmpeg process at a time in software mode, 2 in hardware mode.

### URI protocol
Register `replay` with `tauri::Builder::register_asynchronous_uri_scheme_protocol`:
- `replay://localhost/preview/<urlencoded id>/<s>.mp4`: if the segment is cached, return it. If not, put it at the front of the queue, wait for it and return it. Header `Content-Type: video/mp4; codecs="avc1.64001f, mp4a.40.2"` (read the real codec string from the first segment, or fix the profile with `-profile:v high -level 3.1`).
- `replay://localhost/preview-info/<id>`: JSON `{ segment_s: 12, count, codec }`.
- On Windows, the webview reaches custom schemes as `http://replay.localhost/…`. Use Tauri's `convertFileSrc`-style helper to build URLs for each platform.

### Frontend `Player`
- `<video>` + `MediaSource` + one `SourceBuffer` (`mode = "segments"`).
- **Seek to `t`:**
  1. `s = floor(t/12)`.
  2. If `[t, t+1]` is not in `video.buffered`, abort pending appends, fetch segment `s` and append it.
  3. Set `currentTime = t`.
  4. Fetch `s+1` ahead.
- **During playback:** when less than 8 s is buffered ahead, fetch the next segment. Remove buffered ranges more than 60 s behind the playhead to limit memory.
- **Sync:**
  - `timeupdate`/`requestVideoFrameCallback` → `playheadS` in the store (throttled to 30 Hz).
  - When the user clicks the timeline → `seek`.
  - The timeline auto-scrolls during playback (follow mode, which turns off when the user pans).
- **Keys:**
  - Space = play/pause,
  - J/K/L = −2× / pause / play, with repeated L stepping 1×, 2×, 4× (`playbackRate`),
  - ←/→ = ±1 s, Shift+←/→ = ±5 s, `,`/`.` = ±1 frame at the preview fps.
- **Loop selection** (toggle `Ctrl+L`): when there is an in/out selection, playback loops inside it. This helps with fine-tuning before export.

### Fallback if MSE fails in WebKitGTK
Load each segment as its own `<video src="replay://…/<s>.mp4">`. Use two video elements and swap at segment ends (ping-pong). Cuts at segment boundaries may stutter a little but it works. Build the MSE path first. If the step-0 check below fails, use this instead.

## 5. Edge cases
- Pruned session: segments are numbered from `first`, so the cache key includes `first`. Pruning makes old segment indexes invalid, so throw away that session's preview cache.
- Live session: segments past `last` do not exist yet. Treat the end as the last full segment, and refresh the info when the inventory changes (M5).
- Gap chunks inside a segment: transcode only the parts that exist and leave a gap in the timestamps. MSE tolerates gaps, and the player jumps a gap of less than 3 s with `currentTime += …` when it stalls.
- NVENC session limits on consumer GPUs (5–8 concurrent, but recording itself uses one): keep at most 2 concurrent preview encodes and fall back to software on `OpenEncodeSessionEx failed`.
- Steam recording while the tool previews: GPU load is shared. That is fine, but measure it and document it.
- Different source resolutions (16:9 vs 32:9): `scale=-2:360` keeps the aspect ratio, and the player uses `object-fit: contain`.

## 6. Acceptance criteria
- [ ] **Step 0 (do first, write down the result):** a minimal page in the Tauri webview plays two appended fMP4 H.264 segments through MSE on Windows (WebView2, expected to work) and on Linux (WebKitGTK, NVIDIA). If Linux fails, use the fallback above on Linux only.
- [ ] On the dev machine, clicking a spike starts playback in < 1.5 s (cold cache) and < 300 ms (cached).
- [ ] 5 minutes of continuous playback with no stalls in hardware mode. In software mode, stalls only while seeking.
- [ ] The playhead in the timeline matches the video within 1 frame at the preview fps (checked with a clap: the waveform peak and the visual moment match).
- [ ] The cache stays under the configured size, and the oldest segments are removed first.
- [ ] No orphaned ffmpeg processes after closing the session or the app.
- [ ] clippy, tests, tsc and lint are clean.

## 7. Verification steps
- **Unit tests:** mapping segments to chunks (with `first ≠ 1`, last partial segment, gaps), queue priority and cancel logic, LRU eviction.
- **Integration test** (`#[ignore]`): transcode segment 0 of the 2-chunk clip fixture with software encoding → ffprobe shows h264 360 p, start time 0, duration ≈ 6 s. Run it again with `-output_ts_offset 12` → start time 12.
- **Manual:** seek to 10 random positions and to all spikes in `bg_1445790_20261015_210120`. Play across segment boundaries and watch for audio clicks or video jumps. Check pruned session `bg_1623730_20261010_164846`.
- **Windows** (this milestone): on a teammate's machine, check WebView2 MSE playback, the chosen backend (record it, along with the GPU vendor) and the cold-seek time. The NVIDIA machine must use the cuda backend. Also record the driver version, because NVENC needs a recent driver for the ffmpeg 7 build.

## 8. Open questions / risks
- **MSE in WebKitGTK** with NVIDIA and the GStreamer backend: the biggest risk, hence step 0. Possibly needs `gst-plugins-bad`, `gst-libav` or the `nvcodec` plugins (present on the dev machine). Document the required packages.
- **The first CUDA start took about 5 s.** Warm up the hardware path in the background when a session opens: run a 1-chunk transcode of segment 0.
- Audio at segment boundaries: each segment has its own AAC encode, so tiny clicks or gaps may happen. If they are noticeable, copy the original AAC instead (`-c:a copy`). Its 3 s chunks line up with the video chunks.
