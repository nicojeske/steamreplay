# M2: Waveform, markers and loud-part detection

## 1. Goal and user-visible outcome
Open a session and see a zoomable timeline:
- an audio waveform across the full width,
- Steam markers drawn as flags,
- highlighted **loud regions** (laughs, shouts), also listed in a ranked sidebar.

Clicking a spike or marker moves the playhead there and zooms in. A 1 h session is analysed in a few seconds and opens instantly the second time (cache).

## 2. Prerequisites
- M1 done: session list, `SessionDetail`, chunk inventory and markers in video time.
- `00-overview.md`: DASH facts (one mixed audio track, absolute mpd path), cross-cutting rules (cache keys, long jobs, video time).
- `ffmpeg` on PATH (Linux dev machine: `/usr/bin/ffmpeg`).

## 3. Scope / non-goals
**In scope:** `media/ffmpeg.rs`, `media/audio.rs`, `media/pyramid.rs`, `media/spikes.rs`, the canvas `Timeline` component, `SpikeList`, playhead state (no playback yet), job progress and cancellation.

**Non-goals:**
- video preview (M4), export (M3),
- spike parameter UI (M5): use defaults here, with constants in one place,
- live updates (M5),
- separating mic from game audio (Steam mixes them).

## 4. Design

### `media/ffmpeg.rs`
- `fn locate(override_dir) -> Result<FfmpegPaths>`: order is settings override (store key `ffmpegDir`, set in the Settings dialog) → bundled sidecar next to the executable → `which ffmpeg` / `which ffprobe`. The result is cached in app state.
- **Windows sidecar (this milestone):**
  - `scripts/fetch-ffmpeg.ts` (`pnpm fetch-ffmpeg`) downloads a pinned GPL static build (BtbN `autobuild-2026-07-31-14-10`, `ffmpeg-n7.1.5-…-win64-gpl-7.1.zip`, sha256 checked) and places it as `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` (plus ffprobe). BtbN keeps the monthly builds; if the URL 404s, update the three constants at the top of the script.
  - `bundle.externalBin` lives in `src-tauri/tauri.windows.conf.json`, so Linux builds do not need the binaries. At runtime the sidecar is resolved next to the app executable.
  - tauri-build checks that the sidecars exist, so CI runs the script (cached) **before clippy** on Windows. The binaries are not committed (`.gitignore`).
  - `resources/ffmpeg/NOTICE.txt` (committed) and `LICENSE.txt` (fetched) are bundled as resources.
- On Windows, start processes with the `CREATE_NO_WINDOW` creation flag (`std::os::windows::process::CommandExt`), so no console window flashes up. Check with `ffmpeg -hide_banner -version` and keep the version string.
- `ffmpeg_command(paths, progress)` builds a `tokio::process::Command` with `-hide_banner -nostdin -v error` and `kill_on_drop`. `async fn run(cmd, stdin, progress, cancel) -> Result<Vec<u8>>` runs it to completion. `wait_or_kill` kills **and reaps** the process when the `CancellationToken` fires. Stderr is collected for error messages.
- Progress: with `progress = true` the command gets `-progress pipe:2 -stats_period 0.5`; `out_time_us=` lines become a fraction of the known duration (used from M3 on). Audio extraction reports bytes fed / total bytes instead.

### `media/audio.rs`: extraction
**Do not give ffmpeg `session.mpd`.** Measured in M2: the DASH demuxer returned only 3.0 s of the 6 s saved clip. Instead, for each gap-free run of chunks (`plan_runs`: video chunk in the inventory **and** `chunk-stream1-N.m4s` on disk), stream `init-stream1.m4s` + the run's audio chunks into ffmpeg's stdin:
```
ffmpeg -hide_banner -nostdin -v error -i pipe:0 -map 0:a:0 -vn -ac 1 -ar 8000 -f f32le pipe:1
```
This handles gaps, pruning after the manifest was written, and clips the same way, and avoids the Windows command-line limit of a long `concat:` list. On the 1 h sample it gives the same samples as the mpd path and is slightly faster (1.8 s vs 2.1 s).
- Read stdout in blocks. Each **bin = 80 samples = 10 ms**. For each bin store `min: f32`, `max: f32`, `rms: f32`. A 1 h session is 372 k bins, about 4.5 MB.
- When the samples run out, the last bin may be partial. Its duration is still 10 ms.
- **Start offset:** the audio track starts 0.357917 s after the video in the sample session, and raw f32le output drops that. Each run is placed at bin `round((audio_start_run − video_start_first) / 10 ms)`, with both start times probed by ffprobe on `init + first chunk` piped through stdin. If a probe fails, the nominal position `(n − first) · 3 s` is used. Verified: the 1 h sample starts with 36 silent bins.
- The output always has `(last − first + 1) · 300` bins; gaps and padding are zero (silent) bins.

