# M3: In/out selection and fast export

> Built **after** M4 (see the build order in `00-overview.md`). This spec describes what was implemented.

## 1. Goal and user-visible outcome
Set an in point and an out point on the timeline, by key, by dragging, or with "use spike region". Then press **Export**. In a few seconds an MP4 with the original quality (HEVC + AAC, no re-encode) appears in the export folder. The cut snaps to the 3 s chunk boundaries around the selection.

## 2. Prerequisites
- M1 (sessions, chunk inventory, `export_dir` from `localconfig.vdf`), M2 (timeline, playhead, spikes, `media/ffmpeg.rs`) and **M4** (built before this one): the preview `Player`, `useView.seek`/`playing`, and `media/chunks.rs` for chunk names and joining.
- `00-overview.md`: DASH layout and "Steam's files are read-only".

## 3. Scope / non-goals
**In scope:** selection state and UI, `media/export.rs` fast mode, export panel, progress, "show in folder".

**Non-goals:**
- frame-accurate cuts and re-encoding or resizing (M5),
- uploading or sharing.

## 4. Design

### Facts this relies on (checked on sample data)
- Each 3 s chunk has **exactly one keyframe, at its start**. ffprobe found keyframes at 297.0, 300.0 and 303.0 s for chunks 100–102. So chunk boundaries are the only cut points that need no re-encode.
- Joining the bytes `init-streamX.m4s + chunk-streamX-N… ` gives a valid fragmented MP4. Then
  `ffmpeg -i v.mp4 -i a.mp4 -map 0:v -map 1:a -c copy -movflags +faststart out.mp4`
  gives a normal MP4 whose streams start at 0. Checked: 3 chunks → 9.0 s, HEVC + AAC, durations match.

### Algorithm (`media/export.rs`)
1. Input: `id`, `in_s`, `out_s` (video time), output path.
2. Chunk range: `n_in = first + floor(in_s / seg)`, `n_out = first + ceil(out_s / seg) − 1`, clamped to the inventory. If there is a gap inside the range, return an error ("selection crosses a gap"). Gaps are rare and only happen at pruning edges.
3. **Snapshot:** right away, copy `init-stream0/1.m4s` and chunks `n_in..=n_out` of both streams into `app_cache_dir()/steamreplay/export/<job>/`. This guards against Steam deleting them mid-export. Use a hard link when it is on the same filesystem, else copy. The cost is about 8 MB per chunk.
4. Join the bytes into `v.mp4` and `a.mp4` inside the job folder, streaming (no full read into memory).
5. Run:
   ```
   ffmpeg -hide_banner -nostdin -v error -i v.mp4 -i a.mp4 -map 0:v:0 -map 1:a:0 -c copy \
          -tag:v hvc1 -movflags +faststart -metadata title="<game> <timestamp>" -f mp4 -y <out.part.mp4>
   ```
   then rename `out.part.mp4` → final name, so partial files never appear in the export folder.
6. Delete the job folder (also on error or cancel).
7. Return `ExportResult { path, actual_in_s, actual_out_s, size_bytes }`.

### File naming
Default: `<ExportDirectory or ~/Videos>/<GameName>_<YYYY-MM-DD_HH-MM-SS>.mp4`, where the timestamp is the wall-clock time of `actual_in_s`. Remove characters that are invalid on Windows (`<>:"/\|?*`) from the game name. Add `_2`, `_3` and so on when the file exists.

### Commands
```rust
pub struct ExportRequest { id: String, in_s: f64, out_s: f64, output: Option<PathBuf> }
pub struct ExportResult { path: PathBuf, actual_in_s: f64, actual_out_s: f64, size_bytes: u64 }
pub struct Snap { first: u32, last: u32, actual_in_s: f64, actual_out_s: f64, est_bytes: u64 }
#[tauri::command] async fn export_clip(req: ExportRequest, job_id: String, progress: Channel<Progress>) -> Result<ExportResult, AppError>;
#[tauri::command] async fn default_export_path(id: String, in_s: f64) -> Result<PathBuf, AppError>;
#[tauri::command] async fn export_snap(id: String, in_s: f64, out_s: f64) -> Result<Snap, AppError>;
```
- `export_snap` gives the snapped cut and the size estimate (`session size / chunk count × chunks`) for the panel. The snap math lives only in `export::chunk_range`. `src/lib/selection.ts` mirrors it for drawing only.
- `output: None` uses the default name, worked out when the job starts. That way, queued exports with default names do not collide.
- Progress is one fraction: 0–0.5 snapshot (files linked or copied), 0.5–1 mux (ffmpeg `out_time`). Cancelling uses `cancel_job`; the frontend picks `job_id`.
- Default folder: the account's `ExportDirectory`, else the system video folder, else `~/Videos`.
- Leftover job folders (after a crash) are deleted at startup.

