# M1: Scaffold and recording discovery

## 1. Goal and user-visible outcome
Start the app and see a list of all Steam background recordings and saved clips for every local Steam account. Each row shows game name, start date and time, length, number of markers and size on disk. Selecting a row shows a details panel: folder path, resolution, codecs, chunk range, gaps, and the raw marker list in video time.

## 2. Prerequisites
- `00-overview.md`: all sections, especially "How Steam stores recordings" and "Cross-cutting rules".
- Toolchain: Rust stable, Node and pnpm, Tauri 2 CLI (`cargo install tauri-cli --version '^2'` or `pnpm tauri`). Linux needs `webkit2gtk-4.1` (installed on the dev machine).

## 3. Scope / non-goals
**In scope**
- Tauri 2 + React + TS (Vite, strict) project scaffold with eslint, clippy and tauri-specta binding generation.
- `steam/locate.rs`, `steam/mpd.rs`, `steam/timeline.rs`, `steam/recordings.rs`, `error.rs`, `commands.rs`.
- Session list UI and a simple details panel.
- Settings: a manual override for the gamerecordings root, stored with `tauri-plugin-store`.
- Init the git repo and add a GitHub Actions workflow: matrix `windows-latest` + `ubuntu-latest` running `cargo clippy`, `cargo test`, `pnpm tsc --noEmit` and `pnpm tauri build`. Upload the Windows `.exe` (NSIS) as an artifact. Use `tauri-apps/tauri-action` or plain steps.
- "Copy diagnostics" button in settings: copies detected Steam roots, accounts, the `GameRecording` vdf block and session count to the clipboard, so remote Windows teammates can report problems.

**Non-goals:** ffmpeg, waveform, playback, export, live refresh (a "Refresh" button is enough), reading the protobuf files.

## 4. Design

### Scaffold
- `pnpm create tauri-app` → React + TS template, then move it into the repo root (`src/`, `src-tauri/`).
- Crates: `tauri 2`, `serde`, `serde_json`, `thiserror`, `quick-xml` (serde feature), `keyvalues-parser`, `chrono`, `dirs`, `specta`, `tauri-specta`, `tauri-plugin-store`. On Windows also `winreg`.
- In debug builds, tauri-specta writes `src/bindings.ts` on app start.

### Rust types (exported to TS via specta)
```rust
pub struct SteamAccount { pub account_id: String, pub recordings_root: PathBuf, pub export_dir: Option<PathBuf> }

pub enum SessionKind { Background, Clip }

pub struct SessionSummary {
    pub id: String,              // stable: "<accountId>/<bg|clip>/<folder name>"
    pub kind: SessionKind,
    pub account_id: String,
    pub app_id: u32,
    pub game_name: String,       // fallback "App <appid>"
    pub started_at: String,      // ISO 8601 local, from the folder name (+ startNumber offset)
    pub duration_s: f64,         // from the chunk inventory, not mediaPresentationDuration
    pub marker_count: u32,
    pub size_bytes: u64,
    pub dir: PathBuf,            // absolute path to the folder that holds session.mpd
}

pub struct MpdInfo {
    pub segment_duration_s: f64, // duration / timescale
    pub start_number: u32,
    pub video: StreamInfo,       // rep id, codecs, width, height, bandwidth
    pub audio: StreamInfo,       // rep id, codecs, sample rate, channels
    pub media_template: String,  // "chunk-stream$RepresentationID$-$Number%05d$.m4s"
    pub init_template: String,
}

pub struct ChunkInventory { pub first: u32, pub last: u32, pub missing: Vec<u32> } // based on video stream files that exist

pub struct Marker { pub kind: String, pub title: String, pub video_time_s: f64, pub raw: serde_json::Value }

pub struct SessionDetail { pub summary: SessionSummary, pub mpd: MpdInfo, pub chunks: ChunkInventory, pub markers: Vec<Marker>, pub timeline_file: Option<PathBuf> }
```

### Commands
```rust
#[tauri::command] async fn list_accounts() -> Result<Vec<SteamAccount>, AppError>;
#[tauri::command] async fn list_sessions() -> Result<Vec<SessionSummary>, AppError>; // all accounts, newest first
#[tauri::command] async fn get_session(id: String) -> Result<SessionDetail, AppError>;
#[tauri::command] async fn set_recordings_root_override(path: Option<PathBuf>) -> Result<(), AppError>;
```

