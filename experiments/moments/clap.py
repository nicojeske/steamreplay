"""CLAP zero-shot: similarity of 5 s windows to reaction prompts vs. negative prompts.

  uv run python clap.py [--input vocals] [stems…]

Score = summed softmax probability of the positive prompts over all prompts (temperature from the
model). Only the audio tower would ship; the text embeddings are fixed.
"""

from __future__ import annotations

import argparse

import numpy as np
import soundfile as sf
import torch
from scipy.signal import resample_poly
from transformers import ClapModel, ClapProcessor

from common import OUT, Timer, batched, load_audio, save_method, sessions, windows

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
NAME = "laion/clap-htsat-unfused"
SR, WIN_S, HOP_S, BATCH = 48000, 5.0, 1.0, 64

POSITIVE = [
    "a group of friends laughing hysterically",
    "people laughing loudly at a joke",
    "someone screaming in surprise",
    "excited people shouting and cheering",
    "a gamer yelling in excitement",
]
NEGATIVE = [
    "people talking calmly",
    "a quiet conversation over voice chat",
    "video game background music",
    "gunshots and explosions in a video game",
    "game sound effects",
    "silence",
]


def main(stems: list[str], vocals: bool) -> None:
    model = ClapModel.from_pretrained(NAME).to(DEVICE).eval()
    proc = ClapProcessor.from_pretrained(NAME)
    with torch.no_grad():
        t = proc(text=POSITIVE + NEGATIVE, return_tensors="pt", padding=True).to(DEVICE)
        text = model.get_text_features(**t).pooler_output  # projected (transformers 5)
        text = torch.nn.functional.normalize(text, dim=-1)
        scale = model.logit_scale_a.exp()
    suffix = "_voc" if vocals else ""
    for stem in stems:
        if vocals:
            x, _ = sf.read(OUT / stem / "vocals.wav", dtype="float32")
            x = resample_poly(x, 3, 1).astype(np.float32)
        else:
            x = load_audio(stem, SR)
        rows = []
        with Timer() as tm:
            for batch in batched(windows(x, SR, WIN_S, HOP_S), BATCH):
                inp = proc(audio=[w for _, w in batch], sampling_rate=SR, return_tensors="pt").to(DEVICE)
                with torch.no_grad():
                    a = torch.nn.functional.normalize(model.get_audio_features(**inp).pooler_output, dim=-1)
                    rows.append(torch.softmax(scale * a @ text.T, dim=-1).cpu().numpy())
        p = np.concatenate(rows).astype(np.float32)
        hz = 1.0 / HOP_S
        shift = int(round(WIN_S / 2 * hz))
        p = np.concatenate([np.repeat(p[:1], shift, axis=0), p])[: len(p)]
        curve = p[:, : len(POSITIVE)].sum(axis=1)
        save_method(stem, "clap" + suffix, curve, hz, tm.s, classes=np.array(POSITIVE + NEGATIVE), probs=p)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--input", choices=["mix", "vocals"], default="mix")
    ap.add_argument("stems", nargs="*")
    a = ap.parse_args()
    main(a.stems or sessions(), a.input == "vocals")
