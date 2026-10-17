"""Scores every method against the labels and writes out/report.md.

  uv run python eval.py [--fusion]

A method's region gets the gain of the best label it overlaps (good 1, meh 0.5, bad 0, missed 1).
Regions that overlap no label are "unlabelled" (gain 0, counted separately: if that share is high,
pool again and label the new candidates). `--fusion` adds a logistic regression over all curves,
trained leave-one-session-out.
"""

from __future__ import annotations

import argparse
import json
import math

import numpy as np

from common import OUT, Region, labels_dir, regions, save_method, sessions

GAIN = {"good": 1.0, "meh": 0.5, "bad": 0.0, "missed": 1.0}
K = (10, 20)
RECALL_K = 30


def load_labels(stem: str) -> list[dict]:
    p = labels_dir() / f"{stem}.labels.json"
    return json.loads(p.read_text())["labels"] if p.exists() else []


def overlap(a0: float, a1: float, b0: float, b1: float) -> float:
    return max(0.0, min(a1, b1) - max(a0, b0))


def matches(r: dict, lab: dict) -> bool:
    """Enough overlap to be the same moment: 1 s, or half of the shorter one."""
    o = overlap(r["startS"], r["endS"], lab["startS"], lab["endS"])
    shorter = min(r["endS"] - r["startS"], lab["endS"] - lab["startS"])
    return o > 0 and o >= min(1.0, 0.5 * shorter)


def gain(r: dict, labels: list[dict]) -> float | None:
    g = [GAIN[lab["rating"]] for lab in labels if matches(r, lab)]
    return max(g) if g else None


def score_method(regs: list[dict], labels: list[dict]) -> dict:
    gains = [gain(r, labels) for r in regs]
    out = {}
    for k in K:
        top = gains[:k]
        out[f"P@{k}"] = sum(g or 0.0 for g in top) / k
        out[f"unl@{k}"] = sum(g is None for g in top) / k
    # NDCG@20 against the best possible ranking of this session's labels.
    dcg = sum((g or 0.0) / math.log2(i + 2) for i, g in enumerate(gains[:20]))
    ideal = sorted((GAIN[lab["rating"]] for lab in labels), reverse=True)[:20]
    idcg = sum(g / math.log2(i + 2) for i, g in enumerate(ideal))
    out["NDCG@20"] = dcg / idcg if idcg else 0.0
    positives = [lab for lab in labels if lab["rating"] in ("good", "missed")]
    hit = [any(matches(r, lab) for r in regs[:RECALL_K]) for lab in positives]
    out[f"R@{RECALL_K}"] = sum(hit) / len(hit) if hit else 0.0
    return out


# --- fusion ------------------------------------------------------------------------------------

