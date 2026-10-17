# SteamReplay: overview

> Read this first. Each milestone spec (`01`–`08`, no `07`) assumes you know this file.

## Context and goals
The team records gameplay with **Steam Game Recording** in background mode. Today the only way to find good moments is manual markers. Most good moments are loud: people laughing, shouting or cheering.

SteamReplay is a small desktop tool that:
1. lists Steam's background recordings and saved clips,
2. shows the audio waveform of a recording and points out loud parts automatically,
3. shows Steam's own markers on the same timeline,
4. lets you preview the video, set in and out points, and export an MP4 clip.

**Constraints:** typed languages only (no Python). Small, local and offline.

**Platforms:** **Windows is the main target**, because the teammates who will use the tool are on Windows. Development happens on Linux (CachyOS), which must work too. Because of this:
- Every milestone ships a Windows build that a teammate can run. The CI workflow in M1 builds it.
- ffmpeg is bundled for Windows from M2 on, when ffmpeg is first used.
- Every milestone's acceptance criteria include a check on Windows.
- Do not rely on Linux-only behaviour: path separators, case-sensitive file names, symlinks, `/tmp`, a system ffmpeg, or VAAPI.

## How Steam stores recordings
Checked on a Linux machine (CachyOS, Steam native, RTX 4080). The Windows layout is the same under the Steam install folder.

### Location
`<SteamRoot>/userdata/<accountId>/gamerecordings/`

Finding `<SteamRoot>`:
| OS | Candidates (in order) |
|---|---|
| Linux | `~/.local/share/Steam`, `~/.steam/steam` (symlink), Flatpak `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` |
| Windows | Registry `HKCU\Software\Valve\Steam` → `SteamPath`, fallback `C:\Program Files (x86)\Steam` |

`<accountId>` is any numeric folder under `userdata/` that has a `gamerecordings/` folder. There can be several accounts.

The user can change the recording folder in Steam's settings. In that case the path is stored in `localconfig.vdf` (see below), and the tool must also allow a manual override.

### Folder layout
```
gamerecordings/
  gamerecording.pb                      protobuf index (see below)
  video/
    bg_<appid>_<YYYYMMDD_HHMMSS>/       background recording, one per game session
      session.mpd                       MPEG-DASH manifest
      init-stream0.m4s                  video init segment
      init-stream1.m4s                  audio init segment
      chunk-stream0-NNNNN.m4s           video segments (3 s each)
      chunk-stream1-NNNNN.m4s           audio segments (3 s each)
  timelines/
    timeline_<appid><YYYYMMDD_HHMMSS>.json   markers for one game session (no separator between appid and date)
  clips/
    clip_<appid>_<YYYYMMDD_HHMMSS>/     clips the user saved in Steam
      clip.pb                           protobuf metadata
      thumbnail.jpg
      timelines/timeline_*.json         copy of the relevant timeline
      video/bg_<appid>_<date>/          same DASH layout as above, only the clip's chunks
```
The timestamps in the names are **local time**.

