//! Low-res H.264 preview of a session (M4). Segments of 4 chunks (12 s) are transcoded on
//! demand, cached on disk (`cache.rs`) and served to the player through the `replay://`
//! protocol (`protocol.rs`). Each open session has a small job queue ordered by distance to the
//! playhead.

pub mod cache;
pub mod protocol;
pub mod transcode;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

pub use cache::PreviewCache;
pub use transcode::Backend;

use super::ffmpeg::FfmpegPaths;
use crate::error::{AppError, AppResult};
use crate::steam::recordings::ChunkInventory;

pub const SEGMENT_CHUNKS: u32 = 4;
/// MSE type of every segment: `-profile:v high -level 4.0` and Steam's AAC-LC.
pub const MIME: &str = r#"video/mp4; codecs="avc1.640028, mp4a.40.2""#;
/// Steam records at about 60 fps and the preview keeps the frame rate.
pub const PREVIEW_FPS: f64 = 60.0;
/// Queued segments further than this from the playhead are dropped, running ones cancelled.
const STALE_SEGMENTS: u32 = 3;
/// After this many hardware failures in a row the session switches to software.
const MAX_HW_FAILURES: u32 = 2;

/// The chunks of one preview segment.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentPlan {
    pub index: u32,
    /// Chunk numbers that exist on disk (gaps are left out).
    pub chunks: Vec<u32>,
    /// Set for the last segment when it has fewer than 4 chunks: the last chunk number.
    pub partial_last: Option<u32>,
}

impl SegmentPlan {
    /// A partial segment carries its last chunk in the name, so it is redone when the live
    /// session grows.
    pub fn file_name(&self) -> String {
        match self.partial_last {
            Some(last) => format!("{}_{last}.mp4", self.index),
            None => format!("{}.mp4", self.index),
        }
    }
}

pub fn segment_count(inv: &ChunkInventory) -> u32 {
    if inv.count == 0 { 0 } else { (inv.last - inv.first + 1).div_ceil(SEGMENT_CHUNKS) }
}

/// Segment `s` covers chunks `first + 4s ..= first + 4s + 3` (fewer at the end).
pub fn segment_plan(inv: &ChunkInventory, s: u32) -> Option<SegmentPlan> {
    if s >= segment_count(inv) {
        return None;
    }
    let start = inv.first + s * SEGMENT_CHUNKS;
    let nominal_end = start + SEGMENT_CHUNKS - 1;
    let end = nominal_end.min(inv.last);
    Some(SegmentPlan {
        index: s,
        chunks: (start..=end).filter(|n| inv.missing.binary_search(n).is_err()).collect(),
        partial_last: (nominal_end > inv.last).then_some(inv.last),
    })
}

/// Segments to prepare around the playhead, most urgent first.
pub fn prefetch_order(playhead: u32, count: u32, hardware: bool) -> Vec<u32> {
    let wanted: &[i64] = if hardware { &[0, 1, 2, -1] } else { &[0, 1] };
    wanted
        .iter()
        .filter_map(|d| u32::try_from(i64::from(playhead) + d).ok())
        .filter(|s| *s < count)
        .collect()
}

pub fn is_stale(segment: u32, playhead: u32) -> bool {
    segment.abs_diff(playhead) > STALE_SEGMENTS
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    #[specta(type = specta_typescript::Number)]
    pub segment_s: f64,
    pub count: u32,
    /// For `MediaSource.addSourceBuffer`.
    pub mime: String,
    #[specta(type = specta_typescript::Number)]
    pub fps: f64,
    pub backend: Backend,
}

type Waiter = oneshot::Sender<AppResult<PathBuf>>;

#[derive(Default)]
struct Queue {
    /// Segments to transcode, most urgent first.
    pending: Vec<u32>,
    running: HashMap<u32, CancellationToken>,
    waiters: HashMap<u32, Vec<Waiter>>,
    playhead: u32,
}

