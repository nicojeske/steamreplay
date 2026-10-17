//! Disk cache of preview segments: `<root>/<sha1(id)>-<first>/<s>.mp4`, LRU by total size.
//! A file's mtime is its last use: hits touch it, eviction removes the oldest first.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::media::pyramid::cache_stem;

pub const DEFAULT_LIMIT_MB: u32 = 2048;
/// Where jobs put their temp files. Emptied on start.
const TMP: &str = "tmp";

pub struct PreviewCache {
    pub root: PathBuf,
    limit_bytes: AtomicU64,
}

impl PreviewCache {
    pub fn new(root: PathBuf, limit_mb: u32) -> Self {
        let _ = std::fs::remove_dir_all(root.join(TMP));
        Self { root, limit_bytes: AtomicU64::new(mb(limit_mb)) }
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join(TMP)
    }

    /// Segment indexes are counted from the first chunk on disk, so the folder includes it:
    /// after pruning, old files are not reused. The suffix is the preview height: the 360p
    /// segments from before M11 are not reused either.
    pub fn session_dir(&self, id: &str, first: u32) -> PathBuf {
        self.root.join(format!("{}-{first}-720", cache_stem(id)))
    }

    pub fn set_limit_mb(&self, limit_mb: u32) {
        self.limit_bytes.store(mb(limit_mb), Ordering::Relaxed);
    }

    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes.load(Ordering::Relaxed)
    }

    /// Marks a cached segment as just used.
    pub fn touch(path: &Path) {
        if let Ok(f) = std::fs::File::options().write(true).open(path) {
            let _ = f.set_modified(SystemTime::now());
        }
    }

    /// Deletes the oldest segments until the cache fits the limit. Blocking.
    pub fn evict(&self) -> u64 {
        evict(&self.root, self.limit_bytes())
    }

    /// Removes this session's folders for other `first` chunk numbers. Blocking.
    pub fn remove_stale_sessions(&self, id: &str, first: u32) {
        let keep = self.session_dir(id, first);
        let prefix = format!("{}-", cache_stem(id));
        let Ok(rd) = std::fs::read_dir(&self.root) else { return };
        for entry in rd.flatten() {
            let path = entry.path();
            let is_session = entry.file_name().to_str().is_some_and(|n| n.starts_with(&prefix));
            if is_session && path != keep {
                log::info!("removing stale preview cache {}", path.display());
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }

    /// Total size of all cached segments.
    pub fn size(&self) -> u64 {
        segments(&self.root).iter().map(|s| s.1).sum()
    }

    /// Deletes every cached segment. Blocking.
    pub fn clear(&self) {
        evict(&self.root, 0);
    }
}

fn mb(v: u32) -> u64 {
    u64::from(v) * 1024 * 1024
}

/// `(path, size, mtime)` of every finished segment (`*.mp4`) one folder below `root`.
fn segments(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else { return out };
    for dir in dirs.flatten() {
        if dir.file_name() == TMP {
            continue;
        }
        let Ok(files) = std::fs::read_dir(dir.path()) else { continue };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_none_or(|e| e != "mp4") {
                continue;
            }
            if let Ok(meta) = f.metadata() {
                out.push((path, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
    }
    out
}

/// Deletes the least recently used segments until at most `limit` bytes are left, and removes
/// folders that became empty. Returns the bytes left.
pub fn evict(root: &Path, limit: u64) -> u64 {
    let mut files = segments(root);
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return total;
    }
    files.sort_by_key(|f| f.2);
    for (path, size, _) in files {
        if total <= limit {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= size;
            if let Some(dir) = path.parent() {
                let _ = std::fs::remove_dir(dir); // only succeeds when empty
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn write(path: &Path, size: usize, age_s: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; size]).unwrap();
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(age_s)).unwrap();
    }

    #[test]
    fn evicts_oldest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), 1);
        let a = cache.session_dir("1/bg/a", 1);
        let b = cache.session_dir("1/bg/b", 1);
        write(&a.join("0.mp4"), 400_000, 300);
        write(&a.join("1.mp4"), 400_000, 100);
        write(&b.join("0.mp4"), 400_000, 200);
        write(&b.join("1.mp4"), 400_000, 10);
        write(&b.join("2.mp4.part"), 900_000, 1000); // in progress: never counted or removed
        assert_eq!(cache.size(), 1_600_000);

        PreviewCache::touch(&a.join("0.mp4")); // now the newest
        let left = cache.evict();
        assert_eq!(left, 800_000);
        assert!(a.join("0.mp4").exists());
        assert!(!a.join("1.mp4").exists());
        assert!(!b.join("0.mp4").exists());
        assert!(b.join("1.mp4").exists());
        assert!(b.join("2.mp4.part").exists());

        cache.clear();
        assert_eq!(cache.size(), 0);
        assert!(!a.exists(), "empty session folders are removed");
    }

    #[test]
    fn stale_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(tmp.path().to_path_buf(), DEFAULT_LIMIT_MB);
        let old = cache.session_dir("1/bg/x", 1);
        let current = cache.session_dir("1/bg/x", 3418);
        let other = cache.session_dir("1/bg/y", 1);
        for d in [&old, &current, &other] {
            write(&d.join("0.mp4"), 10, 0);
        }
        cache.remove_stale_sessions("1/bg/x", 3418);
        assert!(!old.exists());
        assert!(current.exists());
        assert!(other.exists());
    }
}
