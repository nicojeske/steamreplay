"""M15: export EfficientAT `mn10_as` (network only, no mel front-end) to ONNX, plus parity fixtures.

  uv run python export_onnx.py

Writes:
  out/mn10_as.onnx                                  the network: [B, 1, 128, 500] mel → [B, 527] logits
  ../../src-tauri/tests/fixtures/effat/*.f32        one 5 s window: input, mel banks, mel, logits (raw LE f32)
  out/<stem>/audio32.f32                            exactly the 32 kHz input the `effat_laugh` curves came from
"""

from __future__ import annotations

import hashlib
import json

import numpy as np
import torch

from common import LAUGH, OUT, ROOT, load_audio, sessions
from taggers import EfficientAT

FIXTURES = ROOT / "../../src-tauri/tests/fixtures/effat"
SR, WIN = 32000, 160000
# Make Way: a window with laughter, so the logits are not all near zero.
FIXTURE_STEM = "115461900__bg__bg_1445790_20261015_210120"


class Logits(torch.nn.Module):
    def __init__(self, model: torch.nn.Module):
        super().__init__()
        self.model = model

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.model(x)[0]


def main() -> None:
    torch.set_grad_enabled(False)
    m = EfficientAT()
    model = m.model.cpu().eval()
    mel = m.mel.cpu().eval()

    # --- fixture window ---------------------------------------------------------------------
    top = json.loads((OUT / FIXTURE_STEM / "effat_laugh.json").read_text())["regions"][0]
    x = load_audio(FIXTURE_STEM, SR)
    # curve[i] is the window that starts at i − 2 s (taggers.py shifts by round(2.5) = 2).
    start = int(top["peakS"] - 2) * SR
    w = x[start : start + WIN].astype(np.float32)
    assert len(w) == WIN
    spec = mel(torch.from_numpy(w)[None])  # [1, 128, 500]
    logits = model(spec.unsqueeze(1))[0]  # [1, 527]
    import torchaudio

    banks, _ = torchaudio.compliance.kaldi.get_mel_banks(128, 1024, SR, 0.0, mel.fmax, vtln_low=100.0, vtln_high=-500.0, vtln_warp_factor=1.0)
    banks = torch.nn.functional.pad(banks, (0, 1))
    assert mel.fmax == 15000, mel.fmax
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, a in [("input", w), ("mel_banks", banks), ("mel", spec[0]), ("logits", logits[0])]:
        np.asarray(a, dtype="<f4").tofile(FIXTURES / f"{name}.f32")
    laugh_idx = [m.labels.index(c) for c in LAUGH]
    probs = torch.sigmoid(logits[0])
    print(f"fixture at {start / SR:.0f} s: mel {tuple(spec.shape)}, laugh indices {laugh_idx}, laugh max {probs[laugh_idx].max():.3f}")

    # --- ONNX -------------------------------------------------------------------------------
    path = OUT / "mn10_as.onnx"
    # The torch reference comes first: after `torch.onnx.export` the module gave different logits.
    batch = torch.cat([spec, mel(torch.from_numpy(x[:WIN])[None])]).unsqueeze(1)
    ref = model(batch)[0].numpy()
    dummy = torch.zeros(2, 1, 128, 500)
    torch.onnx.export(
        Logits(model),
        (dummy,),
        str(path),
        input_names=["mel"],
        output_names=["logits"],
        dynamic_axes={"mel": {0: "batch"}, "logits": {0: "batch"}},
        opset_version=17,
        dynamo=False,
    )
    import onnxruntime as ort

    sess = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    got = sess.run(None, {"mel": batch.numpy()})[0]
    sha = hashlib.sha256(path.read_bytes()).hexdigest()
    print(f"{path}: {path.stat().st_size / 1e6:.1f} MB, sha256 {sha}, onnxruntime vs torch max abs diff {np.abs(ref - got).max():.2e}")

    # --- session inputs for the Rust parity test ---------------------------------------------
    for stem in sessions():
        out = OUT / stem / "audio32.f32"
        if not out.exists():
            load_audio(stem, SR).astype("<f4").tofile(out)
            print(f"{out}")


if __name__ == "__main__":
    main()