### DASH manifest (`session.mpd`)
A real example from `bg_1445790_20261015_210120`:
```xml
<MPD type="static" mediaPresentationDuration="PT1H1M57.947S" maxSegmentDuration="PT3.0S" ...>
 <Period id="0" start="PT0.0S">
  <AdaptationSet id="0" contentType="video" startWithSAP="1" ...>
   <Representation id="0" mimeType="video/mp4" codecs="hev1.2.4.L123.B0" bandwidth="22000000" width="5120" height="1440">
    <SegmentTemplate timescale="1000000" duration="3000000"
      initialization="init-stream$RepresentationID$.m4s"
      media="chunk-stream$RepresentationID$-$Number%05d$.m4s" startNumber="1" />
  </Representation></AdaptationSet>
  <AdaptationSet id="1" contentType="audio" ...>
   <Representation id="1" mimeType="audio/mp4" codecs="mp4a.40.2" bandwidth="128000" audioSamplingRate="48000">
    <AudioChannelConfiguration ... value="2" />
    <SegmentTemplate timescale="1000000" duration="3000000" ... startNumber="1" />
 </Representation></AdaptationSet></Period></MPD>
```
- **Video:** HEVC (`hev1`). Resolution follows the monitor (here 5120×1440, 32:9). About 60 fps and about 20 Mbit/s. Each segment starts on a keyframe (`startWithSAP=1`).
- **Audio:** **one** AAC-LC track, 48 kHz stereo. Game audio and microphone are **mixed into this track** (`Audio_Mic=1`). There is no separate mic track.
- **Segments:** always 3 s (`duration=3000000` at `timescale=1000000`). Segment *n* covers video time `[(n - startNumber)·3 s, (n - startNumber + 1)·3 s)`.
- **Old segments are deleted from the start of the buffer.** Steam deletes the oldest chunks once `BackgroundMaxKeep` is reached. It then raises `startNumber` (seen: `startNumber="3418"` in `bg_1623730_20261010_164846`, where the first file is `chunk-stream0-03418.m4s`). That segment starts `(startNumber-1)·3 s` after the folder's start timestamp.
- **Live sessions:** while a game runs, Steam rewrites `session.mpd` and adds chunks. Old chunks can disappear at any moment.
- **ffmpeg reads `session.mpd` directly** (`-i /abs/path/session.mpd`). **Use an absolute path.** With a relative path, ffmpeg resolves segment URLs wrongly and fails.
- **Speed:** decoding all audio of a 1 h session to mono 8 kHz f32 took **about 2 s** (`ffmpeg -i session.mpd -map 0:a -ac 1 -ar 8000 -f f32le`). Audio analysis is cheap. Decoding 5K HEVC video is not.

### Timeline JSON
```json
{
  "daterecorded": "1792098073",
  "starttime": "0",
  "entries": [
    { "id": "16082308127955867072", "time": "2291279", "type": "usermarker", "title": "", "description": "",
      "icon": "steam_marker", "priority": 0, "duration": "0", "possible_clip": 2 },
    { "id": "1", "time": "3735706", "type": "error", "description": "#GameRecording_RecordingFailed" }
  ],
  "endtime": "5986786"
}
```
- Numbers are stored **as strings**. `time` and `endtime` are **milliseconds since the timeline start**.
- Seen `type` values: `usermarker`, `error`. Expect others too, such as achievements, game-provided events and `possible_clip` hints. Parse unknown types generically and keep the raw JSON.
- The timeline starts a few seconds **before** the bg folder (timeline `…210113`, folder `…210120`). To convert:
  `video_t = marker_ms/1000 − (bg_folder_ts − timeline_ts) − (startNumber − 1)·3`.
  This formula is **not yet confirmed** with a known marker. M2 must check it.
- A timeline can run longer than the video, for example when recording failed (the `error` entry at 3735 s matches the 62 min video length).
- Matching rule: a timeline and a bg folder belong together when they have the same appid and the timeline is the latest one that starts at or before the folder's timestamp.

### `gamerecording.pb` / `clip.pb`
Protobuf with no published schema. `protoc --decode_raw` shows, for each recording, `1: timeline id`, `2: appid`, `3: unix start`, `4: duration ms`, and repeated `7 {…}` items that look like marker or clip references. **Not needed for the MVP.** Folder names, the mpd and the timeline JSON carry everything we need.

### `localconfig.vdf`
`<SteamRoot>/userdata/<accountId>/config/localconfig.vdf` is Valve KeyValues text. The relevant block:
```
"GameRecording"
{
    "BackgroundRecordMode"  "1"
    "Audio_Mic"             "1"
    "ExportDirectory"       "/home/user/Videos"
    "InstantClipDuration"   "90"
    "BackgroundMaxKeep"     "300min"
    "MarkerKey"             "KEY_PAD_MINUS"
}
```
Use `ExportDirectory` as the default export folder. If a recording-path key is present (name not yet seen, because the default folder was used), use it as the gamerecordings root.

