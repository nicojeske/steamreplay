# M7: Live sessions

## 1. Goal and user-visible outcome
While a game is recording, its session is marked **LIVE** in the list. When you open it, the waveform, markers, spikes and the preview grow on their own, at least every 10 s, without reopening. With **follow mode** on, the view stays at the end, so you can mark a funny moment from a second monitor and export it right away. A new recording session appears in the list without pressing Refresh. When Steam prunes old chunks from the start of the buffer, the content under the playhead, the markers and the selection stays the same.

## 2. Prerequisites
- M1–M6 done. M6 is not strictly needed: spike re-detection reuses `get_spikes` as it is.
- `00-overview.md`, "How Steam stores recordings": live sessions, pruning, `startNumber`, and the cache key rule.
- `02-waveform-spikes.md` (pyramid and cache), `04-preview-playback.md` (MSE player, preview cache).

## 3. Scope / non-goals
**In scope:**
- live detection,
- a file watcher for the open session and for the `video/` folder,
- incremental audio analysis,
- front trim on pruning,
- the `session-updated` event,
- frontend shifting and growing,
- the LIVE badge and follow mode.

**Non-goals:**
- watching sessions that are not open, other than noticing that they exist,
- live updates of clips (Steam writes clips once),
- switching to absolute time (see the decision below),
- auto-export or auto-marking.

**Decision (agreed in planning): video time stays relative.**
- Video time 0 stays "the first chunk on disk" (`00-overview.md`, "Time").
- When `first` goes up, the backend reports `time_shift_s = (new_first − old_first)·seg`, and the frontend moves everything back by that amount.
- Pruning only happens once `BackgroundMaxKeep` is reached (hours), so this path is rare. It must still be correct.

## 4. Design

### 4.1 Live flag
- `SessionSummary` gets `live: bool`, which is true when `session.mpd` changed less than `LIVE_WINDOW_S = 30` s ago.
- Today `is_live` (`commands.rs:489`) uses `LIVE_MPD_AGE_S = 5` (`export.rs:20`) to drop the chunk being written. Keep both thresholds, but put them as named constants in `steam/recordings.rs`:
  - `LIVE_WINDOW_S`: for the UI badge and the watcher,
  - `WRITING_WINDOW_S = 5`: for export.
  Both use one helper, `mpd_age(dir) -> Option<Duration>`.

### 4.2 Watcher (`steam/watch.rs`, new)
- Crates: `notify` and `notify-debouncer-mini` (2 s debounce).
- `WatchState` (managed) holds at most:
  - one **session watcher**, on the open bg session folder (non-recursive; it reacts to `session.mpd` and `chunk-*` changes),
  - one **roots watcher**, on each `gamerecordings/video/` folder (non-recursive), which notices new `bg_*` folders.
- Commands:
  ```rust
  #[tauri::command] async fn watch_session(id: String) -> Result<(), AppError>;   // replaces the previous one
  #[tauri::command] fn unwatch_session();
  ```
  The roots watcher starts in `setup` and is rebuilt when `set_recordings_root_override` changes.
- **Fallback:** when `notify::recommended_watcher` fails, or the path is a UNC or network drive (Windows `\\server\…`, or a mapped drive where `GetDriveTypeW == DRIVE_REMOTE`), use `PollWatcher` with a 2 s interval.
- Events (Tauri events, typed with `tauri-specta` `Event`):
  - `sessions-changed {}`: a `bg_*` folder appeared or disappeared, or a session's live flag changed. The frontend calls `list_sessions` again (the same code path as the Refresh button, `App.tsx:37-40`).
  - `session-updated { id, first, last, durationS, timeShiftS, live }`: the open session changed (see 4.3).
- A session with no changes for `LIVE_WINDOW_S` gets a final `session-updated { live: false }`, checked by a 10 s tick inside the watcher task.

### 4.3 Updating the open session
On a debounced change for the open session:
1. Take a new inventory: `get_session` / `recordings::inventory`. Compare it with `Analysis.first/last` (`media/mod.rs:34-39`).
2. **Growth (`last` went up):**
   - Decode only the new audio chunks. `audio::extract` already pipes each gap-free run separately (`audio.rs:206-267`). Add `extract_range(inv, from, to)` that reuses this for the run `old_last+1 ..= new_last`.
   - Place the bins at the nominal `(n − first)·seg` (`audio.rs:227-243`).
   - Skip the chunk that is still being written (newest chunk, `WRITING_WINDOW_S`). It is picked up on the next change.
   - `Pyramid::append(&mut self, level0: &[Bin])` (`media/pyramid.rs`) extends level 0 and rebuilds only the tail of each higher level: the last partial group of `FACTOR` bins onward.
