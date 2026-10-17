//! EfficientAT's mel front-end (`AugmentMelSTFT` in eval mode), ported exactly: pre-emphasis,
//! `torch.stft` (n_fft 1024, Hann 800, hop 320, centred with reflect padding), power, Kaldi mel
//! banks (128 mels, 0–15 kHz), `ln(x + 1e-5)` and `(x + 4.5) / 5`.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex};

pub const SAMPLE_RATE: u32 = 32_000;
/// One window: 5 s. The model gives nonsense for much shorter input.
pub const WINDOW: usize = 5 * SAMPLE_RATE as usize;
pub const N_MELS: usize = 128;
/// Frames of one window: 1 + (WINDOW − 1) / HOP (pre-emphasis drops a sample).
pub const FRAMES: usize = 1 + (WINDOW - 1) / HOP;

const N_FFT: usize = 1024;
const WIN_LENGTH: usize = 800;
const HOP: usize = 320;
const BINS: usize = N_FFT / 2 + 1;
const FMIN: f64 = 0.0;
const FMAX: f64 = 15_000.0;
const PREEMPHASIS: f32 = 0.97;

fn mel_scale(f: f64) -> f64 {
    1127.0 * (1.0 + f / 700.0).ln()
}

/// `torchaudio.compliance.kaldi.get_mel_banks` without VTLN warping (factor 1), padded with a
/// zero column to `BINS`. Row-major `N_MELS × BINS`.
pub fn mel_banks() -> Vec<f32> {
    let fft_bin_width = f64::from(SAMPLE_RATE) / N_FFT as f64;
    let (lo, hi) = (mel_scale(FMIN), mel_scale(FMAX));
    let delta = (hi - lo) / (N_MELS + 1) as f64;
    let mut out = vec![0f32; N_MELS * BINS];
    for m in 0..N_MELS {
        let left = lo + m as f64 * delta;
        let centre = lo + (m as f64 + 1.0) * delta;
        let right = lo + (m as f64 + 2.0) * delta;
        for k in 0..N_FFT / 2 {
            let mel = mel_scale(fft_bin_width * k as f64);
            let up = (mel - left) / (centre - left);
            let down = (right - mel) / (right - centre);
            out[m * BINS + k] = up.min(down).max(0.0) as f32;
        }
    }
    out
}

/// One filter: the first non-zero FFT bin and its weights.
struct Filter {
    start: usize,
    weights: Vec<f32>,
}

/// Reusable per thread: the FFT plan and its buffers.
pub struct MelFrontend {
    filters: Vec<Filter>,
    /// Hann (symmetric) of `WIN_LENGTH`, centred in `N_FFT` zeros, as torch.stft pads it.
    window: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    power: Vec<f32>,
    padded: Vec<f32>,
}

impl Default for MelFrontend {
    fn default() -> Self {
        let banks = mel_banks();
        let filters = banks
            .chunks(BINS)
            .map(|row| {
                let start = row.iter().position(|w| *w > 0.0).unwrap_or(0);
                let end = row.iter().rposition(|w| *w > 0.0).map_or(start, |e| e + 1);
                Filter { start, weights: row[start..end].to_vec() }
            })
            .collect();
        let offset = (N_FFT - WIN_LENGTH) / 2;
        let mut window = vec![0f32; N_FFT];
        for i in 0..WIN_LENGTH {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (WIN_LENGTH - 1) as f64).cos();
            window[offset + i] = w as f32;
        }
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT);
        Self {
            filters,
            window,
            frame: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: fft.make_scratch_vec(),
            fft,
            power: vec![0.0; FRAMES * BINS],
            padded: Vec::with_capacity(WINDOW + N_FFT),
        }
    }
}

impl MelFrontend {
    /// The normalised log-mel spectrogram of one window of exactly [`WINDOW`] samples at 32 kHz,
    /// row-major `N_MELS × FRAMES` (the model's `[1, 128, 500]` input).
    pub fn compute(&mut self, x: &[f32]) -> Vec<f32> {
        assert_eq!(x.len(), WINDOW);
        // Pre-emphasis (one sample shorter), then reflect padding by N_FFT / 2 on both sides.
        let n = x.len() - 1;
        let y = |i: usize| x[i + 1] - PREEMPHASIS * x[i];
        let pad = N_FFT / 2;
        self.padded.clear();
        self.padded.extend((1..=pad).rev().map(y));
        self.padded.extend((0..n).map(y));
        self.padded.extend((0..pad).map(|k| y(n - 2 - k)));

        for t in 0..FRAMES {
            let src = &self.padded[t * HOP..t * HOP + N_FFT];
            for ((f, s), w) in self.frame.iter_mut().zip(src).zip(&self.window) {
                *f = s * w;
            }
            self.fft
                .process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch)
                .expect("buffer sizes come from the plan");
            for (p, c) in self.power[t * BINS..(t + 1) * BINS].iter_mut().zip(&self.spectrum) {
                *p = c.re * c.re + c.im * c.im;
            }
        }

        let mut out = vec![0f32; N_MELS * FRAMES];
        for (m, f) in self.filters.iter().enumerate() {
            for t in 0..FRAMES {
                let p = &self.power[t * BINS + f.start..t * BINS + f.start + f.weights.len()];
                let e: f32 = p.iter().zip(&f.weights).map(|(a, b)| a * b).sum();
                out[m * FRAMES + t] = ((e + 1e-5).ln() + 4.5) / 5.0;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::laughter::model::tests::fixture;

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn banks_match_torchaudio() {
        let d = max_diff(&mel_banks(), &fixture("mel_banks.f32"));
        assert!(d < 1e-4, "{d}"); // torchaudio computes in f32
    }

    #[test]
    fn mel_matches_pytorch() {
        assert_eq!(FRAMES, 500);
        let got = MelFrontend::default().compute(&fixture("input.f32"));
        let d = max_diff(&got, &fixture("mel.f32"));
        eprintln!("mel max abs diff {d:e}");
        assert!(d < 1e-3, "{d}");
    }

    #[test]
    fn silence_is_the_floor() {
        let got = MelFrontend::default().compute(&vec![0.0; WINDOW]);
        let floor = ((1e-5f32).ln() + 4.5) / 5.0;
        assert!(got.iter().all(|v| (v - floor).abs() < 1e-6));
    }
}
