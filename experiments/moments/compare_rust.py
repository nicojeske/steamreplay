"""M15: compares the Rust laughter pipeline with the Python `effat_laugh` run, and picks the default
absolute threshold.

  STEAMREPLAY_LABELS_DIR=labels uv run python compare_rust.py

Needs `out/<stem>/effat_rust_ref.{json,f32}` (Rust test `laughter_parity`: same input as Python) and,
optionally, `effat_rust.json` (Rust test `laughter_session`: the real app path).
"""

from __future__ import annotations

import json

import numpy as np

from common import OUT, regions, sessions
from eval import load_labels, score_method

# Steam prunes old chunks, so a recording's video time 0 can move after the M14 dump (the labels'
# timeline). The app-path curve is aligned to the dump by the best correlation within this range.
MAX_OFFSET_S = 600

THRESHOLDS = [0.005, 0.01, 0.015, 0.02, 0.03, 0.05, 0.1]


def main() -> None:
    q97 = []
    by_thr: dict[float, list[float]] = {t: [] for t in THRESHOLDS}
    counts: dict[float, list[int]] = {t: [] for t in THRESHOLDS}
    for stem in sessions():
        d = OUT / stem
        if not (d / "effat_rust_ref.f32").exists():
            print(f"{stem}: no effat_rust_ref, run the Rust test laughter_parity")
            continue
        py = np.load(d / "effat_laugh.npz")["curve"]
        rs = np.fromfile(d / "effat_rust_ref.f32", dtype="<f4")
        diff = np.abs(py - rs).max() if len(py) == len(rs) else float("nan")
        top = lambda m: [(r["startS"], r["endS"], r["peakS"]) for r in json.loads((d / f"{m}.json").read_text())["regions"][:10]]
        same = top("effat_laugh") == top("effat_rust_ref")
        q = float(np.quantile(py, 0.97))
        q97.append(q)
        labels = load_labels(stem)
        p10 = lambda regs: score_method(regs, labels)["P@10"]
        line = f"{stem}: len {len(py)}/{len(rs)}, max abs diff {diff:.2e}, top 10 identical: {same}, q97 {q:.4f}"
        line += f", P@10 python {p10(json.loads((d / 'effat_laugh.json').read_text())['regions']):.2f}"
        if (d / "effat_rust.json").exists():
            app = np.fromfile(d / "effat_rust.f32", dtype="<f4")
            off = offset(py, app)
            regs = json.loads((d / "effat_rust.json").read_text())["regions"]
            for r in regs:
                for k in ("startS", "endS", "peakS"):
                    r[k] += off
            write = {"method": "effat_rust_aligned", "runtimeS": 0, "audioS": len(app), "regions": regs}
            (d / "effat_rust_aligned.json").write_text(json.dumps(write, indent=1))
            line += f", app path {p10(regs):.2f} (offset {off} s)"
        print(line)
        for t in THRESHOLDS:
            regs = [r.json() for r in regions(py, 1.0, len(py), threshold=t)]
            by_thr[t].append(p10(regs))
            counts[t].append(len(regs))
    if q97:
        print(f"median q97 {np.median(q97):.4f}")
        for t, ps in by_thr.items():
            print(f"threshold {t:.3f}: P@10 {' / '.join(f'{p:.2f}' for p in ps)}, mean {np.mean(ps):.3f}, regions {counts[t]}")


def offset(ref: np.ndarray, cur: np.ndarray) -> int:
    """Seconds to add to `cur`'s times to land on `ref`'s (the pruned start of the recording)."""
    def corr(k: int) -> float:
        n = min(len(cur), len(ref) - k)
        return float(np.corrcoef(ref[k : k + n], cur[:n])[0, 1]) if n > 100 else -1.0

    return max(range(MAX_OFFSET_S), key=corr)


if __name__ == "__main__":
    main()
