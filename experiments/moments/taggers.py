"""AudioSet taggers on sliding windows → reaction score curves.

  uv run python taggers.py panns|effat|effat40|ast [--input vocals] [stems…]

Writes methods `<model>` (max over all reaction classes) and `<model>_laugh` (laughter classes only).
With `--input vocals` the taggers run on the Demucs vocals stem (`out/<stem>/vocals.wav`) and the
methods get a `_voc` suffix.
"""

from __future__ import annotations

import argparse
import sys

import numpy as np
import soundfile as sf
import torch

from common import CONTEXT, LAUGH, OUT, REACTION, ROOT, Timer, batched, class_indices, load_audio, save_method, sessions, windows

DEVICE = "cuda" if torch.cuda.is_available() else "cpu"


class Panns:
    sr, win_s, hop_s, batch = 32000, 2.0, 0.5, 128

    def __init__(self):
        from panns_inference import AudioTagging, labels

        self.at = AudioTagging(checkpoint_path=None, device=DEVICE)
        self.labels = list(labels)

    def __call__(self, x: np.ndarray) -> np.ndarray:
        clipwise, _ = self.at.inference(x)
        return clipwise


class EfficientAT:
    """fschmid56/EfficientAT `mn10_as` (cloned into vendor/EfficientAT). Subclasses pick a larger width."""

    pretrained, width = "mn10_as", 1.0

    # Needs ≥ 4 s of context: 2 s windows give ~1.0 for random classes.
    sr, win_s, hop_s, batch = 32000, 5.0, 1.0, 128

    def __init__(self):
        import os

        repo = ROOT / "vendor/EfficientAT"
        sys.path.insert(0, str(repo))
        cwd = os.getcwd()
        os.chdir(repo)  # the repo reads its metadata and stores weights relative to itself
        try:
            from models.mn.model import get_model
            from models.preprocess import AugmentMelSTFT

            self.model = get_model(width_mult=self.width, pretrained_name=self.pretrained).to(DEVICE).eval()
        finally:
            os.chdir(cwd)
        from panns_inference import labels

        self.mel = AugmentMelSTFT(n_mels=128, sr=32000, win_length=800, hopsize=320).to(DEVICE).eval()
        self.labels = list(labels)

    @torch.no_grad()
    def __call__(self, x: np.ndarray) -> np.ndarray:
        w = torch.from_numpy(x).to(DEVICE)
        logits, _ = self.model(self.mel(w).unsqueeze(1))
        return torch.sigmoid(logits).cpu().numpy()


class Ast:
    sr, win_s, hop_s, batch = 16000, 3.0, 1.0, 64
    NAME = "MIT/ast-finetuned-audioset-10-10-0.4593"

    def __init__(self):
        from transformers import ASTFeatureExtractor, ASTForAudioClassification

        self.fe = ASTFeatureExtractor.from_pretrained(self.NAME)
        self.model = ASTForAudioClassification.from_pretrained(self.NAME).to(DEVICE).eval().half()
        self.labels = [self.model.config.id2label[i] for i in range(self.model.config.num_labels)]

    @torch.no_grad()
    def __call__(self, x: np.ndarray) -> np.ndarray:
        f = self.fe(list(x), sampling_rate=16000, return_tensors="pt")["input_values"].to(DEVICE).half()
        return torch.sigmoid(self.model(f).logits.float()).cpu().numpy()


class EfficientAT40(EfficientAT):
    """`mn40_as`: 4× wider, ~68 M params, higher AudioSet mAP (.484 vs .471)."""

    pretrained, width, batch = "mn40_as", 4.0, 64


MODELS = {"panns": Panns, "effat": EfficientAT, "effat40": EfficientAT40, "ast": Ast}


def run(name: str, stems: list[str], vocals: bool) -> None:
    m = MODELS[name]()
    keep = class_indices(m.labels, REACTION + CONTEXT)
    kept_names = [m.labels[i] for i in keep]
    laugh = [kept_names.index(c) for c in LAUGH if c in kept_names]
    react = [kept_names.index(c) for c in REACTION if c in kept_names]
    suffix = "_voc" if vocals else ""
    for stem in stems:
        if vocals:
            x, sr = sf.read(OUT / stem / "vocals.wav", dtype="float32")
            assert sr == 16000
            if m.sr != 16000:
                from scipy.signal import resample_poly

                x = resample_poly(x, m.sr // 16000, 1).astype(np.float32)
        else:
            x = load_audio(stem, m.sr)
        rows = []
        with Timer() as t:
            for batch in batched(windows(x, m.sr, m.win_s, m.hop_s), m.batch):
                probs = m(np.stack([w for _, w in batch]))
                rows.append(probs[:, keep])
        probs = np.concatenate(rows).astype(np.float32)
        hz = 1.0 / m.hop_s
        # A window's score belongs to its centre; shift so curve[i] is the window centred at i/hz.
        shift = int(round((m.win_s / 2) * hz))
        probs = np.concatenate([np.repeat(probs[:1], shift, axis=0), probs])[: len(probs)]
        for method, cols in [(name + suffix, react), (f"{name}_laugh{suffix}", laugh)]:
            save_method(stem, method, probs[:, cols].max(axis=1), hz, t.s, classes=np.array(kept_names), probs=probs)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("model", choices=MODELS)
    ap.add_argument("--input", choices=["mix", "vocals"], default="mix")
    ap.add_argument("stems", nargs="*")
    a = ap.parse_args()
    run(a.model, a.stems or sessions(), a.input == "vocals")
