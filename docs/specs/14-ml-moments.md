# M14: ML moment detection (experiments)

## 1. Goal and user-visible outcome
The loud-part detector (M2/M6) ranks by "how much louder than the last 30 s". A group laughing, a mic bump and an explosion all look the same to it, and the 1 h sample hits the 200-result cap. The team's good moments are **voice reactions**: laughing, shouting, cheering and screaming in voice chat, which is mixed into the one audio track.

M14 finds out **which ML approach ranks voice reactions best** on our own recordings, with numbers. It ships:
- a **labelling mode** in the app, which rates candidate moments blind,
- a Rust **data dump** (audio + today's spikes, with the app's exact time alignment),
- a **Python experiment harness** (`experiments/moments/`) that runs several models, pools their candidates for labelling and evaluates them,
- a **results** section and a recommendation for M15, which ports the winner to Rust.

Normal users see no change unless they turn on labelling mode.

## 2. Prerequisites
- M2 (`media/audio.rs` extraction and run placement, `media/spikes.rs`), M6 (tuning, presets), M9 (Moments tab, settings dialog).
- `00-overview.md`. **Exception to "typed languages only":** Python is allowed under `experiments/` for throwaway research. Nothing in `experiments/` ships, and the app never runs Python.

## 3. Scope / non-goals
**In scope:** labelling mode, the label and pool files, the 16 kHz dump, the Python models and the evaluation, and the results.

**Non-goals:**
- shipping a model (M15),
- game-event detection (gunfire, kills), keyword spotting ("clip that"), video analysis,
- training a model from scratch (too few labels); we only fit a tiny fusion on top of pretrained models.

## 4. Design

### 4.1 Labels and pools (Rust: `labels.rs`, commands)
Files in `app_data_dir()/labels/`. The stem is the session id with `/` replaced by `__`, for example `115461900__bg__bg_1445790_20261015_210120`.
- `<stem>.pool.json`, written by Python:
  ```json
  { "version": 1, "candidates": [ { "id": "c017", "startS": 812.3, "endS": 818.9 } ] }
  ```
  The candidates are the **merged union of every method's top regions** (TREC-style pooling). The pool does not say which method found a candidate, so the rating is blind.
- `<stem>.labels.json`, written by the app:
  ```json
  { "version": 1, "labels": [ { "candidateId": "c017", "startS": 812.3, "endS": 818.9, "rating": "good" },
                              { "candidateId": null, "startS": 1500.0, "endS": 1506.0, "rating": "missed" } ] }
  ```
  The rating is `good | meh | bad | missed`. Times are stored with each label, so labels survive a re-generated pool: Python matches labels to new candidates by overlap. Setting a rating again replaces the old one, and `rating: null` removes it.
- Commands:
  - `get_label_pool(id) -> Option<Vec<PoolCandidate>>`: `None` when there is no file, and a corrupt file is logged and gives `None`.
  - `get_labels(id) -> Vec<Label>`
  - `set_label(id, label: Label, remove: bool) -> Vec<Label>`
  - `labels_dir() -> PathBuf`, shown in the settings so you can hand the folder to the scripts
- Writes go to `.tmp` + rename. Labelling is a local experiment, so nothing is written into Steam's folders.

### 4.2 Labelling mode (frontend)
- A UI pref `labelling` (localStorage), toggled in **Settings → Loud moments → Labelling mode**. That section also shows the labels folder.
- When it is on and the session has a pool, `useAnalysis` returns the pool (in time order) as its `spikes`. Timeline bands, the list, N/P and Shift+N/P work as before. The Detection tuning is hidden.
- Keys (in `shortcuts.ts`, group "Labelling", active only in labelling mode):
  - `1` = good, `2` = meh, `3` = bad, for the selected candidate,
  - `X` = missed moment: a 6 s region centred on the playhead.
  After rating, the selection moves to the next unrated candidate in time, and playback starts there, so labelling is "listen → press a number".
- Each list row shows a rating chip. The header shows "37 / 120 rated".