3. **Pruning (`first` went up):**
   - `Pyramid::trim_front(&mut self, bins)` drops `Δ·seg / 10 ms` bins from level 0 and rebuilds the higher levels. Rebuilding everything is fine here, because it is rare; aligning groups of 4 after a front trim is not worth the complexity.
   - Recompute the gaps from the inventory, as on load (`media/mod.rs:188`).
4. Replace the `Analysis` in `MediaState.loaded` with the new `first`/`last`. Today `get(id, Some(range))` evicts on a range change (`media/mod.rs:129-139`), so update it in place instead of evicting.
5. **Cache file:**
   - Keep the key `<sha1>-<first>-<last>.bin` (`pyramid.rs:69-75`). Write a new file and call `remove_stale` (`pyramid.rs:133`).
   - For a 1 h session level 0 is about 4.5 MB, so write at most every 60 s, and on `unwatch_session` / app exit.
   - Waveform reads in between come from memory.
6. Send `session-updated` with the new `durationS` (`recordings.rs:138-140`) and `timeShiftS = (new_first − old_first)·seg` (0 when nothing was pruned).
7. **Errors:** if a chunk disappears during the decode (pruned mid-read), drop this update and try again on the next event. Never fail the whole session.

Markers (`markers_for`, `recordings.rs:144-163`) already depend on `first`. The frontend calls `get_session` again after the event.

### 4.4 Preview while live
- Today `get_preview_info` (`commands.rs:354`) reopens the preview session whenever the inventory differs (`:367-371`), and a partial last segment's cache name contains `last` (`preview/mod.rs:49-56`).
- **Growth:**
  - `get_preview_info` updates `count` in place (the segment plan only grows at the end) instead of starting a new session, when `first` is unchanged and `v0` is the same.
  - `MsePlayer` gets `extend(info: PreviewInfo)`, which sets `opts.info`, raises `ms.duration` (only while `readyState === "open"`; otherwise it waits for `sourceopen`) and lets prefetch continue into the new segments (`msePlayer.ts:86, 233, 269, 289`).
  - Once the last segment is no longer partial, it gets transcoded again under its final name. That is already the behaviour, because the name includes `last`.
- **Pruning:** the preview cache folder is keyed by `first` (`preview/cache.rs:29-32`), and the segment indices are counted from `first` (`preview/mod.rs:60-70`). All segments therefore shift. The frontend rebuilds the player and seeks to `playhead − timeShiftS`. A short reload is acceptable for something this rare.

### 4.5 Frontend
- **`App.tsx`:** listen to `sessions-changed` and call `refresh()`. Call `watchSession(id)` when a bg session is selected that is live (or always, which is cheap). Call `unwatchSession` when the selection changes or the app unloads.
- **`session-updated` handler** (in `WaveformPanel.tsx` or one hook `useLiveSession`):
  1. If `timeShiftS > 0`, call `useView.shift(−timeShiftS)`, a new action in `src/state/view.ts`. It moves the playhead, the view start, `inS` and `outS`, and clamps them to `[0, durationS]`. If the selection ends before 0 it is cleared; if it starts before 0 it is clamped to 0, with a toast "Steam removed the oldest part of the recording".
  2. Set the new `durationS`. Fetch the visible waveform range again, and call `getSpikes` again (with the current tuning params from M6). Keep the selected spike by time, not by index.
  3. Call `getSession` again for the markers.
  4. Preview: `player.extend(info)`, or reload on a shift.
- **LIVE badge:** a red dot with "LIVE" in `SessionList.tsx` rows and in the session header (`SessionDetails.tsx`).
- **Follow mode** (`F`, plus a toggle button next to the badge, shown only while live):
  - When on and the session grows, the view end is set to the new `durationS`.
  - Scrolling or zooming by hand turns it off.
  - Add `F` to the shortcut list from M6 (`src/lib/keys.ts`).
  - This is separate from the playback follow mode in M4: that one follows the playhead, this one follows the end of the recording.
