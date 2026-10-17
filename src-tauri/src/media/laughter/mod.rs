//! Laughter detection (M15): EfficientAT `mn10_as` on 5 s windows with a 1 s hop, the laughter
//! classes' max per window → a 1 Hz curve → regions. Reproduces `experiments/moments/taggers.py`.

pub mod mel;
pub mod model;

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};

use lru::LruCache;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::audio::{self, Block};
use super::ffmpeg::{FfmpegPaths, ProgressFn};
use super::spikes::Spike;
use crate::error::{AppError, AppResult};
use crate::steam::recordings::ChunkInventory;
use mel::{MelFrontend, SAMPLE_RATE, WINDOW};
use model::{BATCH, Model};

/// Hop between windows: 1 s.
const HOP: usize = SAMPLE_RATE as usize;
/// Curve values per second.
pub const CURVE_HZ: f64 = 1.0;
/// `curve[i]` is the window that starts at `i − SHIFT` s, so each score sits at its window's
/// centre. taggers.py: `round(2.5)`, which Python rounds to 2.
const SHIFT: usize = 2;
/// The sha256 of the bundled `mn10_as.onnx` (scripts/fetch-model.ts pins the same value).
pub const MODEL_SHA256: &str = "b59b560b0d64f5a094bb5b2b40fb20a65eeb4c9a3e5cd8b183c66f4265d0f978";
/// The audio goes through 16 kHz on its way to 32 kHz, as in M14 (its dump was 16 kHz): the model
/// only hears up to 8 kHz. See docs/specs/15-laughter.md §6 for why.
const DECODE_FILTER: &str = "aresample=16000,aresample=32000";
/// Curves kept in memory.
const LOADED_CAPACITY: usize = 4;

/// Number of windows for `total` samples, as in `common.windows`: one per hop while a full
/// window fits, plus the zero-padded tail; at least one.
pub fn window_count(total: usize) -> usize {
    let span = (total as i64 - WINDOW as i64 + HOP as i64).max(1) as usize;
    span.div_ceil(HOP)
}

/// Worker threads: leave two cores for the UI and the preview, at most 8.
fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, NonZeroUsize::get).saturating_sub(2).clamp(1, 8)
}

/// Turns placed sample blocks into 5 s windows every 1 s. Gaps become zeros; where a run overlaps
/// the previous one, the later samples win while they are still buffered.
struct WindowFeeder {
    total: usize,
    count: usize,
    /// Samples from absolute index `base`.
    buf: Vec<f32>,
    base: usize,
    /// Where the next pushed sample goes.
    pos: i64,
    /// The next window to hand out.
    next: usize,
}

impl WindowFeeder {
    fn new(total: usize) -> Self {
        Self { total, count: window_count(total), buf: Vec::new(), base: 0, pos: 0, next: 0 }
    }

    fn seek(&mut self, at: i64) {
        self.pos = at;
    }

    fn push(&mut self, mut v: &[f32]) {
        let mut pos = self.pos;
        self.pos += v.len() as i64;
        if pos < self.base as i64 {
            let skip = ((self.base as i64 - pos) as usize).min(v.len());
            v = &v[skip..];
            pos += skip as i64;
        }
        let pos = pos as usize;
        if v.is_empty() || pos >= self.total {
            return;
        }
        let v = &v[..v.len().min(self.total - pos)];
        let at = pos - self.base;
        if at > self.buf.len() {
            self.buf.resize(at, 0.0);
        }
        let overlap = (self.buf.len() - at).min(v.len());
        self.buf[at..at + overlap].copy_from_slice(&v[..overlap]);
        self.buf.extend_from_slice(&v[overlap..]);
    }

    /// No more samples: the rest of the session is silence.
    fn finish(&mut self) {
        let len = self.total.saturating_sub(self.base);
        if self.buf.len() < len {
            self.buf.resize(len, 0.0);
        }
    }

    /// The next complete window, or after `finish` the zero-padded tail ones.
    fn next_window(&mut self, finished: bool) -> Option<(usize, Vec<f32>)> {
        if self.next >= self.count {
            return None;
        }
        let start = self.next * HOP - self.base;
        if !finished && self.buf.len() < start + WINDOW {
            return None;
        }
        let end = (start + WINDOW).min(self.buf.len());
        let mut w = self.buf[start.min(end)..end].to_vec();
        w.resize(WINDOW, 0.0);
        let k = self.next;
        self.next += 1;
        // Drop consumed samples now and then (draining every window would copy 5 s each time).
        let consumed = (self.next * HOP - self.base).min(self.buf.len());
        if consumed >= 4 * WINDOW {
            self.buf.drain(..consumed);
            self.base += consumed;
        }
        Some((k, w))
    }
}

