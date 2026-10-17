# M15: Laughter detection

## 1. Goal and user-visible outcome
The Moments tab ranks **laughs** instead of loud parts. M14 (§7) found that EfficientAT `mn10_as` with a laughter score puts about 8 good moments in its top 10. Today's loudness detector puts 3.5. M15 ports that model into the app:
- After the waveform appears, a second background job runs the model over the audio ("Finding laughs… 40 %"). It takes about 1 min per audio hour, and the result is cached like the waveform.
- When it is done, the list, the timeline bands, N/P and the lane show laughs. The Detection panel has one **Sensitivity** slider for them.
- **Loudness is the fallback only.** It is shown while the model runs, and when the model is missing or fails (with the reason and a Retry).

## 2. Prerequisites
- M2 (audio decode and placement, waveform cache), M6 (spike params, per-game presets, tuning UI), M9 (Moments tab, lane), M14 (results and labels).
- `00-overview.md`: cross-cutting rules (long jobs, cache keys, Windows first).

## 3. Scope / non-goals
**In scope:**
- the model in pure Rust (`tract-onnx`),
- the mel front-end in Rust,
- a streaming 32 kHz decode,
- the laughter curve, regions and cache,
- the model fetched at build time and bundled,
- the UI switch from loudness to laughter, the sensitivity slider, and the lane.