def curves_1hz(stem: str, methods: list[str]) -> np.ndarray:
    """Every method's curve max-pooled to 1 Hz and turned into per-session ranks (0–1), so the
    features are comparable across sessions and models."""
    cols = []
    n = None
    for m in methods:
        z = np.load(OUT / stem / f"{m}.npz")
        c, hz = z["curve"], float(z["hz"])
        step = max(1, int(round(hz)))
        c = c[: len(c) // step * step].reshape(-1, step).max(axis=1)
        n = len(c) if n is None else min(n, len(c))
        cols.append(c)
    feats = np.column_stack([c[:n] for c in cols])
    ranks = feats.argsort(axis=0).argsort(axis=0) / max(1, n - 1)
    return ranks.astype(np.float32)


def fusion(stems: list[str], methods: list[str]) -> list[float]:
    from sklearn.linear_model import LogisticRegression

    data = {}
    for s in stems:
        X = curves_1hz(s, methods)
        y = np.full(len(X), np.nan, dtype=np.float32)
        # Unlabelled seconds are weak negatives; labelled ones override.
        rng = np.random.default_rng(0)
        y[rng.choice(len(X), size=min(len(X), 2000), replace=False)] = 0.0
        for lab in load_labels(s):
            a, b = int(lab["startS"]), int(math.ceil(lab["endS"]))
            if lab["rating"] == "meh":
                y[a:b] = np.nan
            else:
                y[a:b] = 1.0 if lab["rating"] in ("good", "missed") else 0.0
        data[s] = (X, y)
    weights = []
    for held in stems:
        train = [data[s] for s in stems if s != held]
        X = np.concatenate([x[~np.isnan(y)] for x, y in train])
        y = np.concatenate([y[~np.isnan(y)] for _, y in train])
        if len(set(y)) < 2:
            print(f"fusion: not enough labels to train without {held}")
            continue
        clf = LogisticRegression(class_weight="balanced", max_iter=1000).fit(X, y)
        weights.append(clf.coef_[0])
        Xh = data[held][0]
        curve = clf.predict_proba(Xh)[:, 1].astype(np.float32)
        save_method(held, "fusion", curve, 1.0, 0.0, regions(curve, 1.0, len(curve)))
    w = np.mean(weights, axis=0) if weights else np.zeros(len(methods))
    return [float(v) for v in w]


# --- report ------------------------------------------------------------------------------------

def fmt(v: float) -> str:
    return f"{v:.2f}"


def main(with_fusion: bool) -> None:
    stems = sessions()
    methods = sorted({p.stem for s in stems for p in (OUT / s).glob("*.json")} - {"fusion"})
    fusion_w = None
    if with_fusion:
        common = [m for m in methods if m != "loud" and all((OUT / s / f"{m}.npz").exists() for s in stems)]
        fusion_w = dict(zip(common, fusion(stems, common)))
        methods.append("fusion")

    rows = []
    per_session = {}
    for m in methods:
        scores, runtime, audio = [], 0.0, 0.0
        for s in stems:
            p = OUT / s / f"{m}.json"
            if not p.exists():
                continue
            j = json.loads(p.read_text())
            labels = load_labels(s)
            if not labels:
                continue
            sc = score_method(j["regions"], labels)
            scores.append(sc)
            per_session.setdefault(s, {})[m] = (sc, j["regions"][:10], labels)
            runtime += j["runtimeS"]
            audio += j["audioS"]
        if not scores:
            continue
        mean = {k: float(np.mean([sc[k] for sc in scores])) for k in scores[0]}
        rows.append((m, mean, runtime / max(audio / 3600, 1e-9), len(scores)))
    rows.sort(key=lambda r: -r[1]["NDCG@20"])

    keys = ["P@10", "P@20", "NDCG@20", f"R@{RECALL_K}", "unl@20"]
    lines = ["# M14 results", "", f"Sessions: {len(stems)}. Mean over sessions with labels.", ""]
    lines.append("| Method | " + " | ".join(keys) + " | s per audio h (GPU) | sessions |")
    lines.append("|---" * (len(keys) + 3) + "|")
    for m, mean, rt, n in rows:
        lines.append(f"| `{m}` | " + " | ".join(fmt(mean[k]) for k in keys) + f" | {rt:.1f} | {n} |")
    if fusion_w:
        lines += ["", "Fusion weights (mean over folds, rank features):", ""]
        lines += [f"- `{m}`: {w:+.2f}" for m, w in sorted(fusion_w.items(), key=lambda kv: -abs(kv[1]))]
    for s, by_m in per_session.items():
        lines += ["", f"## {s}", ""]
        n_lab = len(next(iter(by_m.values()))[2])
        lines.append(f"{n_lab} labels.")
        for m, (sc, top10, labels) in sorted(by_m.items(), key=lambda kv: -kv[1][0]["NDCG@20"]):
            marks = []
            for r in top10:
                g = gain(r, labels)
                marks.append(f"{r['startS']:.0f}{'?' if g is None else {1.0: '+', 0.5: '~', 0.0: '-'}[g]}")
            lines.append(f"- `{m}` NDCG {sc['NDCG@20']:.2f}: " + " ".join(marks))
    lines += ["", "Top 10 legend: start second, `+` good, `~` meh, `-` bad, `?` unlabelled."]
    (OUT / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:len(rows) + 8]))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--fusion", action="store_true")
    main(ap.parse_args().fusion)
