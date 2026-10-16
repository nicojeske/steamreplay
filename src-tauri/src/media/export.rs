//! Fast export (M3): the chunks around the selection, joined and muxed with stream copy.
//! The cut snaps to chunk boundaries, because each chunk starts with the only keyframe in it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{NaiveDateTime, TimeDelta};
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::chunks::{AUDIO_INIT, VIDEO_INIT, audio_chunk, video_chunk};
use super::ffmpeg::{self, FfmpegPaths, ProgressFn};
use crate::error::{AppError, AppResult};
use crate::steam::recordings::ChunkInventory;

/// A manifest changed more recently than this belongs to a running game: its last chunk may still
/// be being written.
pub const LIVE_MPD_AGE_S: u64 = 5;

#[derive(Debug, Clone, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub id: String,
    #[specta(type = specta_typescript::Number)]
    pub in_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub out_s: f64,
    /// Default: [`default_file_name`] in the account's export folder.
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: PathBuf,
    #[specta(type = specta_typescript::Number)]
    pub actual_in_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub actual_out_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub size_bytes: u64,
}

/// The selection snapped to chunk boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Snap {
    pub first: u32,
    pub last: u32,
    #[specta(type = specta_typescript::Number)]
    pub actual_in_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub actual_out_s: f64,
    /// Average chunk size × chunks.
    #[specta(type = specta_typescript::Number)]
    pub est_bytes: u64,
}

impl Snap {
    pub fn chunks(&self) -> u32 {
        self.last - self.first + 1
    }
}

/// The chunks that cover `[in_s, out_s)` (video time). Always at least one chunk.
/// `drop_last` leaves out the newest chunk of a live session.
pub fn chunk_range(
    inv: &ChunkInventory,
    seg_s: f64,
    in_s: f64,
    out_s: f64,
    drop_last: bool,
    session_bytes: u64,
) -> AppResult<Snap> {
    if inv.count == 0 {
        return Err(AppError::NotFound("the recording has no chunks on disk".into()));
    }
    let last = if drop_last && inv.last > inv.first { inv.last - 1 } else { inv.last };
    let (a, b) = if in_s <= out_s { (in_s, out_s) } else { (out_s, in_s) };
    let at = |t: f64| inv.first as i64 + t.max(0.0).floor() as i64;
    let first = at(a / seg_s).clamp(inv.first as i64, last as i64) as u32;
    let end = (inv.first as i64 + (b / seg_s).ceil() as i64 - 1).clamp(first as i64, last as i64) as u32;
    if inv.missing.iter().any(|n| (first..=end).contains(n)) {
        return Err(AppError::NotFound("the selection crosses a gap in the recording".into()));
    }
    let chunks = u64::from(end - first + 1);
    Ok(Snap {
        first,
        last: end,
        actual_in_s: f64::from(first - inv.first) * seg_s,
        actual_out_s: f64::from(end - inv.first + 1) * seg_s,
        est_bytes: session_bytes / u64::from(inv.count.max(1)) * chunks,
    })
}

/// Removes characters Windows does not allow in file names.
pub fn sanitize_name(name: &str) -> String {
    let s: String =
        name.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')).collect();
    let s = s.trim().trim_end_matches(['.', ' ']).to_owned();
    if s.is_empty() { "Clip".to_owned() } else { s }
}

/// `<Game>_<YYYY-MM-DD_HH-MM-SS>.mp4`, the wall-clock time of `in_s`. `started_at` is the
/// session's local time of video time 0 (`SessionSummary::started_at`).
pub fn default_file_name(game: &str, started_at: &str, in_s: f64) -> String {
    let stamp = NaiveDateTime::parse_from_str(started_at, "%Y-%m-%dT%H:%M:%S")
        .map(|t| (t + TimeDelta::milliseconds((in_s * 1000.0).round() as i64)).format("%Y-%m-%d_%H-%M-%S").to_string())
        .unwrap_or_else(|_| format!("{in_s:.0}s"));
    format!("{}_{stamp}.mp4", sanitize_name(game).replace(' ', "_"))
}

/// `path`, or `name_2.ext`, `name_3.ext`, … if it exists.
pub fn dedupe(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_owned();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|i| path.with_file_name(format!("{stem}_{i}{ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

/// `<out>.part.mp4` next to the final file (a rename across volumes fails on Windows).
fn part_path(out: &Path) -> PathBuf {
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    out.with_file_name(format!("{stem}.part.mp4"))
}

/// Deletes the job folder and the partial output when dropped, so errors and cancels leave nothing.
struct Cleanup {
    job_dir: PathBuf,
    part: PathBuf,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.job_dir);
        let _ = std::fs::remove_file(&self.part);
    }
}

/// Hard link (same filesystem) or copy. A missing source means Steam pruned it.
async fn snapshot_file(src: &Path, dst: &Path) -> AppResult<()> {
    if std::fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    match tokio::fs::copy(src, dst).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(AppError::NotFound("part of the selection was removed by Steam".into()))
        }
        Err(e) => Err(AppError::io(src.display(), e)),
    }
}