/// Window index and samples, on their way to a worker.
type Job = (usize, Vec<f32>);

struct Shared<'a> {
    model: &'a Model,
    jobs: Mutex<std_mpsc::Receiver<Job>>,
    scores: Mutex<Vec<f32>>,
    done: AtomicUsize,
    on_progress: &'a ProgressFn,
    cancel: &'a CancellationToken,
}

impl Shared<'_> {
    fn drain(&self) {
        while self.jobs.lock().expect("jobs lock").recv().is_ok() {}
    }

    fn score(&self, ks: &mut Vec<usize>, mels: &mut Vec<Vec<f32>>) -> AppResult<()> {
        if ks.is_empty() {
            return Ok(());
        }
        let found = self.model.laughter(mels)?;
        let mut scores = self.scores.lock().expect("scores lock");
        let n = scores.len();
        for (&k, s) in ks.iter().zip(found) {
            scores[k] = s;
        }
        drop(scores);
        let before = self.done.fetch_add(ks.len(), Ordering::Relaxed);
        let after = before + ks.len();
        // Report every 0.5 %.
        if before * 200 / n != after * 200 / n {
            (self.on_progress)(after as f64 / n as f64);
        }
        ks.clear();
        mels.clear();
        Ok(())
    }

    /// Takes windows until the feeder is done; mel per window, the model per batch.
    fn work(&self) -> AppResult<()> {
        let mut front = MelFrontend::default();
        let (mut ks, mut mels) = (Vec::with_capacity(BATCH), Vec::with_capacity(BATCH));
        let result = loop {
            if self.cancel.is_cancelled() {
                break Err(AppError::Cancelled);
            }
            let job = self.jobs.lock().expect("jobs lock").recv();
            let Ok((k, w)) = job else { break self.score(&mut ks, &mut mels) };
            ks.push(k);
            mels.push(front.compute(&w));
            if ks.len() == BATCH
                && let Err(e) = self.score(&mut ks, &mut mels)
            {
                break Err(e);
            }
        };
        if result.is_err() {
            // Stop the decode and the feeder, and keep the feeder from blocking on a full queue.
            self.cancel.cancel();
            self.drain();
        }
        result
    }
}

/// Feeds the blocks from the decoder to the workers. Blocking.
fn feed(rx: &mut mpsc::Receiver<Block>, total: usize, jobs: &std_mpsc::SyncSender<Job>, cancel: &CancellationToken) -> AppResult<()> {
    let mut feeder = WindowFeeder::new(total);
    let send = |feeder: &mut WindowFeeder, finished: bool| -> AppResult<()> {
        while let Some(w) = feeder.next_window(finished) {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            jobs.send(w).map_err(|_| AppError::Cancelled)?;
        }
        Ok(())
    };
    while let Some(block) = rx.blocking_recv() {
        match block {
            Block::Run(at) => feeder.seek(at),
            Block::Samples(v) => feeder.push(&v),
        }
        send(&mut feeder, false)?;
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    feeder.finish();
    send(&mut feeder, true)
}

/// The laughter score of every window of a stream of `total` placed samples. Blocking: runs the
/// feeder on this thread and the workers on `threads` more.
fn score_windows(
    mut rx: mpsc::Receiver<Block>,
    total: usize,
    model: &Model,
    threads: usize,
    on_progress: &ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<Vec<f32>> {
    let (tx, jobs) = std_mpsc::sync_channel::<Job>(BATCH * 2);
    let shared = Shared {
        model,
        jobs: Mutex::new(jobs),
        scores: Mutex::new(vec![0.0; window_count(total)]),
        done: AtomicUsize::new(0),
        on_progress,
        cancel,
    };
    let error = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads).map(|_| s.spawn(|| shared.work())).collect();
        let fed = feed(&mut rx, total, &tx, cancel);
        drop(tx);
        if fed.is_err() {
            cancel.cancel();
            shared.drain();
        }
        let mut error = fed.err();
        for w in workers {
            if let Err(e) = w.join().expect("laughter worker panicked") {
                // A real error beats the `Cancelled` it caused elsewhere.
                if !matches!(e, AppError::Cancelled) || error.is_none() {
                    error = Some(e);
                }
            }
        }
        error
    });
    match error {
        Some(e) => Err(e),
        None => Ok(shared.scores.into_inner().expect("scores lock")),
    }
}

