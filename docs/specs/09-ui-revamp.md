# M9: UI/UX revamp (editor workspace)

## 1. Goal and user-visible outcome
SteamReplay looks and works like a small video editor:
- **Layout:** a recordings library on the left, a large video in the centre, a tabbed inspector (Moments / Export / Info) on the right and the timeline across the bottom. The page itself never scrolls; each panel scrolls on its own and can be resized.
- **Faster start:** the video plays as soon as a recording opens. The audio analysis runs alongside it and fills in the timeline when done.
- **Library:** thumbnails, search, a kind filter and collapsible game groups. The last recording reopens at start.
- **Exports:** keep running when you switch recordings. A top-bar Exports button shows the queue and the recent exports from anywhere. A finished export shows a toast with "Show in folder".
- **Look:** dark first, light when the system is light, with a manual override. Consistent icons, tooltips that show the shortcut, and real empty, loading and error states.
- **Settings:** a sectioned dialog with a side nav.

## 2. Prerequisites
- M1–M8 done.
- `00-overview.md`: cross-cutting rules (Windows first, no Linux-only behaviour).
- `04-preview-playback.md`: the `replay://` protocol that the thumbnails reuse.
- `06-spike-tuning-settings.md`: the tuning panel, the settings sections and the shortcut overlay, which are reworked here.

## 3. Scope / non-goals
**In scope:**
- design tokens and themes,
- UI primitives on Radix plus lucide icons,
- one central shortcut registry,
- a toast store,
- the shell layout with splitters,
- the library with thumbnails (backend route),
- the editor (stage, transport, timeline panel),
- the inspector,
- a global export queue,
- the settings rework.

**Non-goals:**
- changes to detection or export behaviour,
- new export formats,
- user-defined key bindings,
- multiple windows,
- a WebGL timeline,
- i18n.

## 4. Design

### 4.1 Dependencies
- `radix-ui`: the unified package for Dialog, Tabs, Tooltip, Popover, Slider, ToggleGroup, DropdownMenu, Collapsible and RadioGroup.
- `lucide-react`: icons, tree-shaken.
- Both are frontend only.

### 4.2 Design system (`src/styles/`)
- **`tokens.css`:**
  - Colours. Dark is the default on `:root`. Light is applied by `@media (prefers-color-scheme: light) { :root:not([data-theme="dark"]) {…} }` and by `:root[data-theme="light"]`.
  - The canvas token names stay the same (`--wave`, `--wave-rms`, `--spike`, `--playhead`, `--selection`, `--accent`, `--clip`, `--warn`, `--muted`, `--border`, `--panel`, `--text`), because `Timeline` and `LoudnessLane` read them with `getComputedStyle`.
  - New: surfaces `--bg`, `--surface-1..3`, `--focus`, spacing, radii, type sizes, shadows.
- **The other style files:** `base.css` (reset, focus rings, scrollbars), `ui.css` (primitives), `layout.css` (shell and features). `App.css` is removed.
- **Theme:**
  - The `ui` store keeps it as `system | dark | light`. `main.tsx` applies `data-theme` before the first render.
  - The canvases redraw when the store's theme or the system scheme changes.

### 4.3 UI state (`src/state/ui.ts`)
- A zustand store, persisted to `localStorage` with try/catch (it can throw or be empty; the defaults must work). It holds:
  - the theme,
  - the library width and whether it is collapsed,
  - the inspector width,
  - the timeline height,
  - the inspector tab,
  - the last session id,
  - the collapsed game groups,
  - whether the detection section is open,
  - the loudness lane on/off.

### 4.4 Primitives (`src/components/ui/`)
- Thin, styled wrappers: `Button` (primary, secondary, ghost, danger; sm/md), `IconButton` (needs a `label`; tooltip), `Tooltip` (with an optional shortcut), `Dialog`, `Tabs`, `Slider`, `Segmented`, `Popover`, `Field`, `Badge`, `Kbd`, `Progress`, `Spinner`, `EmptyState`, `Splitter`.
- `Splitter` is a pointer-drag handle, `role="separator"`, and can also be moved with the arrow keys.

