//! Library thumbnails (M9), served as `replay://localhost/thumb/<urlencoded id>.jpg`.
//! A clip uses Steam's own `thumbnail.jpg`; a background recording gets the first frame of its
//! middle chunk. Both are cached by session id, so a hit needs no Steam discovery.

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

/// At most this many ffmpeg extractions at once, so scrolling the library stays smooth.
static EXTRACT: Semaphore = Semaphore::const_new(2);

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

/// The chunk in the middle of the recording that exists on disk.
pub fn middle_chunk(inv: &ChunkInventory) -> Option<u32> {
    if inv.count == 0 {
        return None;
    }
    let mid = inv.first + (inv.last - inv.first) / 2;
    (mid..=inv.last).chain((inv.first..mid).rev()).find(|n| !inv.missing.contains(n))
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
    let tmp = out.with_extension("tmp");

    let clip = matches!(detail.summary.kind, SessionKind::Clip).then(|| clip_thumbnail(&detail.summary.dir)).flatten();
    if let Some(src) = clip {
        tokio::fs::copy(&src, &tmp).await.map_err(|e| AppError::io(src.display(), e))?;
    } else {
        let n = middle_chunk(&detail.chunks).ok_or_else(|| AppError::NotFound(format!("{id} has no video")))?;
        let data = chunks::read_files(&detail.summary.dir, &[VIDEO_INIT.to_owned(), chunks::video_chunk(n)]).await?;
        let mut cmd = ffmpeg::ffmpeg_command(paths, false);
        cmd.args(["-f", "mp4", "-i", "pipe:0", "-frames:v", "1", "-vf"])
            .arg(format!("scale={WIDTH}:-2"))
            .args(["-q:v", "5", "-f", "image2", "-c:v", "mjpeg", "pipe:1"]);
        let jpeg = ffmpeg::run(cmd, Some(data), None, &CancellationToken::new()).await?;
        if jpeg.is_empty() {
            return Err(AppError::Ffmpeg(format!("no frame for {id}")));
        }
        tokio::fs::write(&tmp, jpeg).await.map_err(|e| AppError::io(tmp.display(), e))?;
    }
    // Renamed into place, so a half-written file is never served.
    tokio::fs::rename(&tmp, &out).await.map_err(|e| AppError::io(out.display(), e))?;
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