/// Window scores → the 1 Hz curve, shifted so each score sits at its window's centre.
pub fn curve(scores: &[f32]) -> Vec<f32> {
    let Some(&first) = scores.first() else { return Vec::new() };
    std::iter::repeat_n(first, SHIFT).chain(scores.iter().copied()).take(scores.len()).collect()
}

/// Decodes the session at 32 kHz and returns its laughter curve. `cancel` stops it.
pub async fn analyze(
    paths: &FfmpegPaths,
    dir: &Path,
    inv: &ChunkInventory,
    segment_s: f64,
    model: Arc<Model>,
    on_progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<Vec<f32>> {
    let total = audio::total_samples(inv, segment_s, SAMPLE_RATE);
    let job = cancel.child_token();
    let (tx, rx) = mpsc::channel::<Block>(16);
    let compute = {
        let job = job.clone();
        tokio::task::spawn_blocking(move || score_windows(rx, total, &model, threads(), &on_progress, &job))
    };
    let decoded = audio::stream_placed(paths, dir, inv, segment_s, SAMPLE_RATE, Some(DECODE_FILTER), &tx, &job).await;
    if decoded.is_err() {
        // Before `tx` goes: the feeder must not take a failed decode for the end of the audio.
        job.cancel();
    }
    drop(tx);
    let scored = compute.await.map_err(|e| AppError::Io(format!("laughter task: {e}")))?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    match (decoded, scored) {
        (Err(e), _) | (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(scores)) => Ok(curve(&scores)),
    }
}

/// Runs where the curve is above `threshold`, merged when closer than `merge_gap_s`, padded by
/// `pad_s`, scored by their peak, best first. A port of `common.regions` with an absolute threshold.
pub fn regions(curve: &[f32], duration_s: f64, threshold: f32, merge_gap_s: f32, pad_s: f32, max_results: usize) -> Vec<Spike> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < curve.len() {
        if curve[i] > threshold {
            let a = i;
            while i < curve.len() && curve[i] > threshold {
                i += 1;
            }
            match runs.last_mut() {
                Some(r) if ((a - r.1) as f64 / CURVE_HZ) < f64::from(merge_gap_s) => r.1 = i,
                _ => runs.push((a, i)),
            }
        } else {
            i += 1;
        }
    }
    let pad = f64::from(pad_s);
    let mut out: Vec<Spike> = runs
        .into_iter()
        .map(|(a, b)| {
            // The first maximum, as numpy's argmax.
            let peak = (a..b).fold(a, |best, j| if curve[j] > curve[best] { j } else { best });
            Spike {
                start_s: (a as f64 / CURVE_HZ - pad).max(0.0),
                end_s: (b as f64 / CURVE_HZ + pad).min(duration_s),
                peak_s: peak as f64 / CURVE_HZ,
                peak_excess_db: 0.0,
                score: curve[peak],
            }
        })
        .collect();
    // Stable, so equal scores stay in time order (as Python's sort).
    out.sort_by(|x, y| y.score.total_cmp(&x.score));
    out.truncate(max_results);
    out
}

// --- cache ----------------------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"SRLA";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 16 + 4;

/// The model a cached curve came from: the first 16 hex digits of its sha256.
fn model_id() -> &'static [u8] {
    &MODEL_SHA256.as_bytes()[..16]
}

pub fn cache_path(dir: &Path, session_id: &str, first: u32, last: u32) -> PathBuf {
    super::pyramid::cache_path(dir, session_id, first, last)
}

