//! Audio extraction: Steam's AAC chunks → mono 8 kHz f32 → 10 ms min/max/RMS bins.
//!
//! ffmpeg's DASH demuxer returned only 3 of 6 s for a saved clip, so we never give it
//! `session.mpd`. Instead, each gap-free run of chunks is piped into ffmpeg's stdin as
//! `init-stream1.m4s` + `chunk-stream1-N.m4s`… (plain fragmented MP4). This also handles pruning
//! after the manifest was written, and needs no long `concat:` command line (Windows limit).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use specta::Type;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use super::chunks::{AUDIO_INIT, VIDEO_INIT, audio_chunk, probe_start, video_chunk};
use super::ffmpeg::{self, FfmpegPaths, ProgressFn};
use crate::error::{AppError, AppResult};
use crate::steam::recordings::ChunkInventory;

pub const SAMPLE_RATE: u32 = 8000;
pub const BIN_MS: u16 = 10;
/// 80 samples = 10 ms.
pub const SAMPLES_PER_BIN: usize = (SAMPLE_RATE as usize) * (BIN_MS as usize) / 1000;
pub const BINS_PER_S: f64 = 1000.0 / BIN_MS as f64;

/// `[min, max, rms]` of one bin.
pub type Bin = [f32; 3];

/// Consecutive chunk numbers whose audio file exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub first: u32,
    pub last: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    #[specta(type = specta_typescript::Number)]
    pub start_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub end_s: f64,
}

/// Gap-free runs inside `[inv.first, inv.last]`. A missing video or audio chunk ends a run.
pub fn plan_runs(dir: &Path, inv: &ChunkInventory) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    if inv.count == 0 {
        return runs;
    }
    for n in inv.first..=inv.last {
        if inv.missing.binary_search(&n).is_ok() || !dir.join(audio_chunk(n)).is_file() {
            continue;
        }
        match runs.last_mut() {
            Some(r) if r.last + 1 == n => r.last = n,
            _ => runs.push(Run { first: n, last: n }),
        }
    }
    runs
}

/// The parts of `[inv.first, inv.last]` not covered by `runs`, in video time.
pub fn gaps(runs: &[Run], inv: &ChunkInventory, segment_s: f64) -> Vec<Gap> {
    let mut out = Vec::new();
    if inv.count == 0 {
        return out;
    }
    let t = |n: u32| f64::from(n - inv.first) * segment_s;
    let mut next = inv.first;
    for r in runs {
        if r.first > next {
            out.push(Gap { start_s: t(next), end_s: t(r.first) });
        }
        next = r.last + 1;
    }
    if next <= inv.last {
        out.push(Gap { start_s: t(next), end_s: t(inv.last + 1) });
    }
    out
}

/// Number of level-0 bins for the chunk range of the inventory.
pub fn total_bins(inv: &ChunkInventory, segment_s: f64) -> usize {
    if inv.count == 0 { 0 } else { (f64::from(inv.last - inv.first + 1) * segment_s * BINS_PER_S).round() as usize }
}

/// Folds a stream of samples into bins.
#[derive(Default)]
pub struct Binner {
    pub bins: Vec<Bin>,
    min: f32,
    max: f32,
    sum_sq: f64,
    n: usize,
}

impl Binner {
    pub fn push(&mut self, s: f32) {
        if self.n == 0 {
            self.min = s;
            self.max = s;
        } else {
            self.min = self.min.min(s);
            self.max = self.max.max(s);
        }
        self.sum_sq += f64::from(s) * f64::from(s);
        self.n += 1;
        if self.n == SAMPLES_PER_BIN {
            self.flush();
        }
    }

    fn flush(&mut self) {
        let rms = (self.sum_sq / self.n as f64).sqrt() as f32;
        self.bins.push([self.min, self.max, rms]);
        self.sum_sq = 0.0;
        self.n = 0;
    }