### Pyramid
- Level 0 = the 10 ms bins. Level *k* merges 4 bins of level *k−1* (min of mins, max of maxes, RMS = sqrt(mean of squares)). Stop when a level has fewer than 2000 bins.
- The frontend chooses the level where 1 bin is about 1 pixel or more.

### Cache
- Path: `app_cache_dir()/steamreplay/waveform/<sha1(session id)>-<first>-<last>.bin`.
- Format: header `b"SRWF"`, `version: u16 = 1`, `bin_ms: u16 = 10`, `bins: u32`, then level 0 as `[f32; 3] × bins` (little-endian). Rebuild the higher levels on load (fast). Reading uses mmap or a single read.
- If the session's `first`/`last` changed (live or pruned), the old file does not match. Delete stale files for the same session id.
- Write to `.tmp` and rename, so a cancelled or failed write leaves no partial file. Gaps are not stored; they are recomputed from the inventory on load.
- The pyramid and cache live in `media/pyramid.rs`.

### `media/spikes.rs`
Input: level-0 RMS. Algorithm (all constants in `SpikeParams`, defaults below):
1. `db[i] = 20·log10(max(rms[i], 1e-5))`.
2. Short-term loudness `st[i]` = mean of `db` over a centred window of `short_ms = 250` (25 bins).
3. Baseline `base[i]` = rolling **median** of `st` over `baseline_s = 30` s (3000 bins). Use a two-heap or histogram (0.5 dB buckets, −100..0 dB) sliding median, so it is O(n).
4. `excess[i] = st[i] − base[i]`. A region is a run where `excess > threshold_db = 10` that lasts at least `min_len_ms = 400`.
5. Merge regions less than `merge_gap_s = 5` s apart. Add `pad_s = 1.5` s to each end, clamped to the session.
6. Score = `peak_excess_db + 3·log2(1 + active_len_s)`. Sort by score, keep the top `max_results = 200`.
7. Ignore digital silence: bins with `rms < 1e-4` do not count towards the baseline.

```rust
pub struct SpikeParams { short_ms: u32, baseline_s: f32, threshold_db: f32, min_len_ms: u32, merge_gap_s: f32, pad_s: f32, max_results: u32 }
pub struct Spike { start_s: f64, end_s: f64, peak_s: f64, peak_excess_db: f32, score: f32 }
```

### Commands
```rust
#[tauri::command] async fn analyze_audio(id: String, job_id: String, progress: Channel<Progress>) -> Result<WaveformMeta, AppError>; // cached → immediate
#[tauri::command] async fn get_waveform(id: String, level: u8, start_bin: u32, count: u32) -> Result<Samples, AppError>; // flat [min,max,rms]*
#[tauri::command] async fn get_spikes(id: String, params: Option<SpikeParams>) -> Result<Vec<Spike>, AppError>;
#[tauri::command] async fn cancel_job(job_id: String) -> Result<(), AppError>;
#[tauri::command] async fn get_ffmpeg_info() -> Result<FfmpegPaths, AppError>;
#[tauri::command] async fn set_ffmpeg_dir(path: Option<PathBuf>) -> Result<FfmpegPaths, AppError>;
pub struct WaveformMeta { bin_ms: u16, levels: Vec<LevelInfo /* bins, bin_ms */>, duration_s: f64, gaps: Vec<Gap> }
pub struct Progress { fraction: f64 }
```
- The frontend chooses `job_id` (a UUID), so it can cancel while `analyze_audio` runs.
- A failed decode (not a cancel) is retried once with a fresh inventory.
- `AppError` gains `Ffmpeg(String)` and `Cancelled`.
- f32/f64 fields carry `#[specta(type = Number)]`; otherwise specta types them `number | null` (NaN). `Samples` is a transparent `Vec<f32>` wrapper for the same reason.
Keep the loaded pyramid in a `tauri::State<Mutex<LruCache<SessionId, Arc<Pyramid>>>>` with capacity 4. For large ranges, `get_waveform` can return binary through `tauri::ipc::Response` to avoid JSON number overhead. Use this if JSON is measurably slow.

