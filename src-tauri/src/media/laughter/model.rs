//! The EfficientAT `mn10_as` network (M15), exported to ONNX without its mel front-end and run
//! with tract (pure Rust, so Windows needs no extra DLL).

use std::path::Path;

use tract_onnx::prelude::*;

use super::mel::{FRAMES, N_MELS};
use crate::error::{AppError, AppResult};

/// AudioSet classes.
pub const CLASSES: usize = 527;
/// Laughter, Baby laughter, Giggle, Snicker, Belly laugh, "Chuckle, chortle" (AudioSet order).
pub const LAUGH_CLASSES: [usize; 6] = [16, 17, 18, 19, 20, 21];
/// Windows per model call; the last batch is padded with silence.
pub const BATCH: usize = 8;

type Plan = std::sync::Arc<TypedSimplePlan>;

pub struct Model {
    plan: Plan,
}

fn model_err(e: impl std::fmt::Display) -> AppError {
    AppError::Model(format!("laughter model: {e}"))
}

impl Model {
    /// Loads and optimises the network for `[BATCH, 1, 128, 500]` inputs. Blocking, ~1 s.
    pub fn load(path: &Path) -> AppResult<Self> {
        if !path.is_file() {
            return Err(AppError::Model(format!(
                "laughter model not found at {} (developers: run `pnpm fetch-model`)",
                path.display()
            )));
        }
        let plan = tract_onnx::onnx()
            .model_for_path(path)
            .and_then(|m| m.with_input_fact(0, f32::fact([BATCH, 1, N_MELS, FRAMES]).into()))
            .and_then(|m| m.into_optimized())
            .and_then(|m| m.into_runnable())
            .map_err(model_err)?;
        Ok(Self { plan })
    }

    /// Laughter score (max sigmoid over [`LAUGH_CLASSES`]) of up to [`BATCH`] mel spectrograms of
    /// `N_MELS × FRAMES` values each.
    pub fn laughter(&self, mels: &[Vec<f32>]) -> AppResult<Vec<f32>> {
        Ok(self.logits(mels)?.iter().map(|l| laughter(l)).collect())
    }

    /// Raw logits, one row per input.
    pub fn logits(&self, mels: &[Vec<f32>]) -> AppResult<Vec<Vec<f32>>> {
        assert!(mels.len() <= BATCH, "at most {BATCH} windows per call");
        let per = N_MELS * FRAMES;
        let mut data = vec![0f32; BATCH * per];
        for (i, m) in mels.iter().enumerate() {
            data[i * per..(i + 1) * per].copy_from_slice(m);
        }
        let input = Tensor::from_shape(&[BATCH, 1, N_MELS, FRAMES], &data).map_err(model_err)?;
        let out = self.plan.run(tvec!(input.into())).map_err(model_err)?;
        let view = out[0].try_as_plain_ram().map_err(model_err)?;
        let logits = view.as_slice::<f32>().map_err(model_err)?;
        Ok(logits.chunks(CLASSES).take(mels.len()).map(<[f32]>::to_vec).collect())
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// The max over the laughter classes, after the sigmoid.
pub fn laughter(logits: &[f32]) -> f32 {
    LAUGH_CLASSES.iter().map(|&i| sigmoid(logits[i])).fold(0.0, f32::max)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn fixture(name: &str) -> Vec<f32> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/effat").join(name);
        std::fs::read(&p).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    /// The fetched model, or `None` (the test is then skipped) if `pnpm fetch-model` was not run.
    pub fn model_path() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models/mn10_as.onnx");
        if p.is_file() {
            Some(p)
        } else {
            eprintln!("{} missing (run `pnpm fetch-model`), skipping", p.display());
            None
        }
    }

    #[test]
    fn logits_match_pytorch() {
        let Some(path) = model_path() else { return };
        let model = Model::load(&path).unwrap();
        let mel = fixture("mel.f32");
        let want = fixture("logits.f32");
        let got = model.logits(&[mel.clone(), vec![0.0; mel.len()]]).unwrap();
        let diff = got[0].iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        eprintln!("logits max abs diff {diff:e}");
        assert!(diff < 1e-3, "{diff}");
        assert!((laughter(&got[0]) - laughter(&want)).abs() < 1e-4);
        // The fixture is the top laughter window of the Make Way session.
        assert!((laughter(&want) - 0.346).abs() < 1e-3, "{}", laughter(&want));
    }

    /// Spike: `cargo test --release --lib laughter::model::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing() {
        let Some(path) = model_path() else { return };
        let model = std::sync::Arc::new(Model::load(&path).unwrap());
        let mel = fixture("mel.f32");
        let batch = vec![mel; BATCH];
        model.logits(&batch).unwrap(); // warm-up
        let calls = 8;
        let t = std::time::Instant::now();
        for _ in 0..calls {
            model.logits(&batch).unwrap();
        }
        let per_window = t.elapsed().as_secs_f64() / (calls * BATCH) as f64;
        eprintln!("1 thread: {:.1} ms per window → {:.0} s per audio hour", per_window * 1e3, per_window * 3600.0);
        for threads in [4, 8] {
            let t = std::time::Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    let (m, b) = (model.clone(), &batch);
                    s.spawn(move || {
                        for _ in 0..calls {
                            m.logits(b).unwrap();
                        }
                    });
                }
            });
            let per_window = t.elapsed().as_secs_f64() / (threads * calls * BATCH) as f64;
            eprintln!("{threads} threads: {:.0} s per audio hour", per_window * 3600.0);
        }
    }
}
