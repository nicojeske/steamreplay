# M5: Precise export (frame-accurate re-encode)

## 1. Goal and user-visible outcome
The export panel has a **Fast / Precise** switch. Fast is the M3 stream copy, snapped to 3 s chunks. Precise cuts on the exact in and out frames and re-encodes. It can produce H.264 (or HEVC/AV1) with a downscale, an optional 16:9 centre crop and a size limit, so clips can be posted in Discord and play in Windows Films & TV without the HEVC extension. Presets cover the common cases: "Discord (10 MB)", "Discord Nitro (500 MB)" and "Original quality".

## 2. Prerequisites
- M1–M4 done. M3 (`03-export-fast.md`) is the base: snapshot, join, job folder, `.part` rename, job queue in the panel.
- M4 (`04-preview-playback.md`): backend detection (`Backend`, `preview_backend()`), which picks the GPU vendor family.
- `00-overview.md`: cross-cutting rules (long jobs, Steam files read-only, absolute paths, Windows first).

## 3. Scope / non-goals
**In scope:**
- precise mode in `media/export.rs`,
- encoder capability detection (`media/encoders.rs`),
- the export panel UI (mode, presets, advanced options),
- remembering the last mode and preset.

**Non-goals:**
- two-pass encoding,
- burned-in captions or overlays,
- joining several selections into one file,
- a settings-page section for export defaults (that is M6; this milestone only remembers the last used choice),
- audio-only export.

## 4. Design

### 4.1 Pipeline (`media/export.rs`)
`export::export` (`export.rs:174`) already does three steps: snapshot (0–0.5 progress), join into `job_dir/v.mp4` + `a.mp4`, then mux (`:213-227`). Split it at the mux step:
- `snapshot_and_join(...)`: today's steps 1–2, unchanged (`Cleanup`, `snapshot_file`, `join`, `part_path`, `dedupe` are reused as they are).
- `mux_copy(...)`: today's step 3 (Fast).
- `encode(...)`: new (Precise).

Request and result types:
```rust
pub struct ExportRequest { id: String, in_s: f64, out_s: f64, output: Option<PathBuf>, mode: ExportMode }
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportMode { Fast, Precise(PreciseOpts) }
pub struct PreciseOpts {
    codec: VideoCodec,          // H264 | Hevc | Av1 (Av1 only if an encoder exists)
    max_height: Option<u32>,    // 1080, 720, …; keeps aspect, never upscales
    fps: Option<u32>,           // 60 | 30; None = source
    crop_16_9: bool,            // centre crop for sources wider than 16:9
    target: Quality,            // Crf(u8) | MaxSizeMb(u32)
}
pub struct ExportResult { path, actual_in_s, actual_out_s, size_bytes, encoder: String, fell_back: bool }
```
- Fast: `actual_*` stay the snapped times, and `encoder` is `"copy"`.
- Precise: `actual_*` are the exact `in_s`/`out_s` (clamped to the session).
- `export_snap` stays as it is. The panel only uses it for Fast.

**Encode command** (inputs are the joined files, which start at `snap.actual_in_s`):
```
ffmpeg -hide_banner -nostdin -v error -progress pipe:2 -stats_period 0.5
       [-hwaccel cuda|d3d11va|qsv|vaapi]  -i v.mp4  -i a.mp4
       -ss <in_s − snap.actual_in_s>  -t <out_s − in_s>
       -map 0:v:0 -map 1:a:0
       -vf "[crop=ih*16/9:ih,]scale=-2:<H>:out_range=tv,format=yuv420p[,fps=<F>]"
       -c:v <encoder> <rate control>  [-tag:v hvc1 if hevc]
       -c:a aac -b:a 160k
       -movflags +faststart -metadata title=… -f mp4 -y <out.part.mp4>
```
- `-ss`/`-t` come **after** `-i` (output options), so the cut is frame-accurate. Decoding is short because the input only holds the snapped chunks.
- **Hardware decode without `-hwaccel_output_format`.** Frames are downloaded to system memory, so a single CPU filter chain works for every backend. For short clips this costs little. VAAPI is the exception: the encoder needs `format=nv12,hwupload` and `-vaapi_device`. VAAPI is Linux-only and best effort.
- **`out_range=tv`.** Steam writes full-range `yuvj420p`. Converting to limited range avoids washed-out or crushed colours in Discord and Films & TV.
- **Rounding.** Round `H` down to an even number, skip `scale` when the source is already ≤ `H`, and apply `crop` only when the source aspect is > 16:9 + 1 %.
- Progress = `out_time_us / (out_s − in_s)`, mapped to 0.5–1 like Fast.