fn encode(curve: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + curve.len() * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(model_id());
    out.extend_from_slice(&(curve.len() as u32).to_le_bytes());
    for v in curve {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// `None` for anything but a complete file of this version and model.
fn decode(data: &[u8]) -> Option<Vec<f32>> {
    let header = data.get(..HEADER_LEN)?;
    if &header[..4] != MAGIC || u16::from_le_bytes([header[4], header[5]]) != VERSION || &header[6..22] != model_id() {
        return None;
    }
    let len = u32::from_le_bytes(header[22..26].try_into().ok()?) as usize;
    let body = &data[HEADER_LEN..];
    (body.len() == len * 4).then(|| body.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// The cached curve; a missing, old or broken file gives `None` (run again).
pub fn load(path: &Path) -> Option<Vec<f32>> {
    let data = std::fs::read(path).ok()?;
    let curve = decode(&data);
    if curve.is_none() {
        log::info!("ignoring old or unreadable laughter cache {}", path.display());
    }
    curve
}

pub fn save(path: &Path, curve: &[f32]) -> AppResult<()> {
    super::pyramid::write_cache(path, &encode(curve))
}

// --- state ----------------------------------------------------------------------------------------

/// A session's laughter curve and the chunk range it belongs to.
pub struct Laughs {
    pub curve: Vec<f32>,
    pub duration_s: f64,
    pub first: u32,
    pub last: u32,
}

/// The loaded model, or why it could not be loaded.
type ModelSlot = Option<Result<Arc<Model>, AppError>>;

/// The model (loaded once) and the recent curves. Cheap to clone (shared).
#[derive(Clone)]
pub struct LaughState {
    model: Arc<Mutex<ModelSlot>>,
    loaded: Arc<Mutex<LruCache<String, Arc<Laughs>>>>,
}

impl Default for LaughState {
    fn default() -> Self {
        Self {
            model: Arc::new(Mutex::new(None)),
            loaded: Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(LOADED_CAPACITY).expect("non-zero")))),
        }
    }
}

impl LaughState {
    /// Loads the model on first use. A failure is remembered until restart. Blocking.
    pub fn model(&self, path: &Path) -> AppResult<Arc<Model>> {
        let mut m = self.model.lock().expect("model lock");
        m.get_or_insert_with(|| {
            let t = std::time::Instant::now();
            let r = Model::load(path).map(Arc::new);
            match &r {
                Ok(_) => log::info!("laughter model loaded in {:?}", t.elapsed()),
                Err(e) => log::warn!("{e}"),
            }
            r
        })
        .clone()
    }

    /// The loaded curve, if it still matches the chunk range on disk.
    pub fn get(&self, id: &str, range: Option<(u32, u32)>) -> Option<Arc<Laughs>> {
        let mut loaded = self.loaded.lock().expect("loaded lock");
        let l = loaded.get(id)?.clone();
        match range {
            Some(r) if r != (l.first, l.last) => {
                loaded.pop(id);
                None
            }
            _ => Some(l),
        }
    }

    pub fn put(&self, id: &str, l: Arc<Laughs>) {
        self.loaded.lock().expect("loaded lock").put(id.to_owned(), l);
    }

    pub fn clear_loaded(&self) {
        self.loaded.lock().expect("loaded lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_count_as_python() {
        // range(0, max(1, n − win + hop), hop)
        let py = |n: usize| (0..(n as i64 - WINDOW as i64 + HOP as i64).max(1)).step_by(HOP).count();
        for n in [0, 1, HOP, WINDOW - HOP, WINDOW - 1, WINDOW, WINDOW + 1, WINDOW + HOP, 3720 * HOP, 3720 * HOP + 17] {
            assert_eq!(window_count(n), py(n), "n = {n}");
        }
        assert_eq!(window_count(3720 * HOP), 3716);
    }

    /// Feeds `blocks` and returns all windows.
    fn windows(total: usize, blocks: &[(i64, Vec<f32>)]) -> Vec<(usize, Vec<f32>)> {
        let mut f = WindowFeeder::new(total);
        let mut out = Vec::new();
        for (at, v) in blocks {
            f.seek(*at);
            for chunk in v.chunks(7919) {
                f.push(chunk);
                while let Some(w) = f.next_window(false) {
                    out.push(w);
                }
            }
        }
        f.finish();
        while let Some(w) = f.next_window(true) {
            out.push(w);
        }
        out
    }

    #[test]
    fn feeder_matches_placed_buffer() {
        // Runs with gaps, one starting before 0.
        let total = 23 * HOP + 1234;
        let ramp = |n: usize, base: f32| (0..n).map(|i| base + i as f32).collect::<Vec<_>>();
        let blocks = vec![
            (-500, ramp(6 * HOP, 1.0)),
            (8 * HOP as i64, ramp(5 * HOP, 1e6)),
            (13 * HOP as i64 + 10, ramp(20 * HOP, 2e6)),
        ];
        // The placed buffer, as decode_placed builds it.
        let mut placed = vec![0f32; total];
        for (at, v) in &blocks {
            for (i, x) in v.iter().enumerate() {
                let j = at + i as i64;
                if (0..total as i64).contains(&j) {
                    placed[j as usize] = *x;
                }
            }
        }
        let got = windows(total, &blocks);
        assert_eq!(got.len(), window_count(total));
        for (k, w) in &got {
            let s = k * HOP;
            let mut want: Vec<f32> = placed[s.min(total)..(s + WINDOW).min(total)].to_vec();
            want.resize(WINDOW, 0.0);
            assert_eq!(w, &want, "window {k}");
        }
        // Overlapping runs (rounding of the probed starts): later samples win where still buffered.
        let over = windows(total, &[(0, vec![1.0; 10 * HOP]), (10 * HOP as i64 - 5, vec![2.0; 5 * HOP])]);
        assert_eq!(over.len(), window_count(total));
        assert!(over.iter().all(|(_, w)| w.len() == WINDOW));
        assert_eq!(over[10].1[0], 2.0);
        // Short input still gives one (padded) window.
        assert_eq!(windows(HOP, &[(0, vec![1.0; HOP])]).len(), 1);
    }

    #[test]
    fn curve_shifted_to_window_centres() {
        assert_eq!(curve(&[1.0, 2.0, 3.0, 4.0]), vec![1.0, 1.0, 1.0, 2.0]);
        assert!(curve(&[]).is_empty());
    }

    #[test]
    fn regions_as_python() {
        let mut c = vec![0.0f32; 100];
        c[10] = 0.5;
        c[11] = 0.7;
        c[14] = 0.6; // 3 s after the first run: merged
        c[40] = 0.9; // alone
        c[98] = 0.3; // padded end clamped
        let r = regions(&c, 100.0, 0.2, 5.0, 1.5, 200);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].start_s, r[0].end_s, r[0].peak_s, r[0].score), (38.5, 42.5, 40.0, 0.9));
        assert_eq!((r[1].start_s, r[1].end_s, r[1].peak_s), (8.5, 16.5, 11.0));
        assert_eq!((r[2].start_s, r[2].end_s), (96.5, 100.0));
        assert_eq!(regions(&c, 100.0, 0.2, 5.0, 1.5, 1).len(), 1);
        // A higher threshold leaves fewer; no gap merging splits the first one.
        assert_eq!(regions(&c, 100.0, 0.55, 5.0, 1.5, 200).len(), 2);
        assert_eq!(regions(&c, 100.0, 0.2, 0.0, 1.5, 200).len(), 4);
        assert!(regions(&[], 0.0, 0.2, 5.0, 1.5, 200).is_empty());
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = cache_path(dir.path(), "a/bg/x", 1, 9);
        save(&p, &[0.25, 0.5]).unwrap();
        assert_eq!(load(&p), Some(vec![0.25, 0.5]));
        let mut other = encode(&[0.25]);
        other[6] ^= 1; // another model
        assert_eq!(decode(&other), None);
        assert_eq!(decode(&encode(&[0.25])[..HEADER_LEN + 2]), None);
    }

    // --- M15 parity and quality checks (manual, see docs/specs/15-laughter.md §6) ---------------

    fn experiments() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../experiments/moments")
    }

    /// numpy's default (linear) quantile.
    fn quantile(v: &[f32], q: f64) -> f32 {
        let mut s = v.to_vec();
        s.sort_by(f32::total_cmp);
        let pos = q * (s.len() - 1) as f64;
        let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
        (f64::from(s[lo]) + (f64::from(s[hi]) - f64::from(s[lo])) * (pos - lo as f64)) as f32
    }

    /// The method files `eval.py` reads (`out/<stem>/<method>.json`), plus the raw curve.
    fn write_method(stem: &str, method: &str, curve: &[f32], regs: &[Spike], runtime_s: f64) {
        let out = experiments().join("out").join(stem);
        let json = serde_json::json!({
            "method": method,
            "runtimeS": runtime_s,
            "audioS": curve.len() as f64 / CURVE_HZ,
            "regions": regs,
        });
        std::fs::write(out.join(format!("{method}.json")), serde_json::to_vec_pretty(&json).unwrap()).unwrap();
        let raw: Vec<u8> = curve.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(out.join(format!("{method}.f32")), raw).unwrap();
    }

    /// Session parity: the exact Python input (`audio32.f32` from export_onnx.py) through the Rust
    /// pipeline, regions with the same quantile threshold. Compare with `compare_rust.py`.
    /// `cargo test --release --lib laughter_parity -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn laughter_parity() {
        let Some(path) = model::tests::model_path() else { return };
        let model = Model::load(&path).unwrap();
        let mut stems: Vec<_> = std::fs::read_dir(experiments().join("out"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("audio32.f32").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        stems.sort();
        for stem in stems {
            let file = experiments().join("out").join(&stem).join("audio32.f32");
            let total = (std::fs::metadata(&file).unwrap().len() / 4) as usize;
            let (tx, rx) = mpsc::channel(16);
            let reader = std::thread::spawn(move || {
                use std::io::Read;
                let mut f = std::io::BufReader::new(std::fs::File::open(file).unwrap());
                tx.blocking_send(Block::Run(0)).unwrap();
                let mut buf = vec![0u8; 1 << 20];
                loop {
                    let n = f.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    assert_eq!(n % 4, 0);
                    let v = buf[..n].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
                    tx.blocking_send(Block::Samples(v)).unwrap();
                }
            });
            let t = std::time::Instant::now();
            let progress: ProgressFn = Arc::new(|_| {});
            let scores = score_windows(rx, total, &model, threads(), &progress, &CancellationToken::new()).unwrap();
            let runtime = t.elapsed().as_secs_f64();
            reader.join().unwrap();
            let c = curve(&scores);
            let thr = quantile(&c, 0.97);
            let regs = regions(&c, c.len() as f64, thr, 5.0, 1.5, 200);
            write_method(&stem, "effat_rust_ref", &c, &regs, runtime);
            eprintln!(
                "{stem}: {} windows in {runtime:.1} s ({:.0} s per audio hour, {} threads), q97 {thr:.4}, {} regions",
                c.len(),
                runtime * 3600.0 / (total as f64 / f64::from(SAMPLE_RATE)),
                threads(),
                regs.len()
            );
        }
    }

    /// The real app path on a recording: chunks → 32 kHz stream → curve → regions with the
    /// default threshold. Writes `out/<stem>/effat_rust.json` for `eval.py`.
    /// `STEAMREPLAY_SESSION_DIR=<bg folder> cargo test --release --lib laughter_session -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn laughter_session() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let Some(path) = model::tests::model_path() else { return };
        let dir = PathBuf::from(dir);
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        let id = format!("{}/bg/{}", name(dir.ancestors().nth(3).unwrap()), name(&dir));
        let stem = crate::labels::file_stem(&id);
        let (inv, _) = crate::steam::recordings::inventory(&dir).unwrap();
        let paths = crate::media::ffmpeg::locate(None).unwrap();
        let model = Arc::new(Model::load(&path).unwrap());
        let t = std::time::Instant::now();
        let c = analyze(&paths, &dir, &inv, 3.0, model, Arc::new(|_| {}), &CancellationToken::new()).await.unwrap();
        let runtime = t.elapsed().as_secs_f64();
        let duration = audio::total_samples(&inv, 3.0, SAMPLE_RATE) as f64 / f64::from(SAMPLE_RATE);
        let p = crate::media::spikes::SpikeParams::default();
        let regs = regions(&c, duration, p.laugh_threshold, p.merge_gap_s, p.pad_s, p.max_results as usize);
        std::fs::create_dir_all(experiments().join("out").join(&stem)).unwrap();
        write_method(&stem, "effat_rust", &c, &regs, runtime);
        eprintln!(
            "{stem}: {duration:.0} s in {runtime:.1} s ({:.0} s per audio hour, {} threads), q97 {:.4}, {} regions",
            runtime * 3600.0 / duration,
            threads(),
            quantile(&c, 0.97),
            regs.len()
        );
        for (i, r) in regs.iter().take(10).enumerate() {
            eprintln!("#{:<2} {:>7.1}–{:>7.1} s  peak {:>7.1}  {:.3}", i + 1, r.start_s, r.end_s, r.peak_s, r.score);
        }
    }
}