### Game names
`<library>/steamapps/appmanifest_<appid>.acf` → `"name"`. The libraries are listed in `<SteamRoot>/steamapps/libraryfolders.vdf`. If there is no manifest (game uninstalled), show `App <appid>`.

### Sample data on the dev machine
`~/.local/share/Steam/userdata/10000001/gamerecordings/`
| Path | What it is |
|---|---|
| `video/bg_1445790_20261015_210120` | 1 h 02 min, startNumber 1, 9.8 GB. Main end-to-end sample. |
| `video/bg_1623730_20261010_164846` | Pruned buffer, startNumber 3418 |
| `video/bg_1623730_20261010_204535` | Long session, 28 GB |
| `clips/clip_1478500_20260913_011121` | Small saved clip, 2 chunks. Good for test fixtures. |
| `timelines/timeline_144579020261015_210113.json` | Has 5 user markers and an `error` entry |

## Stack decision
**Tauri 2: Rust backend + React/TypeScript (Vite, `strict`) frontend + ffmpeg/ffprobe as external processes.**
- Typed from end to end. Rust↔TS IPC types are generated with **`tauri-specta`**, so there are no hand-written duplicate types.
- Runs on Linux (WebKitGTK) and Windows (WebView2), with a small binary.
- Rust handles the heavy work: audio analysis, file handling and running ffmpeg. TS/React handles the UI: canvas waveform and player.
- **Why the preview needs transcoding:** WebKitGTK and WebView2 cannot reliably play 5K HEVC, especially through MSE on Linux/NVIDIA. The backend therefore makes low-res **H.264** preview chunks on demand (M4). Export always uses the **original** HEVC data.
- **ffmpeg:** Windows uses a bundled ffmpeg as a Tauri sidecar, set up in M2. Linux uses the system `ffmpeg` (any build with HEVC/AAC/libx264).
- **Windows webview:** WebView2 is based on Chromium, so playing H.264 through MSE is reliable there. The webview risk from M4 mainly affects Linux. Teammates mostly have **NVIDIA** GPUs, so the CUDA/NVDEC + NVENC path is the main one on both OSes (the same ffmpeg flags work on Windows). AMD/Intel get a best-effort `d3d11va` + AMF/QSV path, and everything falls back to software. Video is not decoded in-process (no ffmpeg bindings), because external processes are simpler and avoid licensing issues.

## Architecture

### Repo layout (target)
```
steamreplay/
  docs/specs/              these specs
  src-tauri/               Rust crate (Tauri app)
    src/
      main.rs / lib.rs     Tauri builder, specta export
      commands.rs          #[tauri::command] wrappers only (thin)
      error.rs             AppError (thiserror) → serialisable to TS
      steam/locate.rs      Steam root, accounts, localconfig.vdf, game names
      steam/mpd.rs         session.mpd parsing
      steam/recordings.rs  list sessions and clips, chunk inventory, timeline mapping
      steam/timeline.rs    timeline JSON parsing
      media/ffmpeg.rs      locate ffmpeg, run processes, progress parsing
      media/audio.rs       audio extraction → envelope + peak pyramid, cache
      media/spikes.rs      loudness spike detection
      media/export.rs      clip export (fast / precise)
      media/chunks.rs      Steam chunk file names, joining and probing (shared)
      media/preview/       preview segments: transcode + backends, LRU cache, queue, `replay://` protocol
    tests/fixtures/        small real files (mpd, timeline json, vdf, 2-chunk clip)
  src/                     React + TS frontend
    bindings.ts            generated by tauri-specta (do not edit)
    components/ SessionList, Timeline (canvas), Player, ExportPanel, SpikeList
    state/                 zustand store (selected session, playhead, in/out, zoom)