/// Everything needed to transcode the segments of one open session.
pub struct PreviewSession {
    pub id: String,
    dir: PathBuf,
    pub inv: ChunkInventory,
    pub segment_s: f64,
    /// Start time of the first chunk on disk; subtracted to get video time.
    v0: f64,
    paths: FfmpegPaths,
    cache: Arc<PreviewCache>,
    cache_dir: PathBuf,
    backend: Mutex<Backend>,
    hw_failures: AtomicU32,
    cancel: CancellationToken,
    queue: Mutex<Queue>,
    notify: Notify,
}

pub struct SessionSetup {
    pub id: String,
    pub dir: PathBuf,
    pub inv: ChunkInventory,
    pub chunk_s: f64,
    pub v0: f64,
    pub paths: FfmpegPaths,
    pub backend: Backend,
}

impl PreviewSession {
    fn start(setup: SessionSetup, cache: Arc<PreviewCache>, parent: &CancellationToken) -> Arc<Self> {
        let cache_dir = cache.session_dir(&setup.id, setup.inv.first);
        let session = Arc::new(Self {
            id: setup.id,
            dir: setup.dir,
            segment_s: setup.chunk_s * f64::from(SEGMENT_CHUNKS),
            inv: setup.inv,
            v0: setup.v0,
            paths: setup.paths,
            cache,
            cache_dir,
            backend: Mutex::new(setup.backend),
            hw_failures: AtomicU32::new(0),
            cancel: parent.child_token(),
            queue: Mutex::new(Queue::default()),
            notify: Notify::new(),
        });
        // One transcode at a time: NVDEC is the bottleneck, and two concurrent jobs took 2.5 s
        // each where one alone takes 1.3 s. So the segment under the playhead is ready soonest
        // when it runs alone; prefetch follows right after.
        tauri::async_runtime::spawn(session.clone().worker());
        session
    }

    pub fn count(&self) -> u32 {
        segment_count(&self.inv)
    }

    pub fn backend(&self) -> Backend {
        *self.backend.lock().expect("backend lock")
    }

    pub fn set_backend(&self, b: Backend) {
        *self.backend.lock().expect("backend lock") = b;
        self.hw_failures.store(0, Ordering::Relaxed);
    }

    pub fn info(&self) -> PreviewInfo {
        PreviewInfo {
            segment_s: self.segment_s,
            count: self.count(),
            mime: MIME.to_owned(),
            fps: PREVIEW_FPS,
            backend: self.backend(),
        }
    }

    fn path_of(&self, s: u32) -> Option<PathBuf> {
        segment_plan(&self.inv, s).map(|p| self.cache_dir.join(p.file_name()))
    }

    fn is_cached(&self, s: u32) -> bool {
        self.path_of(s).is_some_and(|p| p.is_file())
    }

