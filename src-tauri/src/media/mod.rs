//! ffmpeg-based media work: audio analysis (M2), later export and preview.

pub mod audio;
pub mod chunks;
pub mod encoders;
pub mod export;
pub mod preview;
pub mod ffmpeg;
pub mod pyramid;
pub mod spikes;
pub mod thumbs;

use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lru::LruCache;
use serde::Serialize;
use specta::Type;
use tokio_util::sync::CancellationToken;

use audio::{BIN_MS, Bin, Gap};
use ffmpeg::{FfmpegPaths, ProgressFn};
use pyramid::{FACTOR, Pyramid};
use spikes::Curves;

use crate::error::{AppError, AppResult};
use crate::steam::recordings::SessionDetail;

/// Analyses kept in memory.
const LOADED_CAPACITY: usize = 4;
/// Used when the session has no readable manifest. Steam always writes 3 s segments.
const DEFAULT_SEGMENT_S: f64 = 3.0;

/// A loaded waveform: the pyramid plus what is needed to describe it.
pub struct Analysis {
    pub pyramid: Pyramid,
    pub gaps: Vec<Gap>,
    pub first: u32,
    pub last: u32,
    /// The spike curves of the last `(short_ms, baseline_ms)`, so threshold changes skip them.
    curves: Mutex<Option<CachedCurves>>,
}

/// `((short_ms, baseline_ms), curves)`.
type CachedCurves = ((u32, u32), Arc<Curves>);

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LevelInfo {
    pub bins: u32,
    pub bin_ms: u32,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct WaveformMeta {
    pub bin_ms: u16,
    pub levels: Vec<LevelInfo>,
    #[specta(type = specta_typescript::Number)]
    pub duration_s: f64,
    /// Missing chunks, drawn hatched.
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// 0..1
    #[specta(type = specta_typescript::Number)]
    pub fraction: f64,
}

impl Analysis {
    pub fn new(pyramid: Pyramid, gaps: Vec<Gap>, first: u32, last: u32) -> Self {
        Self { pyramid, gaps, first, last, curves: Mutex::new(None) }
    }

    /// The spike curves for these windows, computed once per key. Blocking.
    pub fn curves_for(&self, short_ms: u32, baseline_s: f32) -> Arc<Curves> {
        let key = (short_ms, (baseline_s * 1000.0).round() as u32);
        let mut cached = self.curves.lock().expect("curves lock");
        if let Some((k, c)) = cached.as_ref()
            && *k == key
        {
            return c.clone();
        }
        let c = Arc::new(spikes::curves(self.pyramid.level0(), short_ms, baseline_s));
        *cached = Some((key, c.clone()));
        c
    }

    /// The cached curves, if any.
    pub fn cached_curves(&self) -> Option<Arc<Curves>> {
        self.curves.lock().expect("curves lock").as_ref().map(|(_, c)| c.clone())
    }

    pub fn meta(&self) -> WaveformMeta {
        let mut bin_ms = u32::from(BIN_MS);
        let levels = self
            .pyramid
            .levels
            .iter()
            .map(|l| {
                let info = LevelInfo { bins: l.len() as u32, bin_ms };
                bin_ms *= FACTOR as u32;
                info
            })
            .collect();
        WaveformMeta {
            bin_ms: BIN_MS,
            levels,
            duration_s: self.pyramid.level0().len() as f64 * f64::from(BIN_MS) / 1000.0,
            gaps: self.gaps.clone(),
        }
    }
}

/// Shared state of the media commands.
pub struct MediaState {
    ffmpeg: Mutex<Option<FfmpegPaths>>,
    /// Encoder lists of the ffmpeg at that path.
    caps: Mutex<Option<(PathBuf, Arc<encoders::Capabilities>)>>,
    loaded: Mutex<LruCache<String, Arc<Analysis>>>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            ffmpeg: Mutex::new(None),
            caps: Mutex::new(None),
            loaded: Mutex::new(LruCache::new(NonZeroUsize::new(LOADED_CAPACITY).expect("non-zero"))),
            jobs: Mutex::new(HashMap::new()),
        }
    }
}