### 4.5 Shortcuts (`src/lib/shortcuts.ts`)
- **Registry:** a `SHORTCUTS` list (`id`, `keys` label, `action`, `group`). The `?` overlay and the tooltips render from it.
- **Handlers:** `useShortcut(id, handler, enabled = true)` registers a handler. One global `keydown` listener matches `e.key` and modifiers through a `match` function per id. Before dispatching, it applies:
  - `isTyping(target)` (input, textarea, select, contenteditable),
  - an open dialog: `dialog[open]` **or** `[role="dialog"][data-state="open"]` (Radix),
  - a focused interactive widget (`button`, `[role=slider|tab|menuitem|option|radio|switch|separator]`, `summary`): plain arrows, Space and Enter belong to the widget, not to the player.
- A handler returns `false` when it did not act, so the event is not prevented. Example: Esc with no selection.
- **New keys:**

| Key | Action |
|---|---|
| `Ctrl+E` | Export the selection |
| `Ctrl+,` | Settings |
| `Ctrl+B` | Show / hide the library |
| `+` / `-` | Zoom in / out around the playhead |
| `0` | Show the whole session |
| `Z` | Zoom to the selection |
| `F` | Follow the playhead on/off |

### 4.6 Toasts (`src/state/toasts.ts`)
- **API:** `toast.error(e)`, `toast.success(text, actions?)`, `toast.info(text)`.
- **Lifetime:** errors 8 s, others 5 s. The timer pauses while hovered. Every toast has a close button; errors also get "Copy".
- **Replaces** `showError` and the `onError` props.
- `UpdatePrompt` stays a persistent toast at the top of the stack.

### 4.7 Shell
- **Window** (`tauri.conf.json`): default 1440×900, minimum 1100×680.
- **Top bar:**
  - library toggle, app name, breadcrumb (Game · date · length)
  - spacer
  - Exports button: a count badge and a progress ring; it opens a popover with the queue and recent exports
  - Refresh
  - Shortcuts (`?`)
  - Settings
- **Workspace grid:** `library | stage | inspector`, with the timeline below stage and inspector. The library runs full height.
  - Splitters set the library width (220–480), the inspector width (300–560) and the timeline height (160–50 % of the window).
  - **No session:** the stage says "Pick a recording" and shows the main shortcuts.
  - **No recordings at all:** the onboarding state takes the stage (checked roots, "Choose folder…").

### 4.8 Library (`src/components/library/`)
- **Controls:**
  - a search over the game name,
  - a kind filter (All / Background / Clips),
  - game groups that collapse (persisted) and show the count and total size.
- **Card:**
  - a 16:9 thumbnail (96×54),
  - a relative date (`formatRelativeDate`: "Today 21:01", "Yesterday 18:30", otherwise a locale date and time),
  - length, markers and size,
  - a Clip badge,
  - an "empty" badge for sessions of length 0.
- **Keyboard:** `role="listbox"` with roving focus. ↑/↓/Home/End move and select.
- **Start:** the last session id is reselected when it still exists. Skeleton cards show while the first list loads.

### 4.9 Thumbnails (backend)
- **Route:** `replay://localhost/thumb/<urlencoded id>.jpg` (Windows: `http://replay.localhost/thumb/…`), handled in `media/preview/protocol.rs` next to `/preview/`.
- **Implementation:** `media/thumbs.rs`.
  - **Clip:** the session dir is `clips/clip_*/video/bg_*/`. Walk up to `clip_*` and serve its `thumbnail.jpg` when present.
  - **Background:**
    - Take the middle chunk of the video stream (`chunks.first..=last`). Pipe `init-stream0.m4s` plus that chunk into ffmpeg (`-f mp4 -i pipe:0 -frames:v 1 -vf scale=320:-2 -q:v 5`), and save the JPEG.
    - Cache it as `<cache>/thumbs/<fnv(id)>-<chunk>.jpg`. Write to a temp name and rename, so a half-written file is never served.
  - **Limits:** at most 2 extractions at once (a `tokio::sync::Semaphore`).
  - **Failures:** 404 with a message. The frontend shows a placeholder icon.
  - **Frontend:** `Thumbnail` fetches the URL with `fetch` once the card scrolls into view (IntersectionObserver) and shows it as a blob URL. A plain `<img src="replay://…">` is never requested by WebKitGTK (checked in `tauri dev`), while `fetch` works as it does for the preview segments.
