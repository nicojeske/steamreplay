# M14 moment-detection experiments

Throwaway research for [`docs/specs/14-ml-moments.md`](../../docs/specs/14-ml-moments.md). Python is fine here; nothing in this folder ships, and the app never runs it.

Needs [uv](https://docs.astral.sh/uv/), ffmpeg, and an NVIDIA GPU for reasonable speed. Everything also runs on CPU, just slower.

## 1. Dump sessions (Rust, from `src-tauri/`)
```sh
STEAMREPLAY_SESSION_DIR=~/.local/share/Steam/userdata/<acc>/gamerecordings/video/bg_… \
  cargo test --release --lib dump_experiment -- --ignored --nocapture
```
This writes `data/<stem>/audio.wav` (16 kHz mono, sample 0 = video time 0) and `baseline.json` (today's spikes). Repeat it for every session.

## 2. Run the methods
```sh
uv sync
git clone --depth 1 https://github.com/fschmid56/EfficientAT vendor/EfficientAT
./run_all.sh            # taggers, CLAP, arousal, Demucs vocals, then everything again on the vocals
```
Each method writes `out/<stem>/<method>.{json,npz}`: the ranked regions and a score curve.

| Script | Methods |
|---|---|
| `taggers.py panns\|effat\|effat40\|ast` | AudioSet taggers; `<m>` = any reaction class, `<m>_laugh` = laughter only |
| `clap.py` | `clap`: zero-shot against reaction and negative prompts |
| `arousal.py` | `arousal`: wav2vec2 emotion arousal where Silero VAD finds speech |
| `vocals.py` | Demucs vocal stem; the `--input vocals` runs add a `_voc` suffix |
| `eval.py --fusion` | `fusion`: logistic regression over all curves, leave-one-session-out |
| `combo.py name m1 m2…` | rank average of existing curves, no training |

## 3. Label
```sh
uv run python pool.py   # → <app data>/de.steamreplay.app/labels/<stem>.pool.json
```
In the app: **Settings → Moments → Labelling mode → On**. Open a recording. The Moments tab now lists the pool in time order:
- Select a candidate (or press N).
- Listen, then press **1** good, **2** meh or **3** bad. The next unrated candidate starts playing.
- **X** marks a good moment that no candidate covers (missed), around the playhead.

The pool does not say which method found a candidate. Running `pool.py` again after adding methods only adds candidates; ratings you already gave are kept.

## 4. Evaluate
```sh
uv run python eval.py --fusion   # → out/report.md
```
The ratings of the M14 round are committed in `labels/`. Copy them into the labels folder (or point `STEAMREPLAY_LABELS_DIR` at `labels/`) to reproduce the results.

If a method's `unl@20` (the unlabelled share of its top 20) is high, pool again and label the new candidates.