```

### Backend modules
| Module | Responsibility | Milestone |
|---|---|---|
| `steam/locate.rs` | Steam root and accounts, `localconfig.vdf` (`keyvalues-parser` crate), game names | M1 |
| `steam/mpd.rs`, `steam/recordings.rs` | Parse the mpd (`quick-xml`) and inventory chunks (which numbers really exist on disk) | M1 |
| `steam/timeline.rs` | Parse timelines, match them to bg folders, convert to video time | M1 (parse), M2 (display) |
| `media/ffmpeg.rs` | Find ffmpeg and ffprobe, run them async (tokio), parse `-progress pipe:1` | M2 |
| `media/audio.rs` | `ffmpeg … -f f32le pipe:1` → per-10 ms RMS/peak → min/max pyramid → cache | M2 |
| `media/spikes.rs` | Find loud parts | M2 (tuning UI in M6) |
| `media/export.rs` | Fast export (stream copy, snapped to 3 s) in M3; precise export (re-encode) in M5 | M3, M5 |
| `media/encoders.rs` | Cached encoder/hwaccel capabilities, encoder choice and rate control for precise export | M5 |
| `media/chunks.rs` | Chunk file names, `read_files`, `probe_start` (used by audio, preview and export) | M4 (moved from `audio.rs`) |
| `media/preview/` | 12 s H.264 preview segments, backend detection, LRU disk cache, `replay://` protocol | M4 |

### Cross-cutting rules
- **Errors:** one `AppError` enum (thiserror) that serialises to `{ kind, message }`. The frontend shows it in a toast. Commands never panic.
- **Paths:** use `PathBuf` everywhere and give ffmpeg absolute paths. Do not assume `/` as the separator.
- **Cache:** `app_cache_dir()/steamreplay/`. Cache keys contain the session folder name, `startNumber` and the highest chunk number, so pruning or growth makes old entries invalid.
- **Time:** the backend uses `f64` seconds of **video time**. Video time 0 is the first chunk on disk (`startNumber`). Everything sent to the UI is in video time.
- **Long jobs:** audio analysis, export and preview run as tokio tasks and report progress through a Tauri `Channel<Progress>`. Commands that start long jobs return when the job finishes. Cancelling uses a job id plus `cancel_job(id)`.
- **Steam's files are read-only:** the tool never writes, renames or deletes anything inside Steam's folders.
- **Windows builds:** a GitHub Actions workflow (from M1) builds the Windows NSIS installer on `windows-latest` for every push to `main` and uploads it as an artifact. Teammates install that build to test each milestone. If CI is not available, `cargo tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc` cross-compiles from Linux (experimental, NSIS only).
- **Tooling:** `cargo clippy -D warnings`, `cargo test`, `pnpm tsc --noEmit`, `pnpm lint` (eslint). pnpm is the package manager.

## Milestones
| # | Spec | Outcome |
|---|---|---|
| 1 | [01-scaffold-discovery.md](01-scaffold-discovery.md) | App starts and lists recordings and clips with game name, date, length and marker count |
| 2 | [02-waveform-spikes.md](02-waveform-spikes.md) | Zoomable waveform with Steam markers and ranked loud parts |
| 3 | [03-export-fast.md](03-export-fast.md) | Set in and out points, export an MP4 with stream copy |
| 4 | [04-preview-playback.md](04-preview-playback.md) | Video preview synced with the timeline |
| 5 | [05-precise-export.md](05-precise-export.md) | Frame-accurate re-encode with H.264/HEVC/AV1, downscale, crop and size presets |
| 6 | [06-spike-tuning-settings.md](06-spike-tuning-settings.md) | Live spike tuning with per-game presets, settings page, shortcut overlay, recent exports |
| 7 | *(skipped)* | Live sessions were dropped. An open session does not grow; use Refresh. Export already leaves out the chunk being written. |
| 8 | [08-distribution.md](08-distribution.md) | Icon, per-user installer, auto-update from GitHub Releases, AppImage, README |

**Build order: 01 → 02 → 04 → 03 → 05 → 06 → 08.** M4 (preview) comes before M3 (export): choosing in and out points makes much more sense when you can see the video. The file numbers were kept, so the M3/M4 labels in the specs still refer to the files.