### 4.2 Encoders (`media/encoders.rs`, new)
- `Capabilities { encoders: HashSet<String>, hwaccels: HashSet<String> }`: run `ffmpeg -encoders` and `ffmpeg -hwaccels` once per ffmpeg path, and cache the result in `MediaState` (clear it when `set_ffmpeg_dir` changes the path).
  - Move `transcode::list` and the parsing in `transcode::available` (`preview/transcode.rs:219-245`) here, and have the preview call the shared version, so the lists are no longer probed again on every detection.
- **Vendor family** comes from the preview `Backend` already resolved by `preview_backend()` (`commands.rs:326`). `export_clip` therefore also takes `State<'_, PreviewState>`.

| Backend | hwaccel | H.264 | HEVC | AV1 |
|---|---|---|---|---|
| `Cuda` | `cuda` | `h264_nvenc` | `hevc_nvenc` | `av1_nvenc` (RTX 40+) |
| `D3d11vaAmf` | `d3d11va` | `h264_amf` | `hevc_amf` | `av1_amf` |
| `D3d11vaQsv` / `Qsv` | `d3d11va` / `qsv` | `h264_qsv` | `hevc_qsv` | `av1_qsv` |
| `Vaapi` | `vaapi` | `h264_vaapi` | `hevc_vaapi` | `av1_vaapi` |
| `Software` | – | `libx264` | `libx265` | `libsvtav1` |

  If the chosen encoder is not in `Capabilities`, use the software one for that codec. If that does not exist either (no AV1), the codec option is hidden in the UI (`get_export_capabilities() -> Vec<VideoCodec>`).
- **Rate control** (a pure function `rate_args(encoder, target, duration_s)`, unit-tested):
  - `MaxSizeMb(m)`: `v_kbps = m·8·1000·0.95 / dur − 160`, giving `-b:v v -maxrate v -bufsize 2v`. NVENC also gets `-rc vbr`. If `v_kbps < 300`, the command fails with a clear `AppError` (the UI checks this first, see 4.3).
  - `Crf(q)`: `-cq q` (NVENC with `-rc vbr -b:v 0`), `-crf q` (x264/x265/svtav1), `-global_quality q` (QSV), `-qp q` (VAAPI), `-rc cqp -qp_i q -qp_p q` (AMF).
  - Presets: NVENC `-preset p5`, x264 `-preset medium`, x265 `-preset fast`, svtav1 `-preset 8`.
- **Fallback.** If a hardware encode exits with an error (not `Cancelled` or `NotFound`), delete the `.part` file and run once more with the software encoder, then set `fell_back = true`. A common cause is that the NVENC session limit is reached while Steam itself is recording.

### 4.3 Frontend (`src/components/ExportPanel.tsx`)
- **Mode switch** `Fast | Precise` next to the Export button.
- **Preset select** (Precise only). A preset fills in `PreciseOpts`:

| Preset | Codec | Max height | fps | Target |
|---|---|---|---|---|
| Discord (10 MB) | H.264 | 720 | 60 | `MaxSizeMb(10)` |
| Discord Nitro (500 MB) | H.264 | 1080 | 60 | `MaxSizeMb(500)` |
| Original quality | HEVC | source | source | `Crf(22)` |
| Custom | the values from "Advanced" | | | |

- **"Advanced"** disclosure: codec (only codecs from `get_export_capabilities`), max height (source/1440/1080/720), fps (source/60/30), "Crop to 16:9" (shown only when the source is wider than 16:9) and the target (quality slider or MB). Changing any value switches the preset to Custom.
- **Estimate:**
  - Precise + `MaxSizeMb` shows "≤ N MB".
  - Precise + `Crf` shows nothing.
  - Show a warning when the computed video bitrate is < 300 kbit/s ("too long for 10 MB – shorten or lower the resolution"), and disable Export.
- **Timeline.** In Precise mode the selection band is drawn exactly. The ghost edges from `snapSelection` (`src/lib/selection.ts`, used in `Timeline.tsx:162`) appear only in Fast mode. Store the mode in `useView` so the timeline can read it.
- **Job row** shows the mode/preset and, when it is done, `encoder` plus a "software fallback" note if `fell_back` is set.
- **Remembering:** the last mode, preset and custom options are kept in `settings.json`, under the keys `exportMode` and `exportPrecise`. Use new `get_export_prefs`/`set_export_prefs` commands built on `read_setting`/`write_setting` (`commands.rs:300-315`).
- **Tooltip.** Replace the M3 tooltip ("HEVC, may not play everywhere") with: Fast = original quality, instant, HEVC; Precise = exact cut, plays everywhere with H.264.