### 4.3 Data dump (Rust, `#[ignore]` test `dump_experiment`)
`STEAMREPLAY_SESSION_DIR=<bg folder> cargo test --release dump_experiment -- --ignored --nocapture` writes `experiments/moments/data/<stem>/`:
- `audio.wav`: 16 kHz mono f32. It comes from `audio::extract_samples`, which shares run planning, start probing and decoding with `extract`, so sample 0 = video time 0, exactly as in the app.
- `baseline.json`: `{ id, durationS, default: [Spike], loose: [Spike] }`, from `spikes::detect` with the default params and with `threshold_db = 6, max_results = 1000`.
- The test asserts that the first audible sample and the first audible waveform bin agree to within 11 ms. Measured: 0.363 s vs 0.36 s, 0.009 vs 0.01, and 0.527 vs 0.52. Dumping takes 2–6 s per session.
- `extract` and `extract_samples` share `decode_placed` (run planning, `run_starts` probing, placement) through a `Sink` trait (`Binner` or `RawSamples`), so the app's bins and the dump cannot drift apart. `extract_samples` is `#[cfg(test)]` until M15 needs it.

### 4.4 Experiments (`experiments/moments/`, uv project)
- `common.py`: loading, the label/pool format, the shared `regions(score_curve)` peak picking (threshold at a quantile, merge < 5 s, pad 1.5 s, score = peak), and session discovery.
- One script per method, writing `out/<stem>/<method>.npz` (score curve at 10 Hz) and `<method>.json` (ranked regions):

| Method | Model | Score |
|---|---|---|
| `loud` | today's detector, from `baseline.json` (`loose` list, rescored like the app) | M2 score |
| `panns` | PANNs CNN14 (AudioSet, 32 kHz, 80 M params), 2 s windows, 0.5 s hop | max of the voice-reaction class probabilities |
| `effat` | EfficientAT `mn10_as` (AudioSet, 32 kHz, 4.9 M params), 5 s windows, 1 s hop | same classes |
| `ast` | AST `MIT/ast-finetuned-audioset-10-10-0.4593` (16 kHz, 87 M params), 3 s windows, 1 s hop | same classes |
| `*_laugh` | the three taggers above | the laughter classes only |
| `clap` | `laion/clap-htsat-unfused` zero-shot, 5 s windows, 1 s hop | summed softmax of the positive prompts over all prompts |
| `arousal` | wav2vec2 MSP-dim (audeering), 3 s windows, 1 s hop | arousal × min(1, 2 × Silero-VAD speech share) |
| `*_voc` | the taggers on the Demucs `htdemucs` vocals stem | as above |
| `fusion` | logistic regression on per-candidate features from the methods above | leave-one-session-out |

  Voice-reaction AudioSet classes: Laughter, Baby laughter, Giggle, Snicker, Belly laugh, Chuckle/chortle, Shout, Bellow, Whoop, Yell, Battle cry, Children shouting, Screaming, Cheering. Crowd and Applause are left out, because games are full of them.
