//! Library thumbnails (M9), served as `replay://localhost/thumb/<urlencoded id>.jpg`.
//! A clip uses Steam's own `thumbnail.jpg`; a background recording gets the first frame of its
//! middle chunk. Both are cached by session id, so a hit needs no Steam discovery.
//!
//! Hover frames (M13), served as `replay://localhost/frame/<urlencoded id>/<k>.jpg`: the first
//! frame (a keyframe) of chunk `k` counted from video time 0, for the timeline's hover card.

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::chunks::{self, VIDEO_INIT};
use super::ffmpeg::{self, FfmpegPaths};
use super::pyramid::cache_stem;
use crate::error::{AppError, AppResult};
use crate::steam::recordings::{ChunkInventory, SessionDetail, SessionKind};

/// Thumbnail width in pixels; the library shows them at 96 px (192 px on HiDPI).
const WIDTH: u32 = 320;
/// Hover frame height in pixels (by height: recordings can be 32:9); shown at 72 px.
const FRAME_HEIGHT: u32 = 144;

/// At most this many ffmpeg extractions at once, so scrolling the library stays smooth.
static EXTRACT: Semaphore = Semaphore::const_new(2);
/// Hover frames get their own single slot, so they never wait for library thumbnails.
static FRAME: Semaphore = Semaphore::const_new(1);

/// `/thumb/<id>.jpg` → `id`.
pub fn parse_path(path: &str) -> Option<String> {
    let id = path.strip_prefix("/thumb/")?.strip_suffix(".jpg")?;
    let id = percent_decode_str(id).decode_utf8().ok()?.into_owned();
    id.split('/').all(|p| !p.is_empty()).then_some(id)
}

pub fn cache_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.jpg", cache_stem(id)))
}

/// Steam's `thumbnail.jpg` of a clip. The session dir is `clips/clip_*/video/bg_*/`.
pub fn clip_thumbnail(session_dir: &Path) -> Option<PathBuf> {
    session_dir
        .ancestors()
        .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("clip_")))
        .map(|p| p.join("thumbnail.jpg"))
        .filter(|p| p.is_file())
}

/// `/frame/<id>/<k>.jpg` → `(id, k)`.
pub fn parse_frame_path(path: &str) -> Option<(String, u32)> {
    let (id, file) = path.strip_prefix("/frame/")?.rsplit_once('/')?;
    let k = file.strip_suffix(".jpg")?.parse().ok()?;
    let id = percent_decode_str(id).decode_utf8().ok()?.into_owned();
    id.split('/').all(|p| !p.is_empty()).then_some((id, k))
}

/// The chunk in the middle of the recording that exists on disk.
pub fn middle_chunk(inv: &ChunkInventory) -> Option<u32> {
    if inv.count == 0 {
        return None;
    }
    let mid = inv.first + (inv.last - inv.first) / 2;
    (mid..=inv.last).chain((inv.first..mid).rev()).find(|n| !inv.missing.contains(n))
}

/// Chunk `n` if it exists on disk, else the closest one that does (the later one on a tie).
pub fn nearest_chunk(inv: &ChunkInventory, n: u32) -> Option<u32> {
    if inv.count == 0 {
        return None;
    }
    let n = n.clamp(inv.first, inv.last);
    let exists = |c: &u32| inv.missing.binary_search(c).is_err();
    (0..=inv.last - inv.first).find_map(|d| {
        let later = n.checked_add(d).filter(|c| *c <= inv.last && exists(c));
        later.or_else(|| n.checked_sub(d).filter(|c| *c >= inv.first && exists(c)))
    })
}

/// First frame of chunk `n` as JPEG, scaled with the ffmpeg filter `scale`.
async fn extract_jpeg(paths: &FfmpegPaths, session_dir: &Path, n: u32, scale: &str) -> AppResult<Vec<u8>> {
    let data = chunks::read_files(session_dir, &[VIDEO_INIT.to_owned(), chunks::video_chunk(n)]).await?;
    let mut cmd = ffmpeg::ffmpeg_command(paths, false);
    cmd.args(["-f", "mp4", "-i", "pipe:0", "-frames:v", "1", "-vf", scale])
        .args(["-q:v", "5", "-f", "image2", "-c:v", "mjpeg", "pipe:1"]);
    let jpeg = ffmpeg::run(cmd, Some(data), None, &CancellationToken::new()).await?;
    if jpeg.is_empty() {
        return Err(AppError::Ffmpeg(format!("no frame in chunk {n} of {}", session_dir.display())));
    }
    Ok(jpeg)
}