- **Caches:** `CacheSizes` gains `thumbs`, and `clear_caches` empties `thumbs/`.

### 4.10 Editor (`src/components/editor/`)
- **`useAnalysis(detail)`:** the logic of the old `WaveformPanel` (analysis job, params, throttled re-detect, debounced save, reload on settings, reset), moved as is. It returns status, tiles, params, spikes and the actions.
- **Player first:**
  - Opening a session calls `useView.reset(summary.durationS)` right away, so the player can seek before the analysis finishes.
  - When `analyzeAudio` returns, `setDuration(meta.durationS)` corrects the duration and keeps the playhead, the selection and the view (the view is clamped).
- **Stage:** the video fills the space (`object-fit: contain`). Overlays for preparing, loading and failed come with an icon and Retry. Retry re-opens the preview.
- **Transport:**
  - play/pause, frame ±1, ±5 s
  - `current / duration` timecode
  - rate pill
  - In / Out buttons, a selection readout, Loop, Play selection, Clear
  - every button gets a tooltip with its key
- **Timeline panel:**
  - **Toolbar:** zoom −/+, Fit, Zoom to selection, Follow, Loudness lane, and in Fast mode a note "Fast export cuts on 3 s chunks".
  - **While the analysis runs:** progress and Cancel take the timeline's place. **When it fails or is cancelled:** "Analyse again".
  - **`Timeline.tsx`:** keeps its canvas code. The main waveform height comes from the panel height (a ResizeObserver on both sides) instead of the constant 150 px.

### 4.11 Inspector (`src/components/inspector/`)
- **Moments tab:**
  - **Detection section:** Sensitivity, the count, "recalculating…", "Saved for <game>", Reset and Advanced. It can collapse.
  - **One list** of loud moments and Steam markers.
    - Filter: All / Loud / Markers. Sort: Rank / Time.
    - Rows have an icon, the time, a length, a score bar for loud moments and a title for markers. Hover actions: Select (sets in/out) and Play.
    - Clicking a loud row selects it, seeks and zooms (same as before). Clicking a marker seeks.
    - N/P/Shift+N/Shift+P/S stay as they are.
  - "N markers outside the buffer" as a footer.
- **Export tab:**
  - **Selection card:** in, out and length. In Fast mode also the snapped cut and the size estimate.
  - **Mode:** a Segmented control with a visible one-line explanation.
  - **Presets:** radio cards (Discord 10 MB / Nitro 500 MB / Original / Custom), then an Advanced section.
  - **Output:** a path field with Browse.
  - **Export button:** a full-width primary button (Ctrl+E), with inline warnings (too long for the size, codec missing).
  - This session's jobs.
- **Info tab:**
  - the recording facts (folder with Open, account, app id, video, audio, segments, chunks, gaps, offset, timeline file),
  - the preview backend,
  - gaps as a warning callout.

### 4.12 Global export queue (`src/state/exports.ts`)
- **Store:** the job list and the sequential runner move from `ExportPanel` into a zustand store with `enqueue(job)`, `cancel(jobId)` and `clearFinished()`.
- **Each job** carries its `sessionId` and `gameName`.
- **Session switches** no longer cancel exports.
- **`busy`:** a selector; `lib/updater.ts` waits on it. `state/exportBusy.ts` goes away.
- **Recent exports:** a `recentSeq` in the store is bumped after each finished export, so the recent list reloads.
- **Finished export:** a success toast with "Show in folder".
- **Popover:** the top-bar Exports popover lists all jobs (newest first) and the recent exports.