### Logic notes
- **`locate.rs`:** try the candidates from the overview and canonicalise them (symlinks). Remove duplicates. For each `userdata/<id>/` that has `gamerecordings/`, or a custom path from `localconfig.vdf`, return an account. Read `ExportDirectory`. Parse `steamapps/libraryfolders.vdf` → library paths → `appmanifest_<appid>.acf` → `name`. Cache the appid→name map in memory.
- **`mpd.rs`:** deserialise with quick-xml serde. Steam's XML has a namespaced root and tab indentation, so ignore unknown attributes. Expand `$RepresentationID$` and `$Number%05d$` with a small formatter that supports a `%0Nd` width.
- **Chunk inventory:** list the folder once and parse `chunk-stream0-(\d+)\.m4s`. `first` = min, `last` = max. `missing` = gaps between them. Duration = `(last − first + 1) · segment_duration`. Do **not** trust `mediaPresentationDuration`, because it is stale for live sessions and includes deleted chunks.
- **`timeline.rs`:** parse with serde. All number fields are strings, so use custom `de::from_str` helpers. Keep unknown fields in `raw`. Parse the filename `timeline_<appid><YYYYMMDD_HHMMSS>.json`: the last 15 characters are the date, the rest is the appid.
- **Matching a timeline to a bg folder:** same appid, and the timeline is the latest one with `timeline_ts ≤ bg_ts`. For clips, use the timeline inside `clips/<clip>/timelines/`.
- **Video time of a marker:** `video_t = time_ms/1000 − (bg_ts − timeline_ts) − (first − 1)·seg`. Leave out markers with `video_t < 0` or `> duration`, but count them in a "markers outside the buffer" field in the details panel.
- **Clips:** the session folder is `clips/<clip>/video/bg_*/`. The clip's chunks are numbered from 1 again, so the same rules apply.
- **Never** write inside Steam's folders.

### UI
- Left: session list grouped by game, newest first. The "Background" and "Clip" types have different badges.
- Right: details panel with folder path (click to open in the file manager via `tauri-plugin-opener`), stream info, chunk range and gaps, and a marker table (time as `hh:mm:ss`, type, title).
- Settings dialog: shows the detected roots and offers an override folder picker (`tauri-plugin-dialog`).

## 5. Edge cases
- No Steam found → empty state that says which paths were checked, plus the override button.
- Several accounts, Flatpak and native Steam both installed → remove duplicates by canonical path.
- A bg folder without `session.mpd` (just created) or without any chunks → list it with duration 0 and the badge "empty".
- Pruned buffer: `bg_1623730_20261010_164846` has first chunk 3418. `started_at` must be the folder time + 3417·3 s.
- A timeline with no matching bg folder (its video was fully deleted) → ignore it in M1.
- Folder names that do not match the pattern → skip them with a `log::warn!`.
- Windows: registry lookup through `winreg`, behind `#[cfg(windows)]`. The registry value uses forward slashes (`c:/program files (x86)/steam`), so normalise it with `PathBuf`. Paths may contain spaces (`Program Files (x86)`).
- Windows: Steam is often installed on another drive, or the recording folder is set to another drive (large files). Read the custom-path key from `localconfig.vdf` (see open questions) and support the manual override.
- Windows file names are case-insensitive. Match `chunk-stream0-…` without regard to case.

## 6. Acceptance criteria
- [ ] `pnpm tauri dev` starts the app on Linux and shows all 3 bg sessions and 3 clips from the sample folder, with correct game names.
- [ ] `bg_1445790_20261015_210120` shows duration ≈ 1:01:57 (±3 s), 5 user markers, 5120×1440 HEVC, AAC 48 kHz 2 ch.
- [ ] `bg_1623730_20261010_164846` shows first chunk 3418 and a start time shifted by 2 h 50 min 51 s from the folder name.
- [ ] `src/bindings.ts` is generated, and the frontend uses only generated types for IPC.
- [ ] `cargo clippy -- -D warnings`, `cargo test`, `pnpm tsc --noEmit` and `pnpm lint` are all clean.
- [ ] The CI workflow (`.github/workflows/build.yml`) builds a Windows NSIS installer and a Linux build, runs clippy and tests on both, and uploads the installer as an artifact.
- [ ] A teammate installs the Windows build. It finds Steam through the registry and lists their recordings with correct game names, durations and markers. If the list is wrong, the teammate sends the "Copy diagnostics" output.

## 7. Verification steps
- **Unit tests** (`src-tauri/tests/` or `#[cfg(test)]`), with fixtures copied into `src-tauri/tests/fixtures/`:
  - `session.mpd` from `bg_1445790…`, and the pruned one from `bg_1623730_20261010_164846`,
  - `timeline_144579020261015_210113.json`,
  - a trimmed `localconfig.vdf` that keeps only the `GameRecording` block (remove tokens and other personal data),
  - a minimal `libraryfolders.vdf` and `appmanifest_*.acf`,
  - a fake directory tree (empty chunk files) to test the inventory, gaps and pruning.
  
  Test filename parsing (timeline and bg folder names), number-as-string parsing, and the marker video-time formula.
- **Manual:** run the app and compare the rows with `ls` and `ffprobe` output for the sample sessions.

## 8. Open questions / risks
- Name of the custom recording-path key in `localconfig.vdf`: change the folder in Steam once, diff the vdf, and add the key. This matters more on Windows, where teammates are likely to record to a second drive. Ask a teammate for their `GameRecording` vdf block (it contains no secrets).
- The timeline↔video offset formula is unconfirmed. M1 shows the computed values. M2 checks them against audio.
- Steam may change folder or file naming in updates. Keep all parsing in `steam/` with clear errors.
