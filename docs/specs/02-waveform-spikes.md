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
**In scope:** `media/ffmpeg.rs`, `media/audio.rs`, `media/spikes.rs`, the canvas `Timeline` component, `SpikeList`, playhead state (no playback yet), job progress and cancellation.

**Non-goals:**
- video preview (M4), export (M3),
- spike parameter UI (M5): use defaults here, with constants in one place,
- live updates (M5),
- separating mic from game audio (Steam mixes them).

## 4. Design

### `media/ffmpeg.rs`
- `fn locate() -> Result<FfmpegPaths>`: order is settings override → bundled sidecar → `which ffmpeg` / `which ffprobe`.
- **Windows sidecar (this milestone):**
  - A `scripts/fetch-ffmpeg.ts` script downloads a pinned GPL static build (BtbN `ffmpeg-n7.x-win64-gpl`, with a checksum check) and places it as `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe` (plus ffprobe).
  - Configure it under `bundle.externalBin`, and resolve it at runtime next to the app executable.
  - CI runs the script before `tauri build`. Do not commit the binaries; add them to `.gitignore`.
  - Put a NOTICE and the ffmpeg license text in the bundle.
- On Windows, start processes with the `CREATE_NO_WINDOW` creation flag (`std::os::windows::process::CommandExt`), so no console window flashes up. Check with `ffmpeg -hide_banner -version` and keep the version string.
- `async fn run(args, on_progress, cancel: CancellationToken) -> Result<Output>` built on `tokio::process::Command`. Always pass `-hide_banner -nostdin -v error`. Collect stderr for error messages. When cancelled, kill the process.
- Progress: add `-progress pipe:2 -stats_period 0.5`, or read stdout when it is not used for data. Parse `out_time_us=` lines into a fraction of the known duration.

### `media/audio.rs`: extraction
Command (mpd path **absolute**):
```
ffmpeg -hide_banner -nostdin -v error -i <abs>/session.mpd -map 0:a:0 -vn -ac 1 -ar 8000 -f f32le pipe:1
```
- Read stdout in blocks. Each **bin = 80 samples = 10 ms**. For each bin store `min: f32`, `max: f32`, `rms: f32`. A 1 h session is 360 k bins, about 4.3 MB.
- When the samples run out, the last bin may be partial. Its duration is still 10 ms.
- `audio_start_s`: AAC has a start offset (ffprobe showed `start 0.357917` for the audio stream). Use ffmpeg's output with no extra flags, and check in verification that bin 0 matches video time 0 within one bin. If it does not, add `-af aresample=async=1` or shift by the start offset.

### Pyramid
- Level 0 = the 10 ms bins. Level *k* merges 4 bins of level *k−1* (min of mins, max of maxes, RMS = sqrt(mean of squares)). Stop when a level has fewer than 2000 bins.
- The frontend chooses the level where 1 bin is about 1 pixel or more.

### Cache
- Path: `app_cache_dir()/steamreplay/waveform/<sha1(session id)>-<first>-<last>.bin`.
- Format: header `b"SRWF"`, `version: u16 = 1`, `bin_ms: u16 = 10`, `bins: u32`, then level 0 as `[f32; 3] × bins` (little-endian). Rebuild the higher levels on load (fast). Reading uses mmap or a single read.
- If the session's `first`/`last` changed (live or pruned), the old file does not match. Delete stale files for the same session id.

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
#[tauri::command] async fn analyze_audio(id: String, progress: Channel<Progress>) -> Result<WaveformMeta, AppError>; // cached → immediate
#[tauri::command] async fn get_waveform(id: String, level: u8, start_bin: u32, count: u32) -> Result<Vec<f32>, AppError>; // flat [min,max,rms]*
#[tauri::command] async fn get_spikes(id: String, params: Option<SpikeParams>) -> Result<Vec<Spike>, AppError>;
#[tauri::command] async fn cancel_job(job_id: String) -> Result<(), AppError>;
pub struct WaveformMeta { bin_ms: u16, levels: Vec<LevelInfo /* bins, bin_ms */>, duration_s: f64 }
```
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
- Missing chunks in the middle: ffmpeg's DASH demuxer may fail or shorten the timeline. In that case, fall back to giving ffmpeg a generated **concat list** of `init-stream1.m4s` + the existing audio chunks per gap-free run, and insert silent bins for the gaps. Gaps are drawn hatched.
- Chunks deleted by Steam during analysis: ffmpeg errors near the start. Retry once with a fresh inventory.
- A very loud game with a constant high baseline: the median baseline handles it. Mic peaking (clipping) is fine.
- Sessions with digital silence (game muted): there are no spikes. Show an empty state: "No loud moments found."

## 6. Acceptance criteria
- [ ] First analysis of `bg_1445790_20261015_210120` (1 h) takes < 6 s on the dev machine. A reopen takes < 300 ms.
- [ ] Zooming from the full session down to about 2 s visible stays smooth (no frame > 50 ms on the dev machine).
- [ ] User markers show at their positions, and the 5 markers of the sample session are visible.
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
- **Integration test** (`#[ignore]`, needs ffmpeg): analyse the 2-chunk clip fixture and check the bin count ≈ 6 s / 10 ms.
- **Manual:**
  - Check the timeline offset formula. Play `mpv --start=<marker video_t> <abs>/session.mpd` for 2–3 user markers and check what happens there. Better: record a new short session, press the marker key right after a hand clap, and confirm the marker flag sits on the clap spike within 1 s.
  - Check audio and video alignment (bin 0 = video 0) by looking at a clap in the waveform and in mpv.

## 8. Open questions / risks
- The marker offset formula (see the overview) is unconfirmed. Fix it here if the clap test shows a constant offset.
- The defaults (10 dB, 30 s baseline) are guesses. Tune them on 2–3 real sessions and record the chosen values in the code comments.
- The AAC priming offset (0.358 s) may shift the waveform. Decide from the clap test.