/// Writes `data` to `out` through a temporary file, so a half-written file is never served.
async fn write_atomic(out: &Path, data: &[u8]) -> AppResult<()> {
    let tmp = out.with_extension("tmp");
    tokio::fs::write(&tmp, data).await.map_err(|e| AppError::io(tmp.display(), e))?;
    tokio::fs::rename(&tmp, out).await.map_err(|e| AppError::io(out.display(), e))
}

/// The cached thumbnail of `id`, if there is one.
pub fn cached(dir: &Path, id: &str) -> Option<PathBuf> {
    Some(cache_path(dir, id)).filter(|p| p.is_file())
}

/// Creates the thumbnail of `detail` in `dir` and returns its path.
pub async fn create(paths: &FfmpegPaths, dir: &Path, detail: &SessionDetail) -> AppResult<PathBuf> {
    let id = &detail.summary.id;
    let _permit = EXTRACT.acquire().await.map_err(|e| AppError::Io(e.to_string()))?;
    // Another request may have made it while this one waited.
    if let Some(p) = cached(dir, id) {
        return Ok(p);
    }
    tokio::fs::create_dir_all(dir).await.map_err(|e| AppError::io(dir.display(), e))?;
    let out = cache_path(dir, id);

    let clip = matches!(detail.summary.kind, SessionKind::Clip).then(|| clip_thumbnail(&detail.summary.dir)).flatten();
    let jpeg = if let Some(src) = clip {
        tokio::fs::read(&src).await.map_err(|e| AppError::io(src.display(), e))?
    } else {
        let n = middle_chunk(&detail.chunks).ok_or_else(|| AppError::NotFound(format!("{id} has no video")))?;
        extract_jpeg(paths, &detail.summary.dir, n, &format!("scale={WIDTH}:-2")).await?
    };
    write_atomic(&out, &jpeg).await?;
    Ok(out)
}