/// Removes the job from the registry when the job ends, however it ends.
pub struct JobGuard<'a> {
    state: &'a MediaState,
    id: String,
    pub token: CancellationToken,
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        self.state.jobs.lock().expect("jobs lock").remove(&self.id);
    }
}

impl MediaState {
    pub fn cached_ffmpeg(&self) -> Option<FfmpegPaths> {
        self.ffmpeg.lock().expect("ffmpeg lock").clone()
    }

    pub fn set_ffmpeg(&self, paths: Option<FfmpegPaths>) {
        *self.ffmpeg.lock().expect("ffmpeg lock") = paths;
        *self.caps.lock().expect("caps lock") = None;
    }

    /// The capabilities probed for `ffmpeg`, if they are cached.
    pub fn cached_caps(&self, ffmpeg: &Path) -> Option<Arc<encoders::Capabilities>> {
        self.caps.lock().expect("caps lock").as_ref().filter(|(p, _)| p == ffmpeg).map(|(_, c)| c.clone())
    }

    pub fn set_caps(&self, ffmpeg: &Path, caps: Arc<encoders::Capabilities>) {
        *self.caps.lock().expect("caps lock") = Some((ffmpeg.to_owned(), caps));
    }

    /// The loaded analysis, if it still matches the chunk range on disk.
    pub fn get(&self, id: &str, range: Option<(u32, u32)>) -> Option<Arc<Analysis>> {
        let mut loaded = self.loaded.lock().expect("loaded lock");
        let a = loaded.get(id)?.clone();
        match range {
            Some(r) if r != (a.first, a.last) => {
                loaded.pop(id);
                None
            }
            _ => Some(a),
        }
    }

    pub fn put(&self, id: &str, a: Arc<Analysis>) {
        self.loaded.lock().expect("loaded lock").put(id.to_owned(), a);
    }

    /// Forgets every loaded analysis (after the disk cache was cleared).
    pub fn clear_loaded(&self) {
        self.loaded.lock().expect("loaded lock").clear();
    }

    /// Ids of the jobs that are running now.
    pub fn running_jobs(&self) -> HashSet<String> {
        self.jobs.lock().expect("jobs lock").keys().cloned().collect()
    }

    pub fn start_job(&self, id: &str) -> JobGuard<'_> {
        let token = CancellationToken::new();
        let previous = self.jobs.lock().expect("jobs lock").insert(id.to_owned(), token.clone());
        if let Some(p) = previous {
            p.cancel();
        }
        JobGuard { state: self, id: id.to_owned(), token }
    }

    /// Returns `false` if no such job is running (it may have just finished).
    pub fn cancel_job(&self, id: &str) -> bool {
        match self.jobs.lock().expect("jobs lock").get(id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }
}

fn segment_s(detail: &SessionDetail) -> f64 {
    detail.mpd.as_ref().map_or(DEFAULT_SEGMENT_S, |m| m.segment_duration_s)
}

pub fn cache_file(cache_dir: &Path, detail: &SessionDetail) -> PathBuf {
    pyramid::cache_path(cache_dir, &detail.summary.id, detail.chunks.first, detail.chunks.last)
}

fn finish(detail: &SessionDetail, level0: Vec<Bin>, gaps: Vec<Gap>) -> Analysis {
    Analysis::new(Pyramid::build(level0), gaps, detail.chunks.first, detail.chunks.last)
}

/// Loads the analysis from the disk cache. Blocking.
pub fn load_cached(cache_dir: &Path, detail: &SessionDetail) -> Option<Analysis> {
    if detail.chunks.count == 0 {
        return None;
    }
    let level0 = pyramid::load(&cache_file(cache_dir, detail))?;
    let seg = segment_s(detail);
    if level0.len() != audio::total_bins(&detail.chunks, seg) {
        return None;
    }
    // Gaps are not stored in the file; the inventory gives them again.
    let gaps = audio::gaps(&audio::plan_runs(&detail.summary.dir, &detail.chunks), &detail.chunks, seg);
    Some(finish(detail, level0, gaps))
}