## 5. Edge cases
- **Selection shorter than 1 frame or `in ≥ out`:** use at least one frame for Precise. Swap a reversed range, like M3 does.
- **Selection inside a gap or across one:** same error as M3 (`chunk_range` already rejects it).
- **Live session:** the newest chunk is dropped as in M3 (`is_live`). If `out_s` is in that chunk, clamp it and show the real `actual_out_s`.
- **No hardware encoder, or the NVENC session limit is hit:** software fallback, see 4.2.
- **Source smaller than `max_height`:** do not upscale.
- **Odd widths after crop/scale:** `-2` and an even crop width (`floor(ih*16/9/2)*2`).
- **Non-ASCII paths** (Windows user names): pass every path to `Command::arg` as `&Path`/`OsStr`, never through `to_string_lossy`.
- **Cancel during encode:** `wait_or_kill`, and the `Cleanup` drop deletes the job folder and `.part` (as in M3).
- **Windows Defender** scanning the sidecar on the first run makes the first encode slow. That is acceptable; progress shows the delay.

## 6. Acceptance criteria
- [ ] A precise export of a selection that does **not** start on a 3 s boundary starts on the chosen frame: the first frame of the export matches the preview frame at `in_s` ±1 frame.
- [ ] The duration equals `out_s − in_s` ±1 frame.
- [ ] "Discord (10 MB)" gives a file ≤ 10 MB for a 30 s clip and plays in the Discord desktop client (inline preview).
- [ ] "Original quality" gives HEVC at the source resolution and plays in VLC.
- [ ] On the 32:9 sample, "Crop to 16:9" gives 2560×1440 (or scaled 16:9) with the centre of the image.
- [ ] Colours match the preview: no washed-out blacks (check with `ffprobe` that `color_range=tv`).
- [ ] With NVENC forced to fail (for example an unknown encoder option in a test build), the export still finishes and the job row shows "software fallback".
- [ ] **Windows:** an H.264 precise export plays in Films & TV without the HEVC extension. NVENC is used on an NVIDIA teammate machine (the job row shows `h264_nvenc`).
- [ ] Fast export still behaves exactly as in M3.
- [ ] clippy, tests, tsc and lint are clean; CI is green.

## 7. Verification steps
- **Unit tests:**
  - trim offset (`in_s − snap.actual_in_s`) with `first ≠ 1`,
  - `rate_args` for every encoder × target, including the < 300 kbit/s error,
  - filter-chain builder (crop on/off, no upscale, even sizes),
  - encoder choice from `Backend` × codec × `Capabilities` (missing encoder → software, missing AV1 → not offered),
  - `ExportMode` serde round trip (camelCase, tagged).
- **Integration test** `precise_export_session` (`#[ignore]`, needs ffmpeg, `STEAMREPLAY_SESSION_DIR=<bg folder>` like M3's `export_session`): export 1.5 s → 4.2 s as H.264 720p software, then with the detected backend. ffprobe shows h264, height 720, duration 2.7 s ± 1/60, `color_range=tv`, an AAC track, and no job folder or `.part` left.
- **Manual** (`bg_1445790_20261015_210120`):
  - export the same spike with each preset and play each file,
  - clap check: the first frame against the preview,
  - cancel halfway,
  - run two precise exports in a row,
  - export while Steam is recording (NVENC sessions).
- **Windows:** a teammate runs the presets and reports the encoder, time and whether Films & TV and Discord play the files.

## 8. Open questions / risks
- **Encode speed.** A 30 s 5K source decoded via NVDEC and downloaded to system memory should run several times faster than realtime on an RTX 4080. If the download is the bottleneck, use a full-GPU chain for CUDA only (`-hwaccel_output_format cuda`, `scale_cuda`, `crop` via the `hevc_cuvid -crop` decoder option). Measure before optimising.
- **Discord 10 MB vs 60 fps:** at 720p60 the bitrate allows about 30 s of clip. If quality is poor, change the preset to 30 fps. Decide after a test with real footage.
- **AV1** depends on teammates' GPUs (RTX 40 for NVENC AV1) and on where clips are posted. It has the lowest priority; hide it when no encoder exists.
- **GTX 10xx or older:** no AV1 encode, older NVENC. The software fallback covers it.
- **AMF/QSV/VAAPI rate-control flags** are untested without that hardware. Software fallback is the safety net.
