# M5: Precise export, spike tuning, live refresh, polished Windows installer

## 1. Goal and user-visible outcome
The tool is ready for the whole team:
- **Precise export:** frame-accurate cuts, optionally re-encoded to H.264 with downscale and a size limit, so clips can be posted in Discord and similar places.
- **Spike tuning:** sliders for sensitivity, with the timeline updating live. Presets per game are remembered.
- **Live sessions:** the session that is recording right now grows on the timeline without reopening it.
- **Distribution:** CI and the ffmpeg sidecar have existed since M1/M2. This milestone makes the Windows installer pleasant to use: app icon, version shown in the UI, update check, and a signed build if possible. Linux gets an AppImage.

## 2. Prerequisites
- M1–M4 done.
- `00-overview.md`: all of it. M3 (export pipeline and snapshotting) and M4 (choosing the hardware backend) are reused here.

## 3. Scope / non-goals
**In scope:** the four features above, a settings page that brings all settings together, and smaller UX items (recent exports list, keyboard shortcut help overlay `?`).

**Non-goals:**
- cloud upload or sharing links,
- automatic captioning,
- editing several clips into one,
- macOS (Steam Game Recording exists there, but nobody on the team uses it),
- writing markers back into Steam.

## 4. Design

### 4.1 Precise export (`media/export.rs`, mode `Precise`)
Snapshot chunks `n_in..=n_out` as in M3. Then re-encode with an exact trim:
```
ffmpeg -hide_banner -nostdin -v error [hwaccel flags] -i v.mp4 -i a.mp4
       -ss <in_s − (n_in−first)*seg> -to <out_s − (n_in−first)*seg>
       -map 0:v:0 -map 1:a:0
       [-vf scale=<w>:-2 | scale_cuda=… ]  [-r 60|30]
       -c:v <encoder> <rate control> -c:a aac -b:a 160k
       -movflags +faststart -y out.part.mp4
```
Here `-ss`/`-to` are **output options** (placed after `-i`), so the cut is frame-accurate. The input is already limited to the snapped chunks, so decoding is short.

```rust
pub enum ExportMode { Fast, Precise(PreciseOpts) }
pub struct PreciseOpts {
    codec: VideoCodec,          // H264 | Hevc | Av1 (only if the encoder exists)
    max_height: Option<u32>,    // e.g. 1080, 720; keep aspect
    fps: Option<u32>,
    target: Quality,            // Crf(u8) | MaxSizeMb(u32)  → two-pass-free bitrate = size*8/duration − audio
}
```
- Encoder choice reuses M4's backend detection: `h264_nvenc` / `hevc_nvenc` / `av1_nvenc` (RTX 40 has AV1), `*_vaapi`, `*_qsv`, `*_amf`, falling back to `libx264`/`libx265`. Run `ffmpeg -encoders` once and cache the list.
- **Size limit:** `MaxSizeMb` sets `-b:v` and `-maxrate`/`-bufsize`. Preset buttons: "Discord (10 MB)", "Discord Nitro (500 MB)", "Original quality".
- **32:9 sources:** offer a "crop to 16:9 centre" option (`crop=ih*16/9:ih`) in addition to scaling, because ultrawide clips look tiny on phones.
- Progress comes from ffmpeg `out_time_us` over the clip length.

### 4.2 Spike tuning UI
- A panel with sliders for `threshold_db`, `baseline_s`, `min_len_ms` and `merge_gap_s`, plus a "sensitivity" macro slider that maps to the threshold. `get_spikes` is cheap (the pyramid is in memory), so call it debounced at 150 ms and redraw.
- Presets are stored per app id (`tauri-plugin-store`: `spikePresets[appId]`), with a reset to defaults.
- Optional: a **loudness curve lane** under the waveform that shows `st − base` and the threshold line, so tuning is visual.

### 4.3 Live sessions
- Use the `notify` crate to watch the folder of every bg session whose `session.mpd` changed less than 30 s ago (a 2 s debounce is enough).
- When chunks are added:
  - update the inventory,
  - analyse only the **new** audio chunks (concat `init-stream1` + new chunks → same pipeline) and append bins to the pyramid,
  - update the cache file (rewrite the header bin count, append data),
  - send an event `session-updated { id, first, last }`.
