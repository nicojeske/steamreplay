# M6: Spike tuning, settings page and UX extras

## 1. Goal and user-visible outcome
- **Spike tuning:** a panel under the waveform with sliders for loud-part detection. The spike bands and the spike list update live while you drag, in under 200 ms for a 1 h session. Settings are remembered **per game**, and a reset brings back the defaults.
- **Settings page:** the existing settings dialog gains export defaults, spike defaults and one "Clear caches" button.
- **UX extras:** `?` opens a keyboard-shortcut overlay. The export panel shows a list of recent exports that survives restarts.

## 2. Prerequisites
- M1–M5 done. M5 adds the export modes and presets that the settings page uses as defaults.
- `02-waveform-spikes.md` §4: the detection algorithm and `SpikeParams`.
- `00-overview.md`: cross-cutting rules.

## 3. Scope / non-goals
**In scope:**
- tuning panel and per-game presets,
- a faster re-detect path,
- optional loudness lane,
- settings sections,
- clearing caches,
- shortcut overlay,
- recent exports.

**Non-goals:**
- new detection algorithms (laughter classifiers, ML),
- importing or exporting presets as files,
- user-defined key bindings,
- live sessions (not planned).

## 4. Design

### 4.1 Current state
- `SpikeParams` (`media/spikes.rs:19-36`) has `short_ms`, `baseline_s`, `threshold_db`, `min_len_ms`, `merge_gap_s`, `pad_s` and `max_results`. The defaults are at `:38-50`.
- `detect(level0, p)` (`spikes.rs:146`) works in four steps:
  1. dB conversion,
  2. a centred mean over `short_ms` (`st`),
  3. a sliding-histogram median over `baseline_s` (`base`),
  4. regions where `st − base > threshold`, followed by min length, merge, pad, score and sort.
- `get_spikes(id, params: Option<SpikeParams>)` (`commands.rs:249`, already wrapped in `blocking`) exists, but `WaveformPanel.tsx:36` always passes `null`.

### 4.2 Faster re-detect (backend)
Split `detect` into two stages:
```rust
pub struct Curves { st: Vec<f32>, base: Vec<Option<f32>>, include: Vec<bool> }  // per level-0 bin, dB; include = not digital silence
pub fn curves(level0: &[Bin], short_ms: u32, baseline_s: f32) -> Curves;   // expensive
pub fn regions(c: &Curves, p: &SpikeParams) -> Vec<Spike>;                // cheap, O(n)
pub fn detect(level0, p) -> Vec<Spike> { regions(&curves(level0, p.short_ms, p.baseline_s), p) }
```
- `Analysis` (`media/mod.rs:34`) gets `curves: Mutex<Option<((u32, u32), Arc<Curves>)>>`, keyed by `(short_ms, baseline_ms)`. `get_spikes` reuses the curves when the key matches. Changing the threshold, min length, merge gap or pad then only runs `regions`.
- Run `get_spikes` in `spawn_blocking`, so a 1 h baseline rebuild does not block the async runtime.
- Measure `curves` and `regions` on the 1 h sample (about 372 k bins). If dragging the baseline slider takes longer than 200 ms, recompute only when the slider is released (`onChange` commit), and say so in the UI.
- **Measured** (release build, `bg_1445790_20261015_210120`, 372 000 bins):

  | Baseline | `curves` | `regions` |
  |---|---|---|
  | 5 s | 24 ms | 0.40 ms |
  | 30 s | 23 ms | 0.45 ms |
  | 120 s | 23 ms | 0.45 ms |

  The sliding-histogram median does not depend on the window length. **Decided:** every slider, the baseline included, updates live, so no apply-on-release fallback is needed. A "recalculating…" note shows while a request runs.

### 4.3 Presets (backend)
- Store keys in `settings.json` (`read_setting`/`write_setting`, `commands.rs:311-327`):
  - `spikeDefaults: SpikeParams` (absent = the built-in `Default`),
  - `spikePresets: { [appId: string]: SpikeParams }`.