/// Concatenates `parts` into `out` without holding them in memory.
async fn join(parts: &[PathBuf], out: &Path) -> AppResult<()> {
    let mut w = tokio::fs::File::create(out).await.map_err(|e| AppError::io(out.display(), e))?;
    for p in parts {
        let mut r = tokio::fs::File::open(p).await.map_err(|e| AppError::io(p.display(), e))?;
        tokio::io::copy(&mut r, &mut w).await.map_err(|e| AppError::io(out.display(), e))?;
    }
    w.flush().await.map_err(|e| AppError::io(out.display(), e))
}

/// Snapshots chunks `snap.first..=snap.last` of `dir` into `job_dir`, joins them and muxes them
/// into `out`. Progress: 0..0.5 snapshot, 0.5..1 mux. Returns the output size.
#[allow(clippy::too_many_arguments)]
pub async fn export(
    paths: &FfmpegPaths,
    dir: &Path,
    snap: &Snap,
    seg_s: f64,
    title: &str,
    out: &Path,
    job_dir: &Path,
    progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<u64> {
    let part = part_path(out);
    let _cleanup = Cleanup { job_dir: job_dir.to_owned(), part: part.clone() };
    tokio::fs::create_dir_all(job_dir).await.map_err(|e| AppError::io(job_dir.display(), e))?;
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| AppError::io(parent.display(), e))?;
    }

    // 1. Snapshot, so Steam pruning the buffer mid-export cannot break the join.
    let numbers: Vec<u32> = (snap.first..=snap.last).collect();
    let video: Vec<String> = std::iter::once(VIDEO_INIT.to_owned()).chain(numbers.iter().map(|&n| video_chunk(n))).collect();
    let audio: Vec<String> = std::iter::once(AUDIO_INIT.to_owned()).chain(numbers.iter().map(|&n| audio_chunk(n))).collect();
    let total = (video.len() + audio.len()) as f64;
    for (i, name) in video.iter().chain(&audio).enumerate() {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        snapshot_file(&dir.join(name), &job_dir.join(name)).await?;
        progress((i + 1) as f64 / total * 0.5);
    }

    // 2. Join init + chunks into one fragmented MP4 per stream.
    let (v, a) = (job_dir.join("v.mp4"), job_dir.join("a.mp4"));
    join(&video.iter().map(|n| job_dir.join(n)).collect::<Vec<_>>(), &v).await?;
    join(&audio.iter().map(|n| job_dir.join(n)).collect::<Vec<_>>(), &a).await?;
    for name in video.iter().chain(&audio) {
        let _ = tokio::fs::remove_file(job_dir.join(name)).await;
    }

    // 3. Mux with stream copy.
    let mut cmd = ffmpeg::ffmpeg_command(paths, true);
    cmd.arg("-i").arg(&v).arg("-i").arg(&a);
    cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy", "-tag:v", "hvc1", "-movflags", "+faststart"]);
    cmd.arg("-metadata").arg(format!("title={title}"));
    cmd.args(["-f", "mp4", "-y"]).arg(&part).stdout(Stdio::null());
    let mut child = ffmpeg::spawn(&mut cmd)?;
    let mux_progress: ProgressFn = {
        let progress = progress.clone();
        std::sync::Arc::new(move |f| progress(0.5 + f * 0.5))
    };
    let duration = f64::from(snap.chunks()) * seg_s;
    let stderr = ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"), Some((duration, mux_progress)));
    let status = ffmpeg::wait_or_kill(&mut child, cancel).await?;
    ffmpeg::check_exit(status, stderr, "export").await?;

    tokio::fs::rename(&part, out).await.map_err(|e| AppError::io(out.display(), e))?;
    progress(1.0);
    let size = tokio::fs::metadata(out).await.map_err(|e| AppError::io(out.display(), e))?.len();
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(first: u32, last: u32, missing: Vec<u32>) -> ChunkInventory {
        let count = last - first + 1 - missing.len() as u32;
        ChunkInventory { first, last, count, missing }
    }

    #[test]
    fn range_snaps_outwards() {
        let i = inv(3418, 4727, vec![]);
        let s = chunk_range(&i, 3.0, 10.0, 40.5, false, 0).unwrap();
        assert_eq!((s.first, s.last), (3418 + 3, 3418 + 13));
        assert_eq!((s.actual_in_s, s.actual_out_s), (9.0, 42.0));
        // Exactly on boundaries: no extra chunk.
        let s = chunk_range(&i, 3.0, 9.0, 42.0, false, 0).unwrap();
        assert_eq!((s.actual_in_s, s.actual_out_s), (9.0, 42.0));
    }

    #[test]
    fn range_min_one_chunk_and_swapped() {
        let i = inv(1, 100, vec![]);
        let s = chunk_range(&i, 3.0, 4.0, 4.0, false, 0).unwrap();
        assert_eq!((s.first, s.last), (2, 2));
        let s = chunk_range(&i, 3.0, 12.0, 6.0, false, 0).unwrap();
        assert_eq!((s.first, s.last), (3, 4));
    }

    #[test]
    fn range_clamps_and_drops_live_chunk() {
        let i = inv(1, 10, vec![]);
        let s = chunk_range(&i, 3.0, -5.0, 1000.0, false, 0).unwrap();
        assert_eq!((s.first, s.last), (1, 10));
        let s = chunk_range(&i, 3.0, 20.0, 1000.0, true, 0).unwrap();
        assert_eq!((s.first, s.last), (7, 9));
        let s = chunk_range(&i, 3.0, 29.0, 30.0, true, 0).unwrap();
        assert_eq!((s.first, s.last), (9, 9));
    }

    #[test]
    fn range_rejects_gaps_and_estimates_size() {
        let i = inv(10, 18, vec![13]);
        assert!(chunk_range(&i, 3.0, 6.0, 15.0, false, 0).is_err());
        let s = chunk_range(&i, 3.0, 0.0, 9.0, false, 800).unwrap();
        assert_eq!((s.first, s.last, s.est_bytes), (10, 12, 300));
        assert!(chunk_range(&ChunkInventory::default(), 3.0, 0.0, 3.0, false, 0).is_err());
    }

    #[test]
    fn names() {
        assert_eq!(sanitize_name(r#"Half-Life: Alyx <VR>?"#), "Half-Life Alyx VR");
        assert_eq!(sanitize_name("Game..."), "Game");
        assert_eq!(sanitize_name("///"), "Clip");
        assert_eq!(default_file_name("Foo: Bar", "2026-10-15T21:01:20", 61.6), "Foo_Bar_2026-10-15_21-02-21.mp4");
        assert_eq!(part_path(Path::new("x/a.mp4")), Path::new("x/a.part.mp4"));
    }

    #[test]
    fn dedupes() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.mp4");
        assert_eq!(dedupe(&p), p);
        std::fs::write(&p, b"").unwrap();
        assert_eq!(dedupe(&p), d.path().join("a_2.mp4"));
        std::fs::write(d.path().join("a_2.mp4"), b"").unwrap();
        assert_eq!(dedupe(&p), d.path().join("a_3.mp4"));
    }

    /// Needs ffmpeg/ffprobe on PATH and a real bg folder with at least 10 chunks:
    /// `STEAMREPLAY_SESSION_DIR=<…/video/bg_…> cargo test export_session -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn export_session() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let dir = PathBuf::from(dir);
        let (inv, size) = crate::steam::recordings::inventory(&dir).unwrap();
        let paths = ffmpeg::locate(None).unwrap();
        let snap = chunk_range(&inv, 3.0, 0.0, 30.0, false, size).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out.mp4");
        let job = tmp.path().join("job");
        let started = std::time::Instant::now();
        let bytes = export(&paths, &dir, &snap, 3.0, "test", &out, &job, std::sync::Arc::new(|_| {}), &CancellationToken::new())
            .await
            .unwrap();
        eprintln!("exported {bytes} bytes in {:?}", started.elapsed());
        assert!(!job.exists());
        assert!(!part_path(&out).exists());

        let mut cmd = ffmpeg::ffprobe_command(&paths);
        cmd.args(["-show_entries", "stream=codec_name:format=duration", "-of", "csv=p=0"]).arg(&out);
        let probe = ffmpeg::run(cmd, None, None, &CancellationToken::new()).await.unwrap();
        let text = String::from_utf8_lossy(&probe);
        assert!(text.contains("hevc") && text.contains("aac"), "{text}");
        let duration: f64 = text.lines().filter_map(|l| l.trim().parse().ok()).next_back().unwrap();
        assert!((duration - snap.actual_out_s + snap.actual_in_s).abs() < 0.1, "{text}");
    }
}