    /// Keeps a last partial bin; it still counts as 10 ms.
    pub fn finish(mut self) -> Vec<Bin> {
        if self.n > 0 {
            self.flush();
        }
        self.bins
    }
}

/// Decodes one run. `written` counts bytes fed to ffmpeg for progress.
async fn decode_run(
    paths: &FfmpegPaths,
    dir: &Path,
    run: &Run,
    written: Arc<AtomicU64>,
    on_progress: ProgressFn,
    total_bytes: u64,
    cancel: &CancellationToken,
) -> AppResult<Vec<Bin>> {
    let mut cmd = ffmpeg::ffmpeg_command(paths, false);
    cmd.args(["-i", "pipe:0", "-map", "0:a:0", "-vn", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .stdin(std::process::Stdio::piped());
    let mut child = ffmpeg::spawn(&mut cmd)?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let files: Vec<PathBuf> = std::iter::once(dir.join(AUDIO_INIT))
        .chain((run.first..=run.last).map(|n| dir.join(audio_chunk(n))))
        .collect();
    let writer = tokio::spawn(async move {
        let mut last_reported = 0.0;
        for f in files {
            let data = tokio::fs::read(&f).await.map_err(|e| AppError::io(f.display(), e))?;
            if stdin.write_all(&data).await.is_err() {
                break; // ffmpeg exited; its stderr tells why
            }
            let done = written.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
            let fraction = done as f64 / total_bytes.max(1) as f64;
            if fraction - last_reported >= 0.005 {
                last_reported = fraction;
                on_progress(fraction.min(1.0));
            }
        }
        Ok::<_, AppError>(())
    });

    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = tokio::spawn(async move {
        let mut binner = Binner::default();
        let mut buf = vec![0u8; 1 << 16];
        let mut carry = 0usize;
        loop {
            let n = stdout.read(&mut buf[carry..]).await?;
            if n == 0 {
                break;
            }
            let len = carry + n;
            let whole = len - len % 4;
            for b in buf[..whole].as_chunks::<4>().0 {
                binner.push(f32::from_le_bytes(*b));
            }
            buf.copy_within(whole..len, 0);
            carry = len - whole;
        }
        Ok::<_, std::io::Error>(binner.finish())
    });
    let stderr = ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"), None);

    let status = ffmpeg::wait_or_kill(&mut child, cancel).await;
    let write_result = writer.await.map_err(|e| AppError::Io(format!("writer task: {e}")))?;
    let status = status?;
    let bins = reader
        .await
        .map_err(|e| AppError::Io(format!("reader task: {e}")))?
        .map_err(|e| AppError::Ffmpeg(format!("reading ffmpeg output: {e}")))?;
    ffmpeg::check_exit(status, stderr, "audio decode").await?;
    write_result?;
    Ok(bins)
}

/// Decodes all audio of the inventory into level-0 bins that start at video time 0.
/// Missing chunks become silent (zero) bins.
pub async fn extract(
    paths: &FfmpegPaths,
    dir: &Path,
    inv: &ChunkInventory,
    segment_s: f64,
    on_progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<(Vec<Bin>, Vec<Gap>)> {
    let runs = plan_runs(dir, inv);
    if runs.is_empty() {
        return Err(AppError::NotFound(format!("no audio chunks in {}", dir.display())));
    }
    let total = total_bins(inv, segment_s);
    let init_len = std::fs::metadata(dir.join(AUDIO_INIT)).map_err(|e| AppError::io(dir.join(AUDIO_INIT).display(), e))?.len();
    let total_bytes: u64 = runs
        .iter()
        .flat_map(|r| r.first..=r.last)
        .map(|n| std::fs::metadata(dir.join(audio_chunk(n))).map_or(0, |m| m.len()))
        .sum::<u64>()
        + init_len * runs.len() as u64;

    // Video time 0 is the start of the first video chunk. The audio track starts a bit later
    // (0.358 s in the sample session); raw f32 output drops that, so place each run by its
    // own start time.
    let video_start = probe_start(paths, dir, VIDEO_INIT, &video_chunk(inv.first), cancel).await;
    let video_start = match video_start {
        Err(AppError::Cancelled) => return Err(AppError::Cancelled),
        Err(e) => {
            log::warn!("probing video start: {e}; assuming audio and video start together");
            None
        }
        Ok(t) => Some(t),
    };

    let written = Arc::new(AtomicU64::new(0));
    let mut out = vec![[0.0f32; 3]; total];
    for run in &runs {
        let nominal = f64::from(run.first - inv.first) * segment_s;
        let start_s = match video_start {
            Some(v0) => match probe_start(paths, dir, AUDIO_INIT, &audio_chunk(run.first), cancel).await {
                Ok(a) => a - v0,
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(e) => {
                    log::warn!("probing audio start of chunk {}: {e}", run.first);
                    nominal
                }
            },
            None => nominal,
        };
        let bins = decode_run(paths, dir, run, written.clone(), on_progress.clone(), total_bytes, cancel).await?;
        let offset = (start_s * BINS_PER_S).round() as i64;
        log::info!("audio run {}–{}: {} bins at bin {offset}", run.first, run.last, bins.len());
        for (i, b) in bins.into_iter().enumerate() {
            let at = offset + i as i64;
            if (0..total as i64).contains(&at) {
                out[at as usize] = b;
            }
        }
    }
    on_progress(1.0);
    Ok((out, gaps(&runs, inv, segment_s)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn binner() {
        let mut b = Binner::default();
        for i in 0..(SAMPLES_PER_BIN + 2) {
            b.push(if i % 2 == 0 { 0.5 } else { -0.5 });
        }
        let bins = b.finish();
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[0], [-0.5, 0.5, 0.5]);
        assert_eq!(bins[1], [-0.5, 0.5, 0.5]); // partial bin is kept
    }

    #[test]
    fn runs_and_gaps() {
        let dir = tempfile::tempdir().unwrap();
        for n in [10, 11, 12, 14, 16, 17] {
            fs::write(dir.path().join(audio_chunk(n)), b"a").unwrap();
        }
        // 13: both missing; 15: video exists, audio missing; 18: audio missing at the end.
        let inv = ChunkInventory { first: 10, last: 18, count: 8, missing: vec![13] };
        let runs = plan_runs(dir.path(), &inv);
        assert_eq!(
            runs,
            vec![Run { first: 10, last: 12 }, Run { first: 14, last: 14 }, Run { first: 16, last: 17 }]
        );
        assert_eq!(
            gaps(&runs, &inv, 3.0),
            vec![
                Gap { start_s: 9.0, end_s: 12.0 },
                Gap { start_s: 15.0, end_s: 18.0 },
                Gap { start_s: 24.0, end_s: 27.0 }
            ]
        );
        assert_eq!(total_bins(&inv, 3.0), 2700);
        assert!(plan_runs(dir.path(), &ChunkInventory::default()).is_empty());
    }

    /// Needs ffmpeg/ffprobe on PATH. Audio-only copy of the 2-chunk sample clip.
    #[tokio::test]
    #[ignore]
    async fn extract_clip_fixture() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clip_1478500_audio");
        let paths = ffmpeg::locate(None).unwrap();
        let inv = ChunkInventory { first: 1, last: 2, count: 2, missing: vec![] };
        // The fixture has no video chunks, so the video probe fails and runs use nominal times.
        let (bins, gaps) =
            extract(&paths, &dir, &inv, 3.0, Arc::new(|_| {}), &CancellationToken::new()).await.unwrap();
        assert_eq!(bins.len(), 600);
        assert!(gaps.is_empty());
        let silent_tail = bins.iter().rev().take_while(|b| b[2] == 0.0).count();
        assert!(silent_tail <= 3, "decoded audio should cover ≈6 s, {silent_tail} empty bins at the end");
        assert!(bins.iter().any(|b| b[2] > 1e-4));
    }
}