### Frontend
- **Selection:**
  - `I`/`O` set in or out at the playhead,
  - **Shift+drag** on the timeline to make a range. Alt was dropped, because Alt+drag moves windows on KDE and GNOME,
  - drag the edges to adjust (the cursor changes within 5 px of an edge),
  - `Esc` clears the selection,
  - **loop selection** (toggle `Ctrl+L`, moved here from M4): while there is an in/out selection, the preview player loops inside it. It is implemented in the `Player.tsx` tick: it wraps when playback *crosses* the out point, so a seek past the out point is not pulled back. The panel also has a Loop checkbox. This helps fine-tuning before export,
  - in the spike list, "Select" on a spike (or `S` on the selected spike) sets in/out to the spike region.
- The selection is drawn as a band with **ghost edges** that show where the snapped chunk boundaries really fall.
- **Export panel:** shows selection length, snapped length and estimated size (`chunks × average chunk size`), plus an editable output path with a "Browse…" button (`tauri-plugin-dialog` save) and an Export button. After the export: "Show in folder" (`tauri-plugin-opener` reveal) and "Copy path".
- Several exports can run one after another. A small job list shows their status.
- The export panel has a "Play selection" button that seeks the player to the in point and plays with loop on. `useView.requestPlay` → `MsePlayer.play()` starts playback once the seek has landed.
- The panel (`ExportPanel.tsx`) sits between the timeline and the spike list. Selection state (`inS`, `outS`, `loop`) is in `useView` and is cleared on `reset`.

## 5. Edge cases
- `in_s ≥ out_s`, or the selection is shorter than one chunk: still export at least 1 chunk (3 s).
- Selection at the very end of a live session, where the last chunk may still be being written: leave out the chunk with the highest number when the mpd was changed less than 5 s ago.
- Chunks deleted between selecting and snapshotting (old end of a 300 min buffer): error "part of the selection was removed by Steam". Keep any partial snapshot out of the output.
- Export folder missing or not writable: create it, or show the error with a "choose another folder" action.
- Not enough disk space: ffmpeg fails. Show its stderr in the error toast and remove `.part` files.
- Windows: rename across volumes fails, so write `.part` in the target folder, not the cache.

## 6. Acceptance criteria
- [ ] Export a 30 s spike region from `bg_1445790_20261015_210120`. The file plays in mpv and VLC (and on Windows in the Films & TV app, if the HEVC extension is installed) with audio in sync. Length = snapped length ±0.1 s.
- [ ] The export takes < 3 s for 30 s of footage on the dev machine (I/O bound).
- [ ] Nothing in Steam's folder changes (check: `find <gamerecordings> -newer <timestamp file>` is empty except files Steam itself writes).
- [ ] Cancelling mid-export leaves no `.part` file and no job folder.
- [ ] On Windows: export to the default `ExportDirectory` and to a folder on another drive. Both files play in VLC. Playing in the Films & TV app needs the paid "HEVC Video Extensions", so tell teammates to use VLC or wait for H.264 precise export (M5).
- [ ] Export from a pruned session (startNumber 3418) uses the correct chunks: the audio waveform of the export matches the selection.
- [ ] Loop selection: with Ctrl+L on, playback wraps from the out point back to the in point without a visible stall when both are in the buffered run.
- [ ] clippy, tests, tsc and lint are clean.

## 7. Verification steps
- **Unit tests:** chunk-range math (in/out → n_in/n_out with `first ≠ 1`, clamping, gap detection), file-name sanitising and dedupe.
- **Integration test** `export_session` (`#[ignore]`, needs ffmpeg): the clip fixture has audio only, so it uses `STEAMREPLAY_SESSION_DIR=<bg folder>`. It exports the first 30 s → ffprobe shows hevc + aac with a duration of 30 ± 0.1 s, and no job folder or `.part` file is left. Measured: 0.35 s on `bg_1445790_20261015_210120` and 0.31 s on the pruned `bg_1623730_20261010_164846`.
- **Manual:** export 3 different spikes, watch them, and compare the start frame with the timeline position.

## 8. Open questions / risks
- HEVC MP4s from 5120×1440 recordings play poorly in some apps (Discord embeds, WhatsApp). This is expected. Precise mode with H.264 output and optional downscale comes in M5. Point this out in the UI tooltip.
- `hev1` vs `hvc1` tag: the mux uses `-tag:v hvc1` (QuickTime/Apple need it). ffprobe reads the result fine. Playback in other players is checked manually.