- The ExportPanel needs no changes. M3/M5 already drop the chunk that is being written.

## 5. Edge cases
- **The game closes:** after `LIVE_WINDOW_S` with no change, send `live: false`. The badge goes away, the last chunk becomes exportable, and the cache is written.
- **Steam starts a new session for the same game** (restart): a new `bg_*` folder, so `sessions-changed`. The open session simply stops growing.
- **Many quick events while a decode is running:** run one update at a time, coalesce pending events and process the latest state afterwards.
- **A chunk is pruned while the preview or export reads it:** export already reports `NotFound("…removed by Steam")` (`export.rs:148-159`). The preview skips the segment (existing M4 behaviour).
- **Pruning and growth in the same update:** apply the trim first, then the append.
- **Clock and mtime precision on network drives:** `PollWatcher` compares the mpd mtime and the chunk count, not just the mtime.
- **Windows:**
  - `ReadDirectoryChangesW` can report a file before it is fully written, so never read the newest chunk (see `WRITING_WINDOW_S`),
  - non-ASCII paths go through `PathBuf` only,
  - Defender may hold files briefly: retry once after 500 ms on a sharing violation.
- **Leaving the session mid-update:** cancel the update through the job token (`start_job`).

## 6. Acceptance criteria
- [ ] With a game recording, the session shows LIVE within 5 s of opening the app, and a new session appears without pressing Refresh.
- [ ] The open live session's waveform and duration grow at least every 10 s. New spikes appear without user action.
- [ ] Follow mode keeps the newest audio in view for 5 minutes. Scrolling by hand turns it off.
- [ ] The preview plays up to near the live edge without reopening the session.
- [ ] After a prune (simulate one: a copied session folder in a temp dir, deleting the first chunks and raising `startNumber`), the markers, playhead and selection still point at the same content (checked by audio peak position).
- [ ] After the game closes, LIVE disappears within 40 s, and exporting the last seconds works.
- [ ] Incremental analysis gives the same waveform cache as a full analysis of the final session (bin-exact apart from the last partial bin).
- [ ] **Windows:** live growth and the new-session notification work on a teammate machine (NTFS). If someone uses a recording folder on a network drive, the polling fallback works there.
- [ ] clippy, tests, tsc and lint are clean; CI is green.

## 7. Verification steps
- **Unit tests:**
  - **Property test** (`proptest`, dev-dependency): for random chunk bin counts, `build(all)` equals `build(prefix)` + `append(rest)`, and `trim_front(k)` + `append` equals `build(window)`, checked for every level.
  - `view.shift`: playhead, view and selection clamping, including a selection that is fully pruned. Pure function, so test it with vitest if it is set up; otherwise as a Rust-mirrored helper, or note it as a manual check.
  - Coalescing of update requests.
- **Watcher integration test** (`#[ignore]`, `STEAMREPLAY_SESSION_DIR`):
  - copy `session.mpd`, the inits and 2 chunks into a tempdir, start the watcher, then copy in one chunk every 3 s,
  - expect `session-updated` events with a rising `last`,
  - then delete chunk 1 and raise `startNumber`, and expect `timeShiftS = 3`.
- **Manual:**
  - start a game with background recording, open the live session and watch for 5 minutes with follow mode,
  - press the Steam marker key and check that the marker appears,
  - close the game,
  - simulate a prune with a copied folder.

## 8. Open questions / risks
- **Whether `notify` events for `session.mpd` arrive reliably.** Steam may rewrite the file with replace-by-rename. Watching the folder (not the file) covers that. Check this in step 1, before building the rest.
- **The incremental bin placement in `audio.rs` uses `probe_start` of the first chunk as `v0`.** Appending at nominal positions assumes no drift. The property test and a comparison with a full re-analysis on real data will show whether it holds over 1 h.
- **Preview `extend` in WebKitGTK MSE:** raising `duration` on an open MediaSource must not trigger the append-order quirks from M4 (`04-preview-playback.md` §4). Test this early. If it is unreliable, reload the player when it reaches the old end.
- **Cost of a full rebuild on prune** for a 5 h buffer (1.8 M bins) is probably < 100 ms, and it is rare. Measure once.