### 4.13 Settings (`src/components/settings/`)
- **Dialog:** a Radix Dialog (about 860×620) with a side nav.
- **Sections:**
  - General: theme, recordings folder
  - Export
  - Loud moments
  - Video preview
  - ffmpeg
  - Storage: now with thumbnails
  - About: version, updates, diagnostics, shortcuts
- **Logic:** the section logic from M6/M8 is kept, one file per section, laid out with `Field` rows.
- **On close:** `bumpSettings()` as before.

## 5. Edge cases
- **Session switch during analysis:** the old job is cancelled (as before). The player of the new session starts at once.
- **The analysis duration differs from the summary duration** (the chunk inventory vs the decoded audio): `setDuration` clamps the playhead, the selection and the view.
- **A session of length 0:** no player, no timeline. The stage shows an empty state, and the Info tab still works.
- **A deleted last-session id:** nothing is selected.
- **Thumbnails:**
  - for a session with no video chunk, or when ffmpeg is missing: the placeholder;
  - for a live session whose chunk vanishes: 404, and the next open picks another chunk;
  - for a clip without `thumbnail.jpg`: use the background path.
- **Keyboard:**
  - Space on a focused button must not both click and play. A focused button keeps Space and Enter, and the shortcut layer ignores them there.
  - Arrows on a focused slider move the slider only.
  - Shortcuts are paused while a Radix popover or menu has focus inside it.
- **Exports:** a job finishes after its session was closed: the toast still shows, and the recent list updates.
- **`localStorage` unavailable:** the defaults are used and nothing throws.
- **Narrow window (1100 px):** the library can collapse. The inspector keeps at least 300 px, and the stage at least 360 px.

## 6. Acceptance criteria
- [x] No page scroll at 1100×680 and at 2560×1440. Each panel scrolls on its own.
- [x] Opening a 1 h session: the video plays within the preview start time, while the timeline shows the analysis progress.
- [ ] Every shortcut in the `?` overlay works. Arrows on a focused slider do not seek. Space on a focused button does not toggle play twice.
- [ ] An export keeps running after switching sessions and shows in the Exports popover. The success toast's "Show in folder" works.
- [x] Thumbnails show for clips and background recordings. Clear caches removes them, and they are rebuilt.
- [x] Theme system, dark and light all work. The canvases recolour without a reload.
- [ ] The splitter sizes, the library collapse, the inspector tab and the last session survive a restart.
- [ ] **Windows:** all of the above on a teammate machine (WebView2), including thumbnails over `http://replay.localhost`.
- [ ] clippy, tests, tsc and lint are clean; CI is green.

## 7. Verification steps
- **Unit tests:** `protocol.rs` thumb path parsing; `thumbs.rs` clip thumbnail lookup and the middle-chunk choice.
- **Manual (Linux, done 2026-10-17):** `pnpm tauri dev` on the dev machine's sessions: the library with thumbnails, the editor at 1440×900 and 1100×680, N/S/Ctrl+E (an NVENC precise export with the success toast and the Exports popover), settings, the light theme, Ctrl+B, Space playback, and opening a 2:52 h session after clearing the caches (the video and markers are usable while the analysis runs).
- **Still to do by hand:**
  - pointer interactions that the automated run could not drive (splitter drags, timeline Shift+drag, hover tooltips),
  - the keyboard checks of §6 with a real keyboard (focused slider, Space on a clicked button),
  - Windows (`docs/windows-test.md` steps 11–13).

## 8. Open questions / risks
- **Radix in WebKitGTK:** portals, pointer capture and focus management. Test tooltips, popovers and sliders early in the Linux build.
- **Thumbnail decode cost:** about 0.2–0.5 s per 5K HEVC keyframe in software. Lazy images, a semaphore of 2 and the disk cache keep it a one-time cost.
- **Exports that outlive their session:** the backend owns the ffmpeg child processes, so closing the app must still end them. Check this on Windows.