**Non-goals:**
- other reaction classes (shout, cheer); M14 found they add nothing,
- GPU inference,
- PANNs (plan B, if teammates' recordings disagree),
- a manual switch back to loudness,
- re-labelling.

## 4. Design

### Decisions
| Question | Decision |
|---|---|
| Runtime | `tract-onnx` 0.23 (pure Rust, no DLL). `ort` was the fallback and was not needed (§6). |
| Model source | Bundled. The ONNX is a release asset of this repo, fetched at build time with a pinned sha256 (`pnpm fetch-model`), and shipped as a Tauri resource on every platform. |
| Display | Laughter only. Loudness shows only as the fallback. |
| Tuning | One laughter Sensitivity slider on an **absolute** probability threshold, so a session without laughs shows few moments. It shares merge gap, padding and max results with loudness. It is stored in `SpikeParams.laughThreshold`, so per-game presets, reset and the settings default work unchanged. |
| Lane | The lane toggle shows the active detector's curve: laughter probability 0–1 with the threshold line, or loudness above baseline. |

### Pipeline (must match `experiments/moments/taggers.py` `EfficientAT`)
1. **Audio:** mono 32 kHz, decoded straight from the chunks through ffmpeg `-af aresample=16000,aresample=32000`. Sample 0 = video time 0, placed per run exactly like the waveform (`audio::run_starts`). Missing chunks are zeros.
   - The detour through 16 kHz is deliberate: it reproduces M14's input, the 16 kHz dump resampled to 32 kHz, which only had content up to 8 kHz. The full band ranked worse on our labels (§6).
2. **Windows:** 5 s (160 000 samples), 1 s hop, zero-padded at the end. There are `max(1, ceil((n − 160000 + 32000) / 32000))` windows.
3. **Mel** (`media/laughter/mel.rs`, `AugmentMelSTFT` in eval mode):
   - pre-emphasis `y[i] = x[i+1] − 0.97·x[i]`,
   - reflect pad by 512,
   - a symmetric Hann window of 800, centred in 1024 (112 zeros on the left), hop 320, giving 500 frames,
   - power `re² + im²`,
   - Kaldi mel banks: 128 mels, 0–15 kHz, no VTLN warp, one zero column,
   - `(ln(x + 1e-5) + 4.5) / 5`.
4. **Model** (`media/laughter/model.rs`):
   - input `[8, 1, 128, 500]` (fixed batch of 8; the last batch is padded) → 527 logits → sigmoid,
   - score = max over AudioSet indices 16–21: Laughter, Baby laughter, Giggle, Snicker, Belly laugh, "Chuckle, chortle".
5. **Curve:** 1 Hz, `curve[i] = score[i − 2]`, and the first two values repeat `score[0]`. This is exactly what taggers.py does: a window's score belongs to its centre, and the shift is `round(2.5) = 2`.
6. **Regions** (`laughter::regions`, a port of `common.regions`):
   - runs where `curve > threshold`,
   - merged when the gap is `< merge_gap_s`,
   - padded by `pad_s` and clamped to the duration,
   - scored by their peak, best first, at most `max_results`.

   Regions are returned as `Spike` (`peakExcessDb` 0, `score` = probability), so the list and the timeline need no new type.

### Backend
- **Streaming decode** (`audio::stream_placed`): decoding 3 h at 32 kHz into memory would take 1.4 GB.
  - Each run's samples are sent in blocks, with their absolute sample index, through a bounded channel. The run planning and probing are shared with `decode_placed`.
  - A `WindowFeeder` fills gaps with zeros, drops overlap, and hands out the windows.
  - Memory stays at a few MB.
- **Workers:** `available_parallelism − 2` threads, clamped to 1–8. Each has its own `MelFrontend` and they share one `Model`. They take batches of 8 windows and check the cancel token between batches. Progress = windows done / windows.
- **Model state** (`LaughState`, managed by Tauri):
  - The model is loaded lazily once from `resources/models/mn10_as.onnx` (`BaseDirectory::Resource`).
  - A missing file or a load error is remembered as `AppError::Model` until restart, and the UI falls back to loudness.
  - An LRU of 4 curves is kept, like the waveform analyses.
- **Cache:** `<cache>/steamreplay/laughter/<cache_stem(id)>-<first>-<last>.bin`.
  - Format: magic `SRLA`, version, model id (the first 16 hex digits of the ONNX sha256), `u32` length, then the f32 curve.
  - It is written only after a complete run. *Settings → Storage* counts and clears it with the waveforms.
- **Commands:**
  - `analyze_laughter(id, jobId, progress) → null`: the cache or a run. It needs the waveform analysis first, for the inventory and the segment length.
  - `get_laughs(id, params) → Spike[]`
  - `get_laugh_curve(id) → number[]`: the whole curve; 3 h is 10 800 values.

### Frontend
- **`useAnalysis`:**
  - After `analyzeAudio`, it starts `analyzeLaughter` as a second job with its own job id. The job is cancelled on unmount, a session change and Retry.
  - `laugh` status: `running(fraction) | ready | unavailable(reason) | failed(message)`.
  - `detector = laugh.kind === "ready" ? "laughter" : "loudness"`. `spikes` comes from `getLaughs` or `getSpikes` to match. Switching the detector clears the selection.
- **Detection panel:**
  - In laughter mode: Sensitivity ↔ `laughThreshold` on a log scale (`lib/spikeTuning.ts`). The status reads "N laughs found · threshold 1.0 %". Advanced shows only merge gap, padding and max results.
  - In loudness mode: unchanged, plus a line saying why (running / missing / failed) with Retry.
- **Moments tab:** the rows use the `Laugh` icon, and the filter reads All / Laughs / Markers. Labelling mode is unchanged: the pool overrides both detectors.
- **Lane:** in laughter mode it draws the 1 Hz curve (0–1) with the threshold line. In loudness mode it is unchanged.
- **Settings → Moments:** "Default laughter sensitivity" next to the loudness one.

### Model delivery
- `experiments/moments/export_onnx.py` exports the network without its front-end.
  - Opset 17, dynamic batch, 19.5 MB.
  - sha256 `b59b560b0d64f5a094bb5b2b40fb20a65eeb4c9a3e5cd8b183c66f4265d0f978`.
  - It also writes the parity fixtures in `src-tauri/tests/fixtures/effat/`: the top laughter window of the Make Way session at 693 s, its mel banks, mel and logits.
  - Compute the torch reference **before** `torch.onnx.export`. After the export, the module gave logits off by up to 4.
- GitHub release `model-mn10as-v1` holds `mn10_as.onnx`.
- `pnpm fetch-model` (`scripts/fetch-model.ts`) downloads it through the GitHub API, because the repo is private.
  - It uses `GH_TOKEN`/`GITHUB_TOKEN` or falls back to `gh auth token`, checks the sha256, and writes `src-tauri/resources/models/mn10_as.onnx` (gitignored).
  - CI runs it before every build.
- Licence: the EfficientAT code and weights are MIT (`resources/models/LICENSE-EfficientAT.txt`, bundled). The weights were trained on AudioSet.

## 5. Acceptance criteria
- [x] The mel front-end matches PyTorch on the fixture (max abs diff < 1e-3) and the logits match (< 1e-3). Both tests are in `cargo test`.
- [x] Session parity: the Rust pipeline on the exact Python input reproduces `effat_laugh.npz` (max abs diff ≤ 1.4e-3) with identical top-10 regions on all 3 sessions.
- [x] On the real app path (decode from the chunks, default threshold), P@10 on the committed labels equals the Python reference on the audio still on disk (§6).
- [x] Runtime ≤ 3 min per audio hour in release on 8 threads (74 s).
- [ ] Reopening a session loads the cached curve instantly.
- [ ] Opening a recording is not blocked: the waveform and the loudness moments come first, then laughs replace them.
- [ ] Switching sessions mid-run cancels the job; a missing model falls back to loudness with a hint.
- [ ] Windows: the CI installer contains the model, and analysis works on a teammate's machine.

## 6. Results
Measured on the dev machine (CachyOS, 16 cores; release build, 8 worker threads).

**Spike: tract works.** The exported ONNX (opset 17) loads in `tract-onnx` 0.23 without changes.
- Logits vs PyTorch on the fixture: max abs diff 1.8e-5. Mel vs PyTorch: 8.3e-5. Mel banks vs torchaudio: 1.3e-5 (torchaudio computes them in f32).
- Model alone: 132 ms per window on 1 thread (475 s per audio hour), 132 s on 4 threads, 75 s on 8.
- `ort` was not needed.

**Session parity** (`laughter_parity` on `audio32.f32`, the exact Python input; `compare_rust.py`):

| Session | Windows | Max abs diff vs `effat_laugh.npz` | Top 10 identical | Time |
|---|---|---|---|---|
| Make Way, 1.0 h | 3716 | 6.8e-4 | yes | 75 s |
| Palworld 1, 1.1 h | 3926 | 5.0e-4 | yes | 76 s |
| Palworld 2, 2.9 h | 10 349 | 1.4e-3 | yes | 202 s |

**App path** (`laughter_session`: chunks → 32 kHz stream → curve → regions at 0.01). It takes **74 s per audio hour**: the decode runs alongside the model and costs nothing extra.

| Input | P@10 MW / PW1 / PW2 | mean | NDCG@20 |
|---|---|---|---|
| Python reference (`effat_laugh`) | 1.00 / 0.50 / 0.80 | 0.77 | 0.77 |
| App, full band 32 kHz | 0.95 / 0.35 / 0.70 | 0.67 | 0.65 |
| **App, via 16 kHz (shipped)** | **1.00 / 0.35 / 0.80** | **0.72** | 0.72 |
| Python reference, only the audio still on disk | 1.00 / **0.35** / 0.80 | 0.72 | – |
| `loud` (today's detector) | 0.30 / 0.30 / 0.45 | 0.35 | 0.33 |

- **Steam pruned Palworld 1 after the M14 dump.** `startNumber` went from 3418 to 3440, so video time 0 moved by 66 s, and the M14 labels are in the old timeline. `compare_rust.py` finds the offset by correlation and shifts the app's regions before scoring (`effat_rust_aligned`).
  - Two of the reference's top 10 (44 s, good; 62 s, meh) are in the pruned part. On the audio that is still on disk, the reference also scores 0.35.
  - So the shipped app path ranks **exactly** like M14's method.
- **Full band vs 8 kHz.** With the full band, the curves still correlate with the reference (0.93), but the ranking loses one good moment per session on MW and PW2. M14 validated the band-limited input, so the app decodes through 16 kHz. Re-labelling could revisit this, since the pools were built from band-limited runs.

**Default threshold** (on the reference curves; `compare_rust.py`):

| Threshold | P@10 MW / PW1 / PW2 | Regions MW / PW1 / PW2 |
|---|---|---|
| 0.005 | 1.00 / 0.50 / 0.80 | 132 / 23 / 68 |
| **0.01** | 1.00 / 0.50 / 0.80 | 114 / 13 / 31 |
| 0.02 | 1.00 / 0.50 / 0.80 | 82 / 9 / 13 |
| 0.05 | 1.00 / 0.35 / 0.50 | 57 / 5 / 5 |
| 0.1 | 1.00 / 0.20 / 0.20 | 28 / 2 / 2 |

The 0.97 quantile that M14 used is 0.004–0.08: the Palworld sessions have very low laughter probabilities. **0.01** keeps M14's P@10 everywhere and still gives a quiet session about a dozen moments, so it is the default. The slider spans 0.04 % – 25 % on a log scale with 1 % at its middle.

## 7. Open questions / risks
- Three sessions from one player are a small set; teammates' recordings may differ. Plan B is PANNs CNN14 (310 MB, 3× slower).
- Mic and game audio are mixed: laughing NPCs or game sounds tagged as laughter will rank.
- Running 6–8 threads for a minute competes with the preview decode on weaker machines. If teammates notice, lower the thread count.
