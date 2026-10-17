"""Speech arousal (audeering wav2vec2 MSP-dim) on 3 s windows, weighted by Silero-VAD speech share.

  uv run python arousal.py [--input vocals] [stems…]

"Excited voice" without needing a laugh or a scream class.
"""

from __future__ import annotations

import argparse

import numpy as np
import soundfile as sf
import torch
from silero_vad import get_speech_timestamps, load_silero_vad
from torch import nn
from transformers import Wav2Vec2Model, Wav2Vec2PreTrainedModel, Wav2Vec2Processor

from common import OUT, Timer, batched, load_audio, save_method, sessions, windows

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"
NAME = "audeering/wav2vec2-large-robust-12-ft-emotion-msp-dim"
SR, WIN_S, HOP_S, BATCH = 16000, 3.0, 1.0, 64


class RegressionHead(nn.Module):
    def __init__(self, config):
        super().__init__()
        self.dense = nn.Linear(config.hidden_size, config.hidden_size)
        self.dropout = nn.Dropout(config.final_dropout)
        self.out_proj = nn.Linear(config.hidden_size, config.num_labels)

    def forward(self, x):
        return self.out_proj(self.dropout(torch.tanh(self.dense(self.dropout(x)))))


class EmotionModel(Wav2Vec2PreTrainedModel):
    """From the model card. Outputs arousal, dominance, valence in ~[0, 1]."""

    def __init__(self, config):
        super().__init__(config)
        self.wav2vec2 = Wav2Vec2Model(config)
        self.classifier = RegressionHead(config)
        self.post_init()

    def forward(self, input_values):
        h = self.wav2vec2(input_values)[0].mean(dim=1)
        return self.classifier(h)


def speech_mask(x: np.ndarray, vad) -> np.ndarray:
    """Per-sample bool: Silero VAD says speech."""
    mask = np.zeros(len(x), dtype=bool)
    for ts in get_speech_timestamps(torch.from_numpy(x), vad, sampling_rate=SR):
        mask[ts["start"] : ts["end"]] = True
    return mask


def main(stems: list[str], vocals: bool) -> None:
    proc = Wav2Vec2Processor.from_pretrained(NAME)
    model = EmotionModel.from_pretrained(NAME).to(DEVICE).eval().half()
    vad = load_silero_vad()
    suffix = "_voc" if vocals else ""
    for stem in stems:
        x = sf.read(OUT / stem / "vocals.wav", dtype="float32")[0] if vocals else load_audio(stem, SR)
        rows, speech = [], []
        with Timer() as t:
            mask = speech_mask(x, vad)
            for batch in batched(windows(x, SR, WIN_S, HOP_S), BATCH):
                inp = proc([w for _, w in batch], sampling_rate=SR, return_tensors="pt").input_values.to(DEVICE).half()
                with torch.no_grad():
                    rows.append(model(inp).float().cpu().numpy())
                win = int(WIN_S * SR)
                speech.extend(mask[s : s + win].mean() if s < len(mask) else 0.0 for s, _ in batch)
        adv = np.concatenate(rows).astype(np.float32)
        sp = np.array(speech, dtype=np.float32)
        hz = 1.0 / HOP_S
        shift = int(round(WIN_S / 2 * hz))
        adv = np.concatenate([np.repeat(adv[:1], shift, axis=0), adv])[: len(adv)]
        sp = np.concatenate([np.repeat(sp[:1], shift), sp])[: len(sp)]
        curve = adv[:, 0] * np.minimum(1.0, sp * 2)  # arousal, only where people talk
        save_method(
            stem,
            "arousal" + suffix,
            curve,
            hz,
            t.s,
            classes=np.array(["arousal", "dominance", "valence", "speech"]),
            probs=np.column_stack([adv, sp]),
        )


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--input", choices=["mix", "vocals"], default="mix")
    ap.add_argument("stems", nargs="*")
    a = ap.parse_args()
    main(a.stems or sessions(), a.input == "vocals")