### Frontend
- **`Timeline` (canvas, devicePixelRatio aware):**
  - waveform drawn as min/max bars,
  - RMS as a darker inner fill,
  - spike regions as translucent bands with a rank label,
  - markers as vertical flags with a tooltip,
  - playhead line,
  - time ruler (`hh:mm:ss`, tick spacing adapts to zoom).
- **Interaction:** wheel = zoom around the cursor, drag or shift+wheel = pan, click = set playhead, double-click on a spike = zoom to it.
- **Overview strip:** a full-length mini waveform at the top with a viewport rectangle you can drag.
- **Fetching:** ask only for the visible bins at the chosen level, and cache tiles of 4096 bins per level in a `Map`.
- **`SpikeList`:** rank, `hh:mm:ss`, length, score. Click → playhead to `start_s`, zoom to the region. Keys `N`/`P` go to the next or previous spike by rank. `Shift+N`/`Shift+P` go in time order.
- **Progress:** bar shown while `analyze_audio` runs, with a Cancel button.
- **State** (zustand): `playheadS`, `viewStartS`, `viewEndS`, `selectedSpike`.

## 5. Edge cases
- Pruned or live session: the cache key changes. Analysis of a live session covers only the chunks that exist when the job starts.
- Missing chunks in the middle (video or audio): each gap-free run is decoded separately and the gaps become silent bins. Gaps are drawn hatched.
- Chunks deleted by Steam during analysis: ffmpeg errors near the start. Retry once with a fresh inventory.
- A very loud game with a constant high baseline: the median baseline handles it. Mic peaking (clipping) is fine.
- Sessions with digital silence (game muted): there are no spikes. Show an empty state: "No loud moments found."

## 6. Acceptance criteria
- [ ] First analysis of `bg_1445790_20261015_210120` (1 h) takes < 6 s on the dev machine. A reopen takes < 300 ms.
- [ ] Zooming from the full session down to about 2 s visible stays smooth (no frame > 50 ms on the dev machine).
- [ ] User markers show at their positions. In the sample session 2 of the 5 user markers are inside the video and visible; the other 3 come after the `RecordingFailed` entry (video ends at 3720 s) and are counted as outside.
- [ ] The top 10 spikes of the sample session are mostly real laughs or shouts when checked by ear with `mpv --start=<t> session.mpd`. Write the result down in the PR or commit notes.
- [ ] Cancelling a running analysis kills ffmpeg (no zombie process) and leaves no partial cache file.
- [ ] On a teammate's Windows machine, the CI installer works with the bundled ffmpeg and no ffmpeg on PATH. A 1 h session is analysed, spikes show up, and no console window flashes.
- [ ] clippy, tests, tsc and lint are clean.

## 7. Verification steps
- **Unit tests:**
  - pyramid reduction (known input → expected min/max/RMS),
  - cache round trip and version mismatch → rebuild,
  - spike detection on synthetic signals: noise at −40 dB with 1 s bursts at −20 dB at known times → spikes at those times ±pad; bursts 3 s apart are merged; a 200 ms burst is ignored,
  - sliding median against a naive implementation on random data.
- **Integration test** (`#[ignore]`, needs ffmpeg): analyse the 2-chunk clip fixture (`tests/fixtures/clip_1478500_audio/`, audio only) and check the bin count = 600 with audio up to the end.
- **Real session** (`#[ignore]`): `STEAMREPLAY_SESSION_DIR=<bg folder> cargo test --release real_session -- --ignored --nocapture` prints timings and the top 15 spikes for listening. M2 result on the 1 h sample: extract 2.0 s, cache reload ~2 ms, spikes 26 ms.
- **Manual:**
  - Check the timeline offset formula. Play `mpv --start=<marker video_t> <abs>/session.mpd` for 2–3 user markers and check what happens there. Better: record a new short session, press the marker key right after a hand clap, and confirm the marker flag sits on the clap spike within 1 s.
  - Check audio and video alignment (bin 0 = video 0) by looking at a clap in the waveform and in mpv.

## 8. Open questions / risks
- The marker offset formula (see the overview) is unconfirmed. Fix it here if the clap test shows a constant offset.
- The defaults (10 dB, 30 s baseline) are guesses. Tune them on 2–3 real sessions and record the chosen values in the code comments. With the defaults, the 1 h sample hits the 200-result cap and spikes peak +30–39 dB above the baseline, so the defaults look too loose.
- The audio start offset (0.358 s) is handled by probing start times (see `media/audio.rs`). Confirm with the clap test.
