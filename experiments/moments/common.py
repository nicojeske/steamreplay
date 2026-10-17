"""Shared helpers for the M14 experiments: sessions, audio, labels and peak picking.

Data layout (see docs/specs/14-ml-moments.md):
  data/<stem>/audio.wav        16 kHz mono f32, sample 0 = video time 0 (from the Rust dump test)
  data/<stem>/baseline.json    today's spikes (default and loose params)
  out/<stem>/<method>.npz      score curve: `curve` (float32), `hz`, optional `classes` + `probs`
  out/<stem>/<method>.json     ranked regions + runtime
  <labels dir>/<stem>.pool.json / .labels.json   shared with the app
"""

from __future__ import annotations

import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
OUT = ROOT / "out"

# AudioSet classes that mean "people in voice chat react". Crowd/Applause are left out: games are full of them.
LAUGH = ["Laughter", "Baby laughter", "Giggle", "Snicker", "Belly laugh", "Chuckle, chortle"]
SHOUT = ["Shout", "Bellow", "Whoop", "Yell", "Battle cry", "Children shouting", "Screaming", "Cheering"]
REACTION = LAUGH + SHOUT
# Kept as fusion features / to explain false positives.
CONTEXT = ["Speech", "Music", "Gunshot, gunfire", "Explosion", "Video game music", "Silence"]

# Region picking, the same spirit as `spikes::regions` in the app.
MERGE_GAP_S = 5.0
PAD_S = 1.5
POOL_TOP = 30


def labels_dir() -> Path:
    """The app's `labels_dir` command; override with STEAMREPLAY_LABELS_DIR."""
    if env := os.environ.get("STEAMREPLAY_LABELS_DIR"):
        return Path(env)
    ident = "de.steamreplay.app"
    if sys.platform == "win32":
        return Path(os.environ["APPDATA"]) / ident / "labels"
    if sys.platform == "darwin":
        return Path.home() / "Library/Application Support" / ident / "labels"
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / ident / "labels"


def sessions() -> list[str]:
    return sorted(p.name for p in DATA.iterdir() if (p / "audio.wav").exists())


def load_audio(stem: str, sr: int = 16000) -> np.ndarray:
    """Mono float32 at `sr` (resampled from the 16 kHz dump when needed)."""
    x, file_sr = sf.read(DATA / stem / "audio.wav", dtype="float32")
    if sr != file_sr:
        from scipy.signal import resample_poly

        g = np.gcd(sr, file_sr)
        x = resample_poly(x, sr // g, file_sr // g).astype(np.float32)
    return x


def baseline(stem: str) -> dict:
    return json.loads((DATA / stem / "baseline.json").read_text())


@dataclass
class Region:
    start_s: float
    end_s: float
    peak_s: float
    score: float

    def json(self) -> dict:
        return {"startS": self.start_s, "endS": self.end_s, "peakS": self.peak_s, "score": self.score}


def smooth(curve: np.ndarray, hz: float, window_s: float) -> np.ndarray:
    n = max(1, int(round(window_s * hz)))
    if n == 1:
        return curve
    k = np.ones(n, dtype=np.float32) / n
    return np.convolve(curve, k, mode="same")


def regions(curve: np.ndarray, hz: float, duration_s: float, quantile: float = 0.97, top: int = 200) -> list[Region]:
    """Runs above the `quantile` of the curve, merged within 5 s, padded by 1.5 s, scored by their peak."""
    if len(curve) == 0:
        return []
    thr = float(np.quantile(curve, quantile))
    above = curve > thr
    if not above.any():
        return []
    edges = np.flatnonzero(np.diff(np.concatenate([[0], above.astype(np.int8), [0]])))
    runs = list(zip(edges[::2], edges[1::2]))
    merged: list[list[int]] = []
    for a, b in runs:
        if merged and (a - merged[-1][1]) / hz < MERGE_GAP_S:
            merged[-1][1] = b
        else:
            merged.append([a, b])
    out = []
    for a, b in merged:
        i = a + int(np.argmax(curve[a:b]))
        out.append(
            Region(
                start_s=max(0.0, a / hz - PAD_S),
                end_s=min(duration_s, b / hz + PAD_S),
                peak_s=i / hz,
                score=float(curve[i]),
            )
        )
    out.sort(key=lambda r: -r.score)
    return out[:top]


def save_method(
    stem: str,
    method: str,
    curve: np.ndarray,
    hz: float,
    runtime_s: float,
    regs: list[Region] | None = None,
    **extra: np.ndarray,
) -> None:
    d = OUT / stem
    d.mkdir(parents=True, exist_ok=True)
    duration = len(curve) / hz
    if regs is None:
        regs = regions(curve, hz, duration)
    np.savez_compressed(d / f"{method}.npz", curve=curve.astype(np.float32), hz=np.float32(hz), **extra)
    (d / f"{method}.json").write_text(
        json.dumps({"method": method, "runtimeS": runtime_s, "audioS": duration, "regions": [r.json() for r in regs]}, indent=1)
    )
    print(f"{stem} {method}: {len(regs)} regions, {runtime_s:.1f} s for {duration / 3600:.2f} h", flush=True)


def load_method(stem: str, method: str) -> dict | None:
    p = OUT / stem / f"{method}.json"
    return json.loads(p.read_text()) if p.exists() else None


def windows(x: np.ndarray, sr: int, win_s: float, hop_s: float):
    """Yields (start sample, window) with zero padding at the end."""
    win, hop = int(win_s * sr), int(hop_s * sr)
    for start in range(0, max(1, len(x) - win + hop), hop):
        w = x[start : start + win]
        if len(w) < win:
            w = np.pad(w, (0, win - len(w)))
        yield start, w


def batched(it, n: int):
    batch = []
    for item in it:
        batch.append(item)
        if len(batch) == n:
            yield batch
            batch = []
    if batch:
        yield batch


class Timer:
    def __enter__(self):
        self.t = time.perf_counter()
        return self

    def __exit__(self, *_):
        self.s = time.perf_counter() - self.t


def class_indices(names: list[str], wanted: list[str]) -> list[int]:
    idx = [names.index(w) for w in wanted if w in names]
    missing = [w for w in wanted if w not in names]
    if missing:
        print(f"  (classes not in this model: {missing})", flush=True)
    return idx