- Commands:
  ```rust
  #[tauri::command] fn get_spike_params(app_id: u32) -> SpikeParams;                  // preset → defaults → built-in
  #[tauri::command] fn set_spike_params(app_id: u32, params: Option<SpikeParams>);    // None = reset to defaults
  #[tauri::command] fn get_spike_defaults() -> SpikeParams;
  #[tauri::command] fn set_spike_defaults(params: Option<SpikeParams>);               // None = built-in
  #[tauri::command] fn clear_spike_presets();
  ```
- `SessionSummary.app_id` (`steam/recordings.rs:37`) supplies the app id. Clips use the same app id as sessions, so they share the preset.

### 4.4 Tuning panel (frontend)
- A new `src/components/SpikeTuning.tsx`, opened with a "Tune" toggle in the spike list header. It is collapsed by default, and its open state is kept in `localStorage`.
- **Sensitivity** (main slider, 0–100) maps linearly to `threshold_db` from 20 dB (0) to 4 dB (100), with the default of 10 dB at about 62.
- "Advanced" sliders:

| Param | Range | Step |
|---|---|---|
| `threshold_db` | 3–25 dB | 0.5 |
| `baseline_s` | 5–120 s | 5 |
| `min_len_ms` | 100–2000 ms | 50 |
| `merge_gap_s` | 0–20 s | 0.5 |
| `pad_s` | 0–5 s | 0.25 |

- A change calls `getSpikes(id, params)`, throttled to one request every 150 ms with the last value always sent. A trailing debounce would not update while the slider is being dragged. Responses that arrive out of order are dropped with a request counter. Then the spike bands and the list are redrawn.
- On session load, call `getSpikeParams(appId)`. Save with `setSpikeParams` 1 s after the last change. "Reset" calls `setSpikeParams(appId, null)` and reloads.
- The panel shows the spike count, for example "37 loud parts".
- **Optional loudness lane** (do it last, skip it if time is short):
  - a 40 px canvas under the waveform that draws `st − base` and a horizontal threshold line,
  - served by `get_loudness(id, level, start_bin, count) -> Vec<f32>`, which takes the min/max of the cached curves over the visible bins, like `get_waveform`,
  - toggled from the tuning panel.

### 4.5 Settings page (`src/components/SettingsDialog.tsx`)
Today it has: recordings folder override, ffmpeg, video preview (`PreviewSection`), detected Steam folders, accounts, and copy diagnostics. Add:
- **Export:**
  - Default folder override (`exportDirOverride`). `export_dir` (`commands.rs:516`) checks it first, before the account's `ExportDirectory`.
  - "Mode and preset for the next export". **Decided:** there are no separate default keys. This section edits the same `exportMode`/`exportPrecise` keys that M5 saves on every export (`get_export_prefs`/`set_export_prefs`, `commands.rs:654-663`). A separate default would never apply, because the last-used choice is saved anyway. If the ExportPanel is mounted, it reloads the prefs when the dialog closes.
- **Loud parts:** "Default sensitivity" plus a "Reset all game presets" button, which clears `spikePresets` (`set_spike_defaults` plus a `clear_spike_presets` command).
- **Storage:** shows the cache sizes (waveform, preview, export) and has one **Clear caches** button. A new `clear_caches()` command:
  - deletes `app_cache_dir/steamreplay/{waveform,preview,export}`, skipping files that are in use by a running job,
  - empties the in-memory `MediaState.loaded` LRU,
  - and does the same as `clear_preview_cache`.
  - It skips the `export/<jobId>` folders of exports that are still running (live job ids in `MediaState`) and returns `{ skipped: u32 }`, so the UI can say "some files were in use".
  `get_cache_sizes()` reports the sizes.
- Group the sections with headings in this order: Recordings, Export, Loud parts, Preview, ffmpeg, Storage, Diagnostics.

### 4.6 Shortcut overlay
- Move the shortcut definitions into data in `src/lib/keys.ts`:
  ```ts
  export const SHORTCUTS: { keys: string; action: string; group: "Playback" | "Navigation" | "Selection" | "General" }[]
  ```
  This covers every key handled today:
  - `Player.tsx:133-187`: Space, K, L, J, ←/→, Shift+←/→, `,` `.`
  - `SpikeList.tsx:30-54`: S, N/P, Shift+N/P
  - `ExportPanel.tsx:66-90`: I, O, Ctrl+L, Esc
  - plus `?` itself.
  The handlers stay where they are. The list is documentation only, and a comment at each handler points to it.
