//! Min/max/RMS pyramid over the 10 ms bins, and its disk cache.

use std::path::{Path, PathBuf};

use serde::Serialize;
use specta::Type;

use super::audio::{BIN_MS, Bin};
use crate::error::{AppError, AppResult};

/// Each level merges this many bins of the level below.
pub const FACTOR: usize = 4;
/// Stop adding levels once a level is smaller than this.
const MIN_TOP_BINS: usize = 2000;

const MAGIC: &[u8; 4] = b"SRWF";
const VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 2 + 4;

/// Flat `[min, max, rms]*` as sent to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(transparent)]
pub struct Samples(#[specta(type = Vec<specta_typescript::Number>)] pub Vec<f32>);

#[derive(Debug, Clone, PartialEq)]
pub struct Pyramid {
    /// `levels[0]` holds the 10 ms bins; `levels[k]` bins span `10 ms · 4^k`.
    pub levels: Vec<Vec<Bin>>,
}

fn merge(bins: &[Bin]) -> Bin {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut sq = 0.0f64;
    for b in bins {
        min = min.min(b[0]);
        max = max.max(b[1]);
        sq += f64::from(b[2]) * f64::from(b[2]);
    }
    [min, max, (sq / bins.len() as f64).sqrt() as f32]
}

impl Pyramid {
    pub fn build(level0: Vec<Bin>) -> Self {
        let mut levels = vec![level0];
        while let Some(top) = levels.last()
            && top.len() >= MIN_TOP_BINS
        {
            let next: Vec<Bin> = top.chunks(FACTOR).map(merge).collect();
            levels.push(next);
        }
        Self { levels }
    }

    pub fn level0(&self) -> &[Bin] {
        &self.levels[0]
    }

    /// Flat `[min, max, rms]*` for `count` bins from `start` (clamped to the level).
    pub fn slice(&self, level: usize, start: usize, count: usize) -> AppResult<Samples> {
        let bins = self.levels.get(level).ok_or_else(|| AppError::NotFound(format!("no waveform level {level}")))?;
        let start = start.min(bins.len());
        let end = start.saturating_add(count).min(bins.len());
        Ok(Samples(bins[start..end].iter().flatten().copied().collect()))
    }
}

/// `<sha1(session id)>`: the stable part of the cache file name.
pub fn cache_stem(session_id: &str) -> String {
    sha1_smol::Sha1::from(session_id).digest().to_string()
}

pub fn cache_path(dir: &Path, session_id: &str, first: u32, last: u32) -> PathBuf {
    dir.join(format!("{}-{first}-{last}.bin", cache_stem(session_id)))
}

fn encode(level0: &[Bin]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + level0.len() * 12);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&BIN_MS.to_le_bytes());
    out.extend_from_slice(&(level0.len() as u32).to_le_bytes());
    for b in level0 {
        for v in b {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// `None` for anything that is not a complete version-1 file with 10 ms bins.
fn decode(data: &[u8]) -> Option<Vec<Bin>> {
    let header = data.get(..HEADER_LEN)?;
    if &header[..4] != MAGIC
        || u16::from_le_bytes([header[4], header[5]]) != VERSION
        || u16::from_le_bytes([header[6], header[7]]) != BIN_MS
    {
        return None;
    }
    let bins = u32::from_le_bytes([header[8], header[9], header[10], header[11]]) as usize;
    let body = &data[HEADER_LEN..];
    if body.len() != bins * 12 {
        return None;
    }
    Some(body.as_chunks::<4>().0.as_chunks::<3>().0.iter().map(|b| b.map(f32::from_le_bytes)).collect())
}

/// Loads level 0 from the cache. A missing, old or broken file gives `None` (rebuild).
pub fn load(path: &Path) -> Option<Vec<Bin>> {
    let data = std::fs::read(path).ok()?;
    let bins = decode(&data);
    if bins.is_none() {
        log::warn!("ignoring unreadable waveform cache {}", path.display());
    }
    bins
}

/// Writes atomically (temp file + rename) and removes stale files of the same session.
pub fn save(path: &Path, level0: &[Bin]) -> AppResult<()> {
    write_cache(path, &encode(level0))
}

/// Writes a cache file atomically (temp file + rename) and removes the other `<stem>-*.bin` files
/// of the same session next to it. Shared with the laughter cache.
pub fn write_cache(path: &Path, data: &[u8]) -> AppResult<()> {
    let dir = path.parent().ok_or_else(|| AppError::Io(format!("bad cache path {}", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| AppError::io(dir.display(), e))?;
    let tmp = path.with_extension("tmp");
    let result = std::fs::write(&tmp, data).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::io(path.display(), e));
    }
    remove_stale(path);
    Ok(())
}

/// Deletes other `<stem>-*.bin` files next to `keep` (the session was pruned or grew).
fn remove_stale(keep: &Path) {
    let (Some(dir), Some(name)) = (keep.parent(), keep.file_name().and_then(|n| n.to_str())) else { return };
    let Some((stem, _)) = name.split_once('-') else { return };
    let prefix = format!("{stem}-");
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let n = entry.file_name();
        let Some(n) = n.to_str() else { continue };
        if n.starts_with(&prefix)
            && n != name
            && (n.ends_with(".bin") || n.ends_with(".tmp"))
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            log::warn!("removing stale cache {n}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduction() {
        let mut level0 = vec![[0.0, 0.0, 0.0]; 8000];
        level0[0] = [-1.0, 0.5, 1.0];
        level0[1] = [-0.2, 0.9, 1.0];
        level0[2] = [-0.1, 0.1, 1.0];
        level0[3] = [-0.3, 0.2, 1.0];
        level0[4] = [-0.1, 0.1, 2.0];
        let p = Pyramid::build(level0);
        // 8000 → 2000 → 500 (stop: below 2000)
        assert_eq!(p.levels.iter().map(Vec::len).collect::<Vec<_>>(), vec![8000, 2000, 500]);
        assert_eq!(p.levels[1][0], [-1.0, 0.9, 1.0]);
        assert_eq!(p.levels[1][1], [-0.1, 0.1, 1.0]); // sqrt(4 / 4)
        let top = p.levels[2][0];
        assert_eq!(&top[..2], &[-1.0, 0.9]);
        assert!((top[2] - (2.0f32 / 4.0).sqrt()).abs() < 1e-6); // sqrt((1 + 1 + 0 + 0) / 4)
        assert_eq!(p.slice(1, 0, 1).unwrap().0, vec![-1.0, 0.9, 1.0]);
        assert_eq!(p.slice(2, 499, 10).unwrap().0.len(), 3);
        assert!(p.slice(2, 600, 10).unwrap().0.is_empty());
        assert!(p.slice(3, 0, 1).is_err());

        let small = Pyramid::build(vec![[0.0; 3]; 10]);
        assert_eq!(small.levels.len(), 1);
        // A partial group at the end still makes one bin.
        let odd = Pyramid::build(vec![[0.0, 0.0, 1.0]; 2001]);
        assert_eq!(odd.levels[1].len(), 501);
    }

    #[test]
    fn cache_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let level0: Vec<Bin> = (0..1000).map(|i| [-(i as f32), i as f32, 0.5]).collect();
        let old = cache_path(dir.path(), "1/bg/x", 1, 10);
        save(&old, &level0[..10]).unwrap();
        let other_session = cache_path(dir.path(), "1/bg/y", 1, 10);
        save(&other_session, &level0[..10]).unwrap();

        let path = cache_path(dir.path(), "1/bg/x", 3, 12);
        save(&path, &level0).unwrap();
        assert_eq!(load(&path).unwrap(), level0);
        assert!(!old.exists(), "stale file of the same session is removed");
        assert!(other_session.exists());
        assert!(!path.with_extension("tmp").exists());

        // Version mismatch and truncation → rebuild.
        let mut data = std::fs::read(&path).unwrap();
        data[4] = 2;
        std::fs::write(&path, &data).unwrap();
        assert!(load(&path).is_none());
        data[4] = 1;
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(load(&path).is_none());
        assert!(load(&dir.path().join("missing.bin")).is_none());
    }

    #[test]
    fn failed_save_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        // The target is a directory, so the rename fails.
        let path = cache_path(dir.path(), "s", 1, 2);
        std::fs::create_dir_all(path.join("blocker")).unwrap();
        assert!(save(&path, &[[0.0; 3]]).is_err());
        assert!(!path.with_extension("tmp").exists());
    }
}
