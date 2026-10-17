# M13: Volume control and hover frames

## 1. Goal and user-visible outcome
- **Volume:** the transport bar gets a mute button and a volume slider. `M` mutes and unmutes. The level and the mute state are remembered on this machine. Before M13 the preview always played at full volume, which hurts in an app that hunts for shouting and laughing.
- **Hover frames:** hovering the timeline (waveform, ruler or overview strip) shows a small card with a frame of the video at that point, the timecode, and the marker or loud-moment text when there is one. While you drag a selection edge, the card shows the frame at that edge. This lets you find a moment without seeking around.

## 2. Prerequisites
- M9 (transport, timeline, `state/ui.ts` prefs), M11 (preview sessions, the `replay://` protocol).
- `00-overview.md`: cross-cutting rules. Every Steam chunk is 3 s and starts on a keyframe (`startWithSAP=1`).

## 3. Scope / non-goals
**In scope:** the mute button, volume slider and `M` shortcut; the `/frame/` route in `replay://`; the hover card in the timeline.

**Non-goals:**
- Volume-step keys (the arrows already seek).
- Frame-exact hover images. The card shows the first frame of the 3 s chunk under the pointer; a click still seeks to the exact frame.
- A pre-generated storyboard for the whole recording.
- Hover frames in the loudness lane.
- Automatic tuning of loud-moment detection (discussed and deferred).

## 4. Design

### Volume (frontend only)
- `state/ui.ts` `Prefs` gains `volume` (0–1, default 1) and `muted` (default false). They are saved in `localStorage` like the other prefs. Old stored prefs get the defaults.
- `Player.tsx` applies them to the `<video>`: `volume²` (a linear slider spends most of its travel on barely audible changes) and `muted`. These are properties of the element, so they survive a new `MsePlayer` (fallback, Retry).
- `Transport.tsx` `Volume`:
  - The mute button's icon follows the level (`VolumeX` / `Volume1` / `Volume2`).
  - The slider runs 0–100. Moving it unmutes, and the mouse wheel over it changes the level by 5 %.
  - Unmuting at level 0 comes back at 50 %.
  - Below 1100 px window width only the button shows.
- Shortcut `mute` = `M` (Playback group), so the `?` overlay and the tooltip list it.

### Hover frames
**Backend** (`media/thumbs.rs`, `media/preview/protocol.rs`, `commands::hover_frame`):
- `replay://localhost/frame/<urlencoded id>/<k>.jpg`, where `k` is the chunk index from video time 0 (`floor(t / chunkS)`). It has its own `/frame/` prefix, because `/thumb/<id>.jpg` would also accept an id that ends in `/<k>`.
- The route needs an **open** preview session (`PreviewState::get`), which holds the session folder, the chunk inventory and the ffmpeg paths. Without one it returns 404.
- `k` → chunk `inv.first + k`. A missing chunk, or one past the end, uses the nearest chunk on disk (`nearest_chunk`).
- The frame is `init + chunk → ffmpeg -frames:v 1 -vf scale=-2:144 → mjpeg`. This is the same extraction as the library thumbnail (`extract_jpeg`, shared). It is scaled by height because recordings can be 32:9: 5120×1440 gives 512×144, about 10–20 KB.
- **Cache:** `<cache>/steamreplay/thumbs/frames/<cache_stem(id)>/<chunk>.jpg`, written through a `.tmp` file.
  - Chunk numbers don't change when Steam prunes the buffer, so the cache never goes stale.
  - *Settings → Storage* counts and clears it with the thumbnails.
- At most one frame is extracted at a time (`FRAME` semaphore). It has its own slot, so the library's thumbnail queue never delays a hover frame.

**Measured** (release ffmpeg, software HEVC decode, 16 threads):

| Case | Time |
|---|---|
| 5K chunk (3.5–11.6 MB), `bg_1445790_20261015_210120` | 231–255 ms per cold frame |
| Pruned session `bg_1623730_20261010_164846` (`frame_fixture`) | 183 ms cold, 21 µs cached |

No `-hwaccel` is needed.

**Frontend** (`lib/hoverFrames.ts`, `Timeline.tsx`):
- `useHoverFrame(id, t, chunkS)` keeps one `FrameLoader` for the open session:
  - an LRU of object URLs (up to 300 frames),
  - one fetch at a time plus one pending request (the latest wins), with a 60 ms debounce,
  - a failed frame is retried after 5 s.
- Until frame `k` arrives, the closest loaded frame is shown dimmed. Opening another session drops the old loader and revokes its URLs.
- `Timeline` tracks the hovered time:
  - main area and ruler: `timeAtX`,
  - overview strip: the whole-recording scale,
  - while dragging a selection edge: the edge.

  Panning, dragging the overview window and the wheel hide the card.
- The card replaces the old text tip. It sits above the pointer (below it near the top), centred and clamped inside the timeline, because `.timeline` clips its overflow. The image is 72 px high (the 144 px source covers HiDPI).

## 5. Edge cases
- **Preview not open yet, or the open failed:** the route returns 404, so the card shows only the timecode and text. Frames load once the preview is open (retry after 5 s).
- **Missing chunk / pruned buffer:** the nearest chunk on disk is used, and `first` comes from the inventory as for playback.
- **Clip sessions:** the same layout (`clips/clip_*/video/bg_*`), so they work unchanged.
- **Empty recording:** the timeline is not shown (M9 empty state).
- **Volume while a slider has focus:** the focused slider owns the arrow keys (`widgetOwns`), as with every other slider.

## 6. Acceptance criteria
- [x] Frame extraction: `parse_frame_path`, `nearest_chunk` unit tests; `frame_fixture` on the pruned session (cold < 600 ms).
- [x] clippy, tests, tsc and lint are clean; `bindings.ts` unchanged (no new commands).
- [ ] In the app (Linux):
  - the slider changes the loudness, `M` mutes, and both survive a restart;
  - the volume applies in the 720p fallback;
  - hovering shows frames, at once on a second pass;
  - dragging an edge shows the edge's frame;
  - *Clear caches* removes the frames.
- [ ] Windows (teammate): the volume control, and hover frames over `http://replay.localhost/frame/…`.

## 7. Verification steps
- **Unit tests:** `thumbs::tests::paths` (frame paths: no empty id parts, no negative or non-numeric index), `thumbs::tests::nearest`.
- **Integration test:** `STEAMREPLAY_SESSION_DIR=<…/video/bg_…> cargo test frame_fixture -- --ignored --nocapture` prints the cold and cached times and checks the past-the-end fallback.
- **Manual** (`pnpm tauri dev`, `bg_1445790_20261015_210120`):
  - Move the volume and press `M`, then restart.
  - Set "Always 720p preview" and check that the volume still applies.
  - Sweep the waveform and the overview strip.
  - Shift-drag a selection.
  - Watch *Settings → Storage → Thumbnails* grow, then clear it.

## 8. Open questions / risks
- **3 s granularity.** At high zoom the image changes only every 3 s. If that confuses people, show the chunk's time range in the card, or decode the exact frame (`-ss` inside the chunk, about 3× the cost) when the view spans less than about 30 s.
- **Slower machines:** about 0.25 s per cold frame on a 16-thread CPU. A 4-core laptop may take about 1 s. The dimmed nearest frame hides some of that. If it's not enough, use NVDEC (`-hwaccel cuda`) for the extraction, as the 720p transcode does.
- **Cache size:** frames have no size limit (about 15 KB × 1200 chunks ≈ 18 MB for a fully swept 1 h recording). Clearing is manual, like the other thumbnails.