- **EfficientAT needs ≥ 4 s of context.** With 2 s windows it outputs ~1.0 for random classes (checked on the repo's own sample file), so it uses 5 s windows.
- Every tagger window's score is assigned to the window centre.
- `pool.py`: the top 30 of each method → merged pool → `labels_dir/<stem>.pool.json`. Ids stay stable across re-pooling for regions that were already pooled, so a second round only adds new candidates.
- `eval.py`, per method and session:
  - a candidate counts as a hit when it overlaps a `good` (1.0) or `meh` (0.5) label; precision@10 and @20 and NDCG@20,
  - recall@30 against all `good` + `missed` labels. **Decided:** the user does not mark missed moments, because scanning the whole footage takes too long. So this is *pooled* recall: the share of the good moments any method found that this method also finds. Moments that no method finds stay unknown.
  - unlabelled share of the top 20,
  - runtime per audio hour and model size.
  Writes `out/report.md`.

## 5. Edge cases
- A session without a pool in labelling mode: the normal spikes show, with a note "No label pool for this recording".
- Labels whose candidate left the pool: they stay in the file, are not shown in the list, and `eval.py` still uses them by overlap.
- Several labels for the same candidate: the last one wins (`set_label` replaces by `candidateId`, or for `missed` by an exact `startS`).
- Gaps in the audio: `extract_samples` leaves zeros, the same as the silent bins.

## 6. Acceptance criteria
- [ ] `dump_experiment` writes `audio.wav` with `duration × 16000` samples. The first non-silent sample lines up with the first non-silent bin of the app (±10 ms).
- [ ] Labelling: rate with 1/2/3/X, restart the app, and the ratings are back. `eval.py` reads them.
- [ ] Every method script runs on all three sample sessions and reports its runtime.
- [ ] `out/report.md` has the table for every method, and §7 has the results and a recommendation.
- [ ] clippy, tests, tsc and lint are clean.

## 7. Results
**Data.** 3 sessions, 6.9 h of audio:
- Make Way, 1.0 h (Oct 15): 39 pooled candidates, rated 27 good, 4 meh, 8 bad.
- Palworld, 1.1 h (Oct 10): 39 candidates, 6 good, 11 meh, 22 bad.
- Palworld, 2.9 h (Oct 10): 63 candidates, 17 good, 7 meh, 39 bad.

The pool is the union of every method's top 10, so **P@10 is fully labelled for every single method**. P@20 and NDCG@20 are lower bounds: 13–22 % of the top 20 is unrated and counts as 0. No missed moments were marked (§4.4), so R@30 is pooled recall.

| Method | P@10 per session (MW / PW1 / PW2) | mean P@10 | mean NDCG@20 | s per audio h, GPU | s per audio h, CPU (8 threads) | Size |
|---|---|---|---|---|---|---|
| `panns_laugh` (PANNs CNN14) | 0.95 / 0.55 / 0.95 | **0.82** | **0.78** | 3.5 | 168 | 80 M params, 310 MB |
| rank average `effat_laugh` + `panns_laugh` | 1.00 / 0.50 / 0.90 | 0.80 | 0.79 | – | – | – |
| `effat_laugh` (EfficientAT mn10) | 1.00 / 0.50 / 0.80 | 0.77 | 0.77 | 2.1 | **54** | **4.9 M params, 20 MB** |
| `clap` (LAION-CLAP zero-shot) | 1.00 / 0.55 / 0.75 | 0.77 | 0.76 | 69 | – | 150 M+ |
| `fusion` (LR over 16 curves, leave-one-session-out) | 1.00 / 0.50 / 0.70 | 0.73 | 0.77 | – | – | – |
| `ast` (AST) | – | 0.70 | 0.70 | 18.5 | – | 87 M |
| `*_voc` (taggers on the Demucs vocals) | – | 0.55–0.72 | 0.56–0.69 | + 53 for Demucs | – | – |
| `arousal` (wav2vec2 MSP-dim) | – | 0.67 | 0.66 | 44 | – | 300 M |
| **`loud` (today's detector)** | 0.30 / 0.30 / 0.45 | **0.35** | **0.33** | 0 | 0 | – |

The full table and the per-session top 10 are in `experiments/moments/out/report.md` (not committed; regenerate it with `eval.py`).

**Findings**
- **Every AudioSet tagger beats loudness by more than 2×.** Today's top 10 holds 3 good moments; PANNs or EfficientAT hold 8 of 10.
- **Laughter carries the signal.** Each tagger's laughter-only score ranks the same as, or better than, its all-reactions score. Shout, scream and cheer classes add nothing here.
- **Separating the voices with Demucs makes every model worse,** despite the mixed track. Source separation leaves artefacts that the taggers were not trained on. It is dropped.
- **The learned fusion is not worth it.** With 3 sessions its weights flip sign between correlated models (`ast` +1.3, `ast_voc` −1.6), and it is no better than the best single model. A plain rank average of two taggers ties PANNs.
- **CLAP zero-shot and speech arousal** cost 20–30× more than a tagger and rank worse.
- **Palworld session 1 is hard for every method** (P@10 ≈ 0.5): it has few reactions (6 good in 39), so the top 10 cannot be full of them.

**Recommendation for M15: port EfficientAT `mn10_as`** with the laughter-class score (max of Laughter, Baby laughter, Giggle, Snicker, Belly laugh, Chuckle/chortle) on 5 s windows with a 1 s hop.
- It is 1 good moment per 10 behind PANNs on one session and level elsewhere. That difference is within noise for 3 sessions.
- It is 15× smaller and 3× faster on CPU: about 1 min per audio hour on 8 threads, which runs in the background after the waveform.
- It is a plain CNN (convs, BN, Hardswish, SE, linear) plus a mel front-end, so it should run in pure-Rust `tract-onnx`. Then Windows needs no extra DLL.
- Keep loudness as the fallback, and keep **PANNs CNN14** as plan B if teammates' recordings disagree.
- Before porting, check `mn40_as` (larger, higher AudioSet mAP) and a 2 s hop on the same labels; both are cheap to test with `taggers.py`.

## 8. Open questions / risks
- Three sessions from one player are a small and biased set. The recommendation needs a check on teammates' recordings in M15.
- Mic and game audio are mixed. Taggers may hear in-game voices (NPC screams) as reactions. That is what the `_voc` variants and the negative CLAP prompts test.