- When chunks are deleted from the start (`first` goes up), cut bins from the front. Video time is still measured from the current first chunk, so send `time_shift_s` so the frontend can move the view, playhead and selection back. **Alternative to decide during implementation:** keep video time tied to chunk 1 (absolute) everywhere so nothing has to shift. That is the more robust choice, so check the M1–M4 code to see how hard the change is.
- UI: "LIVE" badge. When follow mode is on, the view stays at the end.

### 4.4 Distribution polish
CI (M1), the ffmpeg sidecar and its license NOTICE (M2) are already in place. This section adds:
- **Installer:** the Tauri NSIS bundle with an embedded WebView2 bootstrapper, an app icon, Start-menu entry and per-user install (no admin rights needed).
- **Updates:** `tauri-plugin-updater` with update files served from GitHub Releases. CI publishes a release on each `v*` tag, so teammates get new versions without reinstalling by hand.
- **Code signing:** without a signature, Windows SmartScreen warns on first start. Sign if the team has a certificate (Azure Trusted Signing or an existing company cert). Otherwise document the "More info → Run anyway" step in the README.
- **Linux:** an AppImage via `tauri build`, which uses system ffmpeg. Document that `ffmpeg` and the GStreamer plugins are required. Optionally an Arch PKGBUILD.

### 4.5 Settings page
Shows and lets you edit:
- recording root override,
- ffmpeg path override,
- preview backend and cache size,
- default export folder and default export mode and preset,
- spike defaults.

"Clear caches" button.

## 5. Edge cases
- No hardware encoder (or the NVENC session limit is reached while Steam records): fall back to software without failing the export, and note it in the result.
- `MaxSizeMb` too small for the length (bitrate < 300 kbit/s): warn and suggest a lower resolution.
- Live session ends (game closed): the watcher sees no changes for 30 s and drops the LIVE badge.
- The notify watcher on Windows with paths on network drives: polling fallback.
- Windows: Defender may scan the sidecar on first start, which is slow. The ffmpeg check runs in the background.
- Windows paths with non-ASCII characters (user names): pass them to ffmpeg as `OsStr`, never through lossy UTF-8.

## 6. Acceptance criteria
- [ ] A precise export of a selection that does **not** start on a 3 s boundary starts on the chosen frame (check: the first frame of the export matches the preview frame at `in_s` ±1 frame).
- [ ] The "Discord (10 MB)" preset gives a file ≤ 10 MB for a 30 s clip at 1080p (or the cropped resolution), and it plays in the Discord desktop client.
- [ ] Moving the sensitivity slider updates the spike bands in under 200 ms for a 1 h session.
- [ ] While a game records, the timeline grows at least every 10 s and the new spikes appear. After a buffer prune, markers and selection stay on the correct content.
- [ ] The Windows installer installs without admin rights on a clean teammate machine, and the app updates itself from a newer tagged release. Precise H.264 exports play in the Films & TV app without the HEVC extension.
- [ ] The Linux AppImage runs on the dev machine.
- [ ] clippy, tests, tsc and lint are clean, and CI is green if it is set up.

## 7. Verification steps
- **Unit tests:**
  - the precise trim offset math,
  - bitrate from the size target,
  - incremental pyramid append and front-trim equal a full rebuild (property test with random chunk sizes),
  - preset storage round trip.
- **Integration** (`#[ignore]`): precise export of the 2-chunk fixture from 1.5 s to 4.2 s gives a duration of 2.7 s ±1 frame, h264.
- **Manual:**
  - Start a game with background recording on, open the live session, and watch it grow for 5 minutes.
  - Export with each preset.
  - Windows smoke test on a teammate's machine, following a short checklist in `docs/windows-test.md`, which this milestone creates.

## 8. Open questions / risks
- Absolute vs current-first-chunk video time (see 4.3). Decide early in this milestone, because it touches M1–M4 types.
- Teammates are mostly on NVIDIA, so NVENC (H.264, HEVC and AV1 on RTX 40+) is the tested encoder path. AMF/QSV stay best effort with software fallback. Ask whether anyone is on a GTX 10xx or older, because there is no AV1 encode there and NVENC session limits are tighter.
- The AV1 option depends on teammates' GPUs and on where clips are posted (Discord plays AV1). Low priority.