/// The hover frame of chunk index `k` (from video time 0) of an open session, created on the
/// first request in `<dir>/frames/<stem>/<chunk>.jpg`. Chunk numbers never change when Steam
/// prunes a recording, so the cache stays valid.
pub async fn frame(paths: &FfmpegPaths, dir: &Path, id: &str, session_dir: &Path, inv: &ChunkInventory, k: u32) -> AppResult<PathBuf> {
    let n = nearest_chunk(inv, inv.first.saturating_add(k)).ok_or_else(|| AppError::NotFound(format!("{id} has no video")))?;
    let folder = dir.join("frames").join(cache_stem(id));
    let out = folder.join(format!("{n}.jpg"));
    if out.is_file() {
        return Ok(out);
    }
    let _permit = FRAME.acquire().await.map_err(|e| AppError::Io(e.to_string()))?;
    if out.is_file() {
        return Ok(out);
    }
    tokio::fs::create_dir_all(&folder).await.map_err(|e| AppError::io(folder.display(), e))?;
    let t = std::time::Instant::now();
    let jpeg = extract_jpeg(paths, session_dir, n, &format!("scale=-2:{FRAME_HEIGHT}")).await?;
    log::debug!("hover frame {n} of {id}: {:?}", t.elapsed());
    write_atomic(&out, &jpeg).await?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(
            parse_path("/thumb/10000001%2Fbg%2Fbg_1445790_20261015_210120.jpg"),
            Some("10000001/bg/bg_1445790_20261015_210120".to_owned())
        );
        assert_eq!(parse_path("/thumb/.jpg"), None);
        assert_eq!(parse_path("/thumb/a%2F%2Fb.jpg"), None);
        assert_eq!(parse_path("/thumb/x.png"), None);
        assert_eq!(parse_path("/preview/x/1.mp4"), None);
        assert_eq!(
            parse_frame_path("/frame/10000001%2Fbg%2Fbg_1445790_20261015_210120/42.jpg"),
            Some(("10000001/bg/bg_1445790_20261015_210120".to_owned(), 42))
        );
        assert_eq!(parse_frame_path("/frame/a%2F%2Fb/1.jpg"), None);
        assert_eq!(parse_frame_path("/frame//1.jpg"), None);
        assert_eq!(parse_frame_path("/frame/x/-1.jpg"), None);
        assert_eq!(parse_frame_path("/frame/x/1.png"), None);
        assert_eq!(parse_frame_path("/thumb/x.jpg"), None);
    }

    #[test]
    fn middle() {
        let inv = |first, last, missing: Vec<u32>| ChunkInventory {
            first,
            last,
            count: last - first + 1 - missing.len() as u32,
            missing,
        };
        assert_eq!(middle_chunk(&inv(1, 9, vec![])), Some(5));
        assert_eq!(middle_chunk(&inv(3418, 3418, vec![])), Some(3418));
        assert_eq!(middle_chunk(&inv(1, 9, vec![5, 6, 7, 8, 9])), Some(4));
        assert_eq!(middle_chunk(&inv(1, 4, vec![2, 3])), Some(4));
        assert_eq!(middle_chunk(&ChunkInventory::default()), None);
    }

    #[test]
    fn nearest() {
        let inv = |first, last, missing: Vec<u32>| ChunkInventory {
            first,
            last,
            count: last - first + 1 - missing.len() as u32,
            missing,
        };
        let pruned = inv(3418, 3427, vec![]);
        assert_eq!(nearest_chunk(&pruned, 3418), Some(3418));
        assert_eq!(nearest_chunk(&pruned, 3418 + 5), Some(3423));
        assert_eq!(nearest_chunk(&pruned, 99_999), Some(3427));
        assert_eq!(nearest_chunk(&pruned, 0), Some(3418));
        let holes = inv(1, 9, vec![4, 5, 6]);
        assert_eq!(nearest_chunk(&holes, 4), Some(3));
        assert_eq!(nearest_chunk(&holes, 5), Some(7));
        assert_eq!(nearest_chunk(&holes, 6), Some(7));
        assert_eq!(nearest_chunk(&ChunkInventory::default(), 1), None);
    }

    /// Real recording: `STEAMREPLAY_SESSION_DIR=<…/video/bg_…> cargo test frame_fixture -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn frame_fixture() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let dir = PathBuf::from(dir);
        let (inv, _) = crate::steam::recordings::inventory(&dir).unwrap();
        let paths = ffmpeg::locate(None).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let t = std::time::Instant::now();
        let first = frame(&paths, cache.path(), "test/bg", &dir, &inv, 1).await.unwrap();
        let cold = t.elapsed();
        let t = std::time::Instant::now();
        let again = frame(&paths, cache.path(), "test/bg", &dir, &inv, 1).await.unwrap();
        eprintln!("cold {cold:?}, cached {:?}", t.elapsed());
        assert_eq!(first, again);
        let jpeg = std::fs::read(&first).unwrap();
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        // Past the end: the last chunk.
        let last = frame(&paths, cache.path(), "test/bg", &dir, &inv, u32::MAX).await.unwrap();
        assert!(last.ends_with(format!("{}.jpg", inv.last)));
    }

    #[test]
    fn clip_lookup() {
        let root = tempfile::tempdir().unwrap();
        let clip = root.path().join("clips/clip_1478500_20260913_011121");
        let session = clip.join("video/bg_1478500_20260913_011121");
        std::fs::create_dir_all(&session).unwrap();
        assert_eq!(clip_thumbnail(&session), None);
        std::fs::write(clip.join("thumbnail.jpg"), b"jpg").unwrap();
        assert_eq!(clip_thumbnail(&session), Some(clip.join("thumbnail.jpg")));
        assert_eq!(clip_thumbnail(&root.path().join("video/bg_1")), None);
    }
}