/// Decodes the session's audio, builds the pyramid and writes the cache.
/// The cache file is only written after a complete decode.
pub async fn analyze(
    paths: &FfmpegPaths,
    cache_dir: &Path,
    detail: &SessionDetail,
    on_progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<Analysis> {
    let (level0, gaps) =
        audio::extract(paths, &detail.summary.dir, &detail.chunks, segment_s(detail), on_progress, cancel).await?;
    let path = cache_file(cache_dir, detail);
    let detail = detail.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = pyramid::save(&path, &level0) {
            log::warn!("could not write waveform cache: {e}");
        }
        finish(&detail, level0, gaps)
    })
    .await
    .map_err(|e| AppError::Io(format!("background task failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam::recordings::inventory;
    use std::time::Instant;

    /// Manual check on a real recording: `STEAMREPLAY_SESSION_DIR=<bg folder> cargo test --release
    /// real_session -- --ignored --nocapture`. Prints timings and the top spikes for listening.
    #[test]
    fn curves_cached_per_key() {
        let level0: Vec<Bin> = (0..5000).map(|i| [-0.1, 0.1, 0.01 + (i % 100) as f32 * 1e-4]).collect();
        let a = Analysis::new(Pyramid::build(level0), Vec::new(), 1, 10);
        assert!(a.cached_curves().is_none());
        let c1 = a.curves_for(250, 30.0);
        assert!(Arc::ptr_eq(&c1, &a.curves_for(250, 30.0)));
        let c2 = a.curves_for(250, 60.0);
        assert!(!Arc::ptr_eq(&c1, &c2));
        assert!(Arc::ptr_eq(&c2, &a.cached_curves().unwrap()));
        assert_eq!(*c1, spikes::curves(a.pyramid.level0(), 250, 30.0));
    }

    #[tokio::test]
    #[ignore]
    async fn real_session() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let dir = PathBuf::from(dir);
        let (inv, _) = inventory(&dir).unwrap();
        let paths = ffmpeg::locate(None).unwrap();
        let t = Instant::now();
        let (level0, gaps) =
            audio::extract(&paths, &dir, &inv, 3.0, Arc::new(|_| {}), &CancellationToken::new()).await.unwrap();
        let t_extract = t.elapsed();
        let t = Instant::now();
        let p = Pyramid::build(level0);
        let t_pyramid = t.elapsed();
        let t = Instant::now();
        let found = spikes::detect(p.level0(), &spikes::SpikeParams::default());
        let t_spikes = t.elapsed();
        // M6: the two stages separately, for live tuning.
        let def = spikes::SpikeParams::default();
        for baseline_s in [5.0, 30.0, 120.0] {
            let t = Instant::now();
            let c = spikes::curves(p.level0(), def.short_ms, baseline_s);
            let t_curves = t.elapsed();
            let t = Instant::now();
            let n = spikes::regions(&c, &def).len();
            eprintln!("baseline {baseline_s} s: curves {t_curves:?}, regions {:?} ({n} spikes)", t.elapsed());
        }
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("x.bin");
        pyramid::save(&path, p.level0()).unwrap();
        let t = Instant::now();
        let reloaded = Pyramid::build(pyramid::load(&path).unwrap());
        let t_reload = t.elapsed();
        assert_eq!(reloaded, p);
        let lead = p.level0().iter().take_while(|b| b[2] == 0.0).count();
        eprintln!(
            "{} bins, {} gaps, {lead} silent bins at the start; extract {t_extract:?}, pyramid {t_pyramid:?}, \
             spikes {t_spikes:?}, cache reload {t_reload:?}; {} spikes",
            p.level0().len(),
            gaps.len(),
            found.len()
        );
        for (i, s) in found.iter().take(15).enumerate() {
            eprintln!(
                "#{:<2} {:>8.1}–{:>8.1} s  peak {:>8.1} s  +{:.1} dB  score {:.1}",
                i + 1,
                s.start_s,
                s.end_s,
                s.peak_s,
                s.peak_excess_db,
                s.score
            );
        }
    }
}
