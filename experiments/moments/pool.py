"""Blind candidate pool for labelling: the union of every method's top regions.

  uv run python pool.py [--top 30] [--methods a,b,…] [stems…]

Writes `<labels dir>/<stem>.pool.json` for the app's labelling mode. Existing candidates keep their
id and times (they may already be rated); new regions that overlap them are absorbed, the rest get
new ids. So a second round only adds candidates.
"""

from __future__ import annotations

import argparse
import json

from common import OUT, POOL_TOP, baseline, labels_dir, sessions

MAX_LEN_S = 20.0  # longer unions are split, so one candidate stays one moment to listen to


def ensure_loud(stem: str) -> None:
    """Today's detector as a method: the app's ranking with default params."""
    d = OUT / stem
    d.mkdir(parents=True, exist_ok=True)
    b = baseline(stem)
    (d / "loud.json").write_text(json.dumps({"method": "loud", "runtimeS": 0.0, "audioS": b["durationS"], "regions": b["default"]}))


def methods_for(stem: str) -> list[str]:
    return sorted(p.stem for p in (OUT / stem).glob("*.json"))


def union(spans: list[tuple[float, float]]) -> list[tuple[float, float]]:
    out: list[list[float]] = []
    for a, b in sorted(spans):
        if out and a < out[-1][1] and b - out[-1][0] <= MAX_LEN_S:
            out[-1][1] = max(out[-1][1], b)
        else:
            out.append([a, b])
    return [(a, b) for a, b in out]


def overlaps(a: tuple[float, float], b: tuple[float, float]) -> bool:
    return a[0] < b[1] and b[0] < a[1]


def main(stems: list[str], top: int, only: list[str] | None) -> None:
    ldir = labels_dir()
    ldir.mkdir(parents=True, exist_ok=True)
    for stem in stems:
        ensure_loud(stem)
        methods = [m for m in methods_for(stem) if not only or m in only]
        spans = []
        for m in methods:
            regs = json.loads((OUT / stem / f"{m}.json").read_text())["regions"][:top]
            spans += [(r["startS"], r["endS"]) for r in regs]
        merged = union(spans)

        path = ldir / f"{stem}.pool.json"
        old = json.loads(path.read_text())["candidates"] if path.exists() else []
        cands = list(old)
        next_id = 1 + max((int(c["id"][1:]) for c in old), default=0)
        added = 0
        for span in merged:
            if any(overlaps(span, (c["startS"], c["endS"])) for c in old):
                continue
            cands.append({"id": f"c{next_id:03d}", "startS": round(span[0], 2), "endS": round(span[1], 2)})
            next_id += 1
            added += 1
        cands.sort(key=lambda c: c["startS"])
        path.write_text(json.dumps({"version": 1, "candidates": cands}, indent=1))
        total_s = sum(c["endS"] - c["startS"] for c in cands)
        print(f"{stem}: {len(methods)} methods → {len(cands)} candidates (+{added}), {total_s / 60:.0f} min to listen → {path}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--top", type=int, default=POOL_TOP)
    ap.add_argument("--methods", help="comma-separated; default: every method in out/<stem>/")
    ap.add_argument("stems", nargs="*")
    a = ap.parse_args()
    main(a.stems or sessions(), a.top, a.methods.split(",") if a.methods else None)
