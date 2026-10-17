"""Demucs `htdemucs` vocals stem of each session → out/<stem>/vocals.wav (16 kHz mono).

Voice chat and game audio share one track; the taggers may do better on the voices alone.
  uv run python vocals.py [stems…]
"""

from __future__ import annotations

import sys

import numpy as np
import soundfile as sf
import torch
from demucs.apply import apply_model
from demucs.pretrained import get_model
from scipy.signal import resample_poly

from common import OUT, Timer, load_audio, sessions

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
BLOCK_S = 600  # demucs splits further inside; this only bounds host memory


def main(stems: list[str]) -> None:
    model = get_model("htdemucs").to(DEVICE).eval()
    vi = model.sources.index("vocals")
    sr = model.samplerate  # 44100
    for stem in stems:
        x16 = load_audio(stem, 16000)
        out = []
        with Timer() as t:
            for a in range(0, len(x16), BLOCK_S * 16000):
                block = resample_poly(x16[a : a + BLOCK_S * 16000], 441, 160).astype(np.float32)
                wav = torch.from_numpy(np.stack([block, block]))[None]
                with torch.no_grad():
                    src = apply_model(model, wav, device=DEVICE, split=True, overlap=0.1, progress=False)[0, vi]
                v = src.mean(0).cpu().numpy()
                out.append(resample_poly(v, 160, 441).astype(np.float32)[: min(BLOCK_S * 16000, len(x16) - a)])
        voc = np.concatenate(out)
        (OUT / stem).mkdir(parents=True, exist_ok=True)
        sf.write(OUT / stem / "vocals.wav", voc, 16000, subtype="FLOAT")
        print(f"{stem}: vocals in {t.s:.1f} s for {len(x16) / 16000 / 3600:.2f} h", flush=True)


if __name__ == "__main__":
    main(sys.argv[1:] or sessions())