- `?` (when not typing, `isTyping()`) opens `ShortcutsOverlay.tsx`, a native `<dialog>` like `SettingsDialog`, with one table per group. Esc closes it. A "Keyboard shortcuts" button in the settings dialog opens it too.

### 4.7 Recent exports
- When an export finishes, append `{ path, gameName, inS, outS, mode, sizeBytes, finishedAt }` to the store key `recentExports` (keep the newest 20). **Decided:** do it inside `export_clip` on success (`commands.rs:558`), using a pure `push_recent(list, entry)` that keeps the newest 20. A failure to save is only logged, so the export never fails because of it. The commands are `get_recent_exports() -> Vec<{ entry, exists: bool }>` and `remove_recent_export(path)`.
- In `ExportPanel.tsx`, a collapsible "Recent exports" list under the job list, with "Show in folder" and "Copy path" (pull the buttons out of `JobRow`, `ExportPanel.tsx:450`, into a shared `PathActions`). Entries whose file no longer exists are shown greyed out, with a "Remove" action. The backend sets `exists`, because there is no fs plugin in `capabilities/default.json`; do not add one.

## 5. Edge cases
- A preset saved by an older version that has missing or extra fields: the stored JSON is merged over the defaults (`spikes::params_from_json`). `#[serde(default)]` is not used, because it would make every field optional in the generated TS type.
- Slider values outside the valid range (a hand-edited store): clamp them in `get_spike_params`.
- A session with no audio analysis yet: the tuning panel is disabled until `analyze_audio` has finished.
- `max_results` is reached: the panel shows "showing top 200".
- Clear caches while a preview transcode or an export runs: skip locked files (Windows) and report "some files were in use".
- A recent export on a drive that is no longer connected: shown greyed out, with no error toast.
- `?` on keyboard layouts where it needs Shift (German QWERTZ: Shift+ß): match `e.key === "?"`, not the key code.

## 6. Acceptance criteria
- [ ] Moving the sensitivity slider updates the spike bands in < 200 ms on `bg_1445790_20261015_210120` (1 h). Measure from input to redraw, logged in dev.
- [ ] Changing the baseline is either < 200 ms, or applied on release with a visible "recalculating" state.
- [ ] Tuning one game and opening another game's session shows that game's own preset or the defaults. Reopening the first game restores its values after an app restart.
- [ ] Reset restores the defaults.
- [ ] Changing the export default folder in settings changes the default export path.
- [ ] Clear caches brings the shown sizes to about 0. Waveforms and previews are rebuilt when opened again.
- [ ] `?` shows every shortcut that the app actually handles (checked against the three handlers).
- [ ] Recent exports survive a restart, and "Show in folder" works.
- [ ] **Windows:** all of the above on a teammate machine, including `?` on a German keyboard.
- [ ] clippy, tests, tsc and lint are clean; CI is green.

## 7. Verification steps
- **Unit tests:**
  - `regions(curves(x)) == detect(x)` (old implementation kept in the test as reference) on random and fixture data,
  - curve-cache key reuse,
  - `get_spike_params` precedence (preset → defaults → built-in) and clamping, through a pure `resolve_spike_params(preset, defaults)` helper and `SpikeParams::clamped()` (also clamp `short_ms` 50–2000 and `max_results` 1–1000),
  - `recentExports` trimming to 20.
- **Bench** (`#[ignore]`, `STEAMREPLAY_SESSION_DIR`): time `curves` and `regions` on the 1 h session, and print the numbers into this spec.
- **Manual:**
  - tune on two games,
  - restart and check the presets,
  - clear caches,
  - check `?`,
  - export three clips and check the recent list,
  - delete one file and check the greyed-out state.

## 8. Open questions / risks
- The sliding-histogram median over 120 s might be too slow to recompute live. That is the reason for the "apply on release" fallback.
- The sensitivity → threshold mapping is a guess. Tune it with real sessions from teammates, and ask what sensitivity they end up using.
- Per-game presets assume that the loudness character depends on the game. If it depends more on who is in voice chat, a "per account" scope might be better. Ask the team after a week of use.