    fn wake(&self) {
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    /// The segment's file, transcoding it first (ahead of everything else) when needed.
    pub async fn segment(&self, s: u32) -> AppResult<PathBuf> {
        let path = self.path_of(s).ok_or_else(|| AppError::NotFound(format!("no preview segment {s}")))?;
        if path.is_file() {
            PreviewCache::touch(&path);
            return Ok(path);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut q = self.queue.lock().expect("queue lock");
            q.waiters.entry(s).or_default().push(tx);
            if !q.running.contains_key(&s) {
                q.pending.retain(|p| *p != s);
                q.pending.insert(0, s);
            }
        }
        self.wake();
        tokio::select! {
            r = rx => r.unwrap_or(Err(AppError::Cancelled)),
            () = self.cancel.cancelled() => Err(AppError::Cancelled),
        }
    }

    /// Moves the queue to a new playhead: drops and cancels far-away work, queues prefetch.
    pub fn set_playhead(&self, t: f64) {
        let count = self.count();
        if count == 0 {
            return;
        }
        let playhead = ((t.max(0.0) / self.segment_s) as u32).min(count - 1);
        let hardware = self.backend().is_hardware();
        // File checks outside the lock.
        let prefetch: Vec<u32> =
            prefetch_order(playhead, count, hardware).into_iter().filter(|s| !self.is_cached(*s)).collect();
        let mut q = self.queue.lock().expect("queue lock");
        q.playhead = playhead;
        let stale: Vec<u32> = q.pending.iter().copied().filter(|s| is_stale(*s, playhead)).collect();
        q.pending.retain(|s| !is_stale(*s, playhead));
        for s in stale {
            for w in q.waiters.remove(&s).unwrap_or_default() {
                let _ = w.send(Err(AppError::Cancelled));
            }
        }
        for (s, token) in &q.running {
            if is_stale(*s, playhead) {
                token.cancel();
            }
        }
        // Explicit requests stay first, then the prefetch order, then older prefetch.
        let (mut requested, older): (Vec<u32>, Vec<u32>) =
            q.pending.iter().partition(|s| q.waiters.contains_key(s));
        for s in prefetch.into_iter().chain(older) {
            if !requested.contains(&s) && !q.running.contains_key(&s) {
                requested.push(s);
            }
        }
        q.pending = requested;
        drop(q);
        self.wake();
    }

    pub fn close(&self) {
        self.cancel.cancel();
    }

    fn running_jobs(&self) -> usize {
        self.queue.lock().expect("queue lock").running.len()
    }

    async fn worker(self: Arc<Self>) {
        loop {
            // Register for a wake-up before looking at the queue, so none is missed.
            let notified = self.notify.notified();
            if self.queue.lock().expect("queue lock").pending.is_empty() {
                tokio::select! {
                    () = notified => {}
                    () = self.cancel.cancelled() => return,
                }
                continue;
            }
            let job = {
                let mut q = self.queue.lock().expect("queue lock");
                if q.pending.is_empty() {
                    None
                } else {
                    let s = q.pending.remove(0);
                    let token = self.cancel.child_token();
                    q.running.insert(s, token.clone());
                    Some((s, token))
                }
            };
            let Some((s, token)) = job else { continue };
            let result = self.run(s, &token).await;
            let waiters = {
                let mut q = self.queue.lock().expect("queue lock");
                q.running.remove(&s);
                q.waiters.remove(&s).unwrap_or_default()
            };
            for w in waiters {
                let _ = w.send(result.clone());
            }
            if self.cancel.is_cancelled() {
                return;
            }
        }
    }

    async fn run(&self, s: u32, cancel: &CancellationToken) -> AppResult<PathBuf> {
        let plan = segment_plan(&self.inv, s).ok_or_else(|| AppError::NotFound(format!("no preview segment {s}")))?;
        let out = self.cache_dir.join(plan.file_name());
        if out.is_file() {
            return Ok(out);
        }
        let backend = self.backend();
        let started = Instant::now();
        let tmp = self.cache.tmp_dir();
        let src = transcode::Source { paths: &self.paths, dir: &self.dir, v0: self.v0, tmp_dir: &tmp };
        let mut result = transcode::transcode(&src, backend, &plan, &out, cancel).await;
        if backend.is_hardware() {
            match &result {
                Err(AppError::Cancelled) | Err(AppError::NotFound(_)) => {}
                Err(e) => {
                    log::warn!("preview segment {s} with {backend:?} failed, using software: {e}");
                    if self.hw_failures.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_HW_FAILURES {
                        log::warn!("switching the preview of {} to software", self.id);
                        self.set_backend(Backend::Software);
                    }
                    result = transcode::transcode(&src, Backend::Software, &plan, &out, cancel).await;
                }
                Ok(()) => self.hw_failures.store(0, Ordering::Relaxed),
            }
        }
        result?;
        log::info!("preview {} segment {s}: {:?}", self.id, started.elapsed());
        let cache = self.cache.clone();
        tauri::async_runtime::spawn_blocking(move || cache.evict());
        Ok(out)
    }
}

/// Open preview sessions plus the shared cache.
pub struct PreviewState {
    pub cache: Arc<PreviewCache>,
    sessions: Mutex<HashMap<String, Arc<PreviewSession>>>,
    /// Detected backend, kept for the app's lifetime (also stored in the settings).
    pub detected: Mutex<Option<Backend>>,
    shutdown: CancellationToken,
}

impl PreviewState {
    pub fn new(cache: PreviewCache) -> Self {
        Self {
            cache: Arc::new(cache),
            sessions: Mutex::new(HashMap::new()),
            detected: Mutex::new(None),
            shutdown: CancellationToken::new(),
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<PreviewSession>> {
        self.sessions.lock().expect("sessions lock").get(id).cloned()
    }

    /// Opens (or replaces) the session. An open session with the same chunk range is kept.
    pub fn open(&self, setup: SessionSetup) -> Arc<PreviewSession> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        if let Some(s) = sessions.get(&setup.id)
            && s.inv == setup.inv
            && !s.cancel.is_cancelled()
        {
            return s.clone();
        }
        let id = setup.id.clone();
        let session = PreviewSession::start(setup, self.cache.clone(), &self.shutdown);
        if let Some(old) = sessions.insert(id, session.clone()) {
            old.close();
        }
        session
    }

    pub fn close(&self, id: &str) {
        if let Some(s) = self.sessions.lock().expect("sessions lock").remove(id) {
            s.close();
        }
    }

    pub fn set_backend(&self, b: Backend) {
        for s in self.sessions.lock().expect("sessions lock").values() {
            s.set_backend(b);
        }
    }

    /// Cancels all jobs and waits (briefly) until their ffmpeg processes are killed.
    /// Called from the app's exit handler, so it blocks.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
        let sessions: Vec<_> = self.sessions.lock().expect("sessions lock").drain().map(|(_, s)| s).collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while sessions.iter().any(|s| s.running_jobs() > 0) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(first: u32, last: u32, missing: Vec<u32>) -> ChunkInventory {
        ChunkInventory { first, last, count: last - first + 1 - missing.len() as u32, missing }
    }

    #[test]
    fn segments_from_first_chunk() {
        // Pruned session: chunks 3418..=3427 (10 chunks) → 3 segments, the last with 2 chunks.
        let i = inv(3418, 3427, vec![]);
        assert_eq!(segment_count(&i), 3);
        assert_eq!(
            segment_plan(&i, 0),
            Some(SegmentPlan { index: 0, chunks: vec![3418, 3419, 3420, 3421], partial_last: None })
        );
        let last = segment_plan(&i, 2).unwrap();
        assert_eq!(last.chunks, vec![3426, 3427]);
        assert_eq!(last.partial_last, Some(3427));
        assert_eq!(last.file_name(), "2_3427.mp4");
        assert_eq!(segment_plan(&i, 1).unwrap().file_name(), "1.mp4");
        assert_eq!(segment_plan(&i, 3), None);

        // Exactly full: no partial segment.
        let full = inv(1, 8, vec![]);
        assert_eq!(segment_count(&full), 2);
        assert_eq!(segment_plan(&full, 1).unwrap().partial_last, None);

        assert_eq!(segment_count(&ChunkInventory::default()), 0);
        assert_eq!(segment_plan(&ChunkInventory::default(), 0), None);
    }

    #[test]
    fn gaps_are_left_out() {
        let i = inv(1, 12, vec![6, 7]);
        assert_eq!(segment_plan(&i, 1).unwrap().chunks, vec![5, 8]);
        let all_missing = inv(1, 9, vec![5, 6, 7, 8]);
        assert!(segment_plan(&all_missing, 1).unwrap().chunks.is_empty());
    }

    #[test]
    fn prefetch_and_stale() {
        assert_eq!(prefetch_order(5, 100, true), vec![5, 6, 7, 4]);
        assert_eq!(prefetch_order(5, 100, false), vec![5, 6]);
        assert_eq!(prefetch_order(0, 100, true), vec![0, 1, 2]);
        assert_eq!(prefetch_order(9, 10, true), vec![9, 8]);
        assert!(!is_stale(8, 5));
        assert!(!is_stale(2, 5));
        assert!(is_stale(9, 5));
        assert!(is_stale(1, 5));
    }
}
