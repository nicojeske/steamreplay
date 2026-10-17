//! Export of a selection. Both modes snapshot and join the chunks around it first.
//! - Fast (M3): stream copy. The cut snaps to chunk boundaries, because each chunk starts with the
//!   only keyframe in it.
//! - Precise (M5): re-encode, cut on the exact frames.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use chrono::{NaiveDateTime, TimeDelta};
use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::chunks::{AUDIO_INIT, VIDEO_INIT, audio_chunk, video_chunk};
use super::encoders::{self, AUDIO_KBPS, EncoderChoice, Quality, VideoCodec};
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
    pub mode: ExportMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportMode {
    /// Stream copy, cut at chunk boundaries.
    Fast,
    /// Re-encode, cut on the exact frames.
    Precise(PreciseOpts),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreciseOpts {
    pub codec: VideoCodec,
    /// 1080, 720, …; keeps the aspect ratio, never upscales. `None`: source height.
    pub max_height: Option<u32>,
    /// `None`: source frame rate.
    pub fps: Option<u32>,
    /// Centre crop to 16:9 for sources wider than that.
    pub crop_16x9: bool,
    pub target: Quality,
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
    /// ffmpeg encoder name, or `"copy"` for Fast.
    pub encoder: String,
    /// The hardware encoder failed and the software one made the file.
    pub fell_back: bool,
}

/// What the export panel remembers between runs (settings keys `exportMode`, `exportPrecise`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ExportModeKind {
    Fast,
    Precise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ExportPreset {
    Discord,
    DiscordNitro,
    Original,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PrecisePrefs {
    pub preset: ExportPreset,
    pub opts: PreciseOpts,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ExportPrefs {
    pub mode: Option<ExportModeKind>,
    pub precise: Option<PrecisePrefs>,
}

/// Recent exports kept in the settings store.
pub const RECENT_EXPORTS: usize = 20;

/// A finished export, for the "Recent exports" list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentExport {
    pub path: PathBuf,
    pub game_name: String,
    #[specta(type = specta_typescript::Number)]
    pub in_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub out_s: f64,
    pub mode: ExportModeKind,
    #[specta(type = specta_typescript::Number)]
    pub size_bytes: u64,
    /// RFC 3339 local time.
    pub finished_at: String,
    /// The share link once the file was uploaded to Fireshare (M10).
    #[serde(default)]
    pub fireshare_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub entry: RecentExport,
    /// The file is still there (false also when its drive is not connected).
    pub exists: bool,
}

/// Puts `entry` first, drops an older entry for the same file and keeps the newest
/// [`RECENT_EXPORTS`].
pub fn push_recent(list: Vec<RecentExport>, entry: RecentExport) -> Vec<RecentExport> {
    let mut out = Vec::with_capacity(RECENT_EXPORTS);
    let path = entry.path.clone();
    out.push(entry);
    out.extend(list.into_iter().filter(|e| e.path != path));
    out.truncate(RECENT_EXPORTS);
    out
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

/// The exact cut inside the snapped chunks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreciseRange {
    /// Where the cut starts in the joined file, which begins at `snap.actual_in_s`.
    pub offset_s: f64,
    pub duration_s: f64,
    pub actual_in_s: f64,
    pub actual_out_s: f64,
}

/// `[in_s, out_s]` ordered, clamped to the snapped chunks (which already drop a live session's
/// newest chunk) and at least one frame long.
pub fn precise_range(snap: &Snap, in_s: f64, out_s: f64, frame_s: f64) -> PreciseRange {
    let (lo, hi) = (snap.actual_in_s, snap.actual_out_s);
    let (a, b) = if in_s <= out_s { (in_s, out_s) } else { (out_s, in_s) };
    let (mut a, mut b) = (a.clamp(lo, hi), b.clamp(lo, hi));
    if b - a < frame_s {
        b = (a + frame_s).min(hi);
        a = (b - frame_s).max(lo);
    }
    PreciseRange { offset_s: a - lo, duration_s: b - a, actual_in_s: a, actual_out_s: b }
}

/// `-vf` of a precise export: optional 16:9 centre crop, downscale (never up) with conversion to
/// limited range, frame rate, pixel format. `src` is the source size from the manifest; without
/// it the same is done with ffmpeg expressions.
pub fn filter_chain(src: Option<(u32, u32)>, opts: &PreciseOpts, pix_fmt: &str) -> String {
    let mut f = Vec::new();
    let max_h = opts.max_height.map(|h| h & !1);
    match src {
        Some((w, h)) => {
            if opts.crop_16x9 && f64::from(w) * 9.0 > f64::from(h) * 16.0 * 1.01 {
                let cw = (u64::from(h) * 16 / 9 / 2 * 2) as u32;
                f.push(format!("crop={cw}:{h}"));
            }
            match max_h {
                Some(m) if m < h => f.push(format!("scale=-2:{m}:out_range=tv")),
                _ => f.push("scale=out_range=tv".to_owned()),
            }
        }
        None => {
            if opts.crop_16x9 {
                f.push("crop='min(iw,trunc(ih*16/9/2)*2)':ih".to_owned());
            }
            match max_h {
                Some(m) => f.push(format!("scale=-2:'min(ih,{m})':out_range=tv")),
                None => f.push("scale=out_range=tv".to_owned()),
            }
        }
    }
    if let Some(fps) = opts.fps {
        f.push(format!("fps={fps}"));
    }
    f.push(format!("format={pix_fmt}"));
    f.join(",")
}

/// Everything after `ffmpeg_command(paths, true)` for a precise export of `v` + `a` into `out`.
#[allow(clippy::too_many_arguments)]
pub fn encode_args(
    choice: &EncoderChoice,
    opts: &PreciseOpts,
    src: Option<(u32, u32)>,
    range: &PreciseRange,
    title: &str,
    v: &Path,
    a: &Path,
    out: &Path,
) -> AppResult<Vec<OsString>> {
    let enc = choice.encoder.as_str();
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |xs: &[&str]| args.extend(xs.iter().map(OsString::from));
    if enc.ends_with("_vaapi") {
        // `hwupload` needs a device; VAAPI is Linux-only and best effort.
        push(&["-vaapi_device", "/dev/dri/renderD128"]);
    }
    if let Some(hw) = choice.hwaccel {
        push(&["-hwaccel", hw]);
    }
    args.extend(["-i".into(), v.as_os_str().to_owned(), "-i".into(), a.as_os_str().to_owned()]);
    // After `-i`: an output option, so the cut is frame-accurate.
    args.extend([
        "-ss".into(),
        format!("{:.6}", range.offset_s).into(),
        "-t".into(),
        format!("{:.6}", range.duration_s).into(),
    ]);
    args.extend(["-map", "0:v:0", "-map", "1:a:0", "-vf"].map(OsString::from));
    args.push(filter_chain(src, opts, encoders::pix_fmt(enc)).into());
    args.extend(["-c:v".into(), enc.into()]);
    args.extend(encoders::rate_args(enc, opts.target, range.duration_s)?.into_iter().map(OsString::from));
    if opts.codec == VideoCodec::Hevc {
        args.extend(["-tag:v", "hvc1"].map(OsString::from));
    }
    args.extend(["-color_range".into(), "tv".into(), "-c:a".into(), "aac".into(), "-b:a".into(), format!("{AUDIO_KBPS}k").into()]);
    args.extend(["-movflags", "+faststart", "-metadata"].map(OsString::from));
    args.push(format!("title={title}").into());
    args.extend(["-f", "mp4", "-y"].map(OsString::from));
    args.push(out.as_os_str().to_owned());
    Ok(args)
}

/// How the joined chunks become the output file.
#[derive(Debug, Clone)]
pub enum Plan {
    Copy,
    Encode {
        choice: EncoderChoice,
        /// Software encoder to retry with when a hardware encode fails.
        fallback: Option<EncoderChoice>,
        opts: PreciseOpts,
        src: Option<(u32, u32)>,
        range: PreciseRange,
    },
}

pub struct Exported {
    pub size_bytes: u64,
    pub encoder: String,
    pub fell_back: bool,
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

/// Snapshots chunks `snap.first..=snap.last` of `dir` into `job_dir`, joins them, and muxes or
/// encodes them into `out` as `plan` says. Progress: 0..0.5 snapshot, 0.5..1 mux/encode.
#[allow(clippy::too_many_arguments)]
pub async fn export(
    paths: &FfmpegPaths,
    dir: &Path,
    snap: &Snap,
    seg_s: f64,
    title: &str,
    out: &Path,
    job_dir: &Path,
    plan: &Plan,
    progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<Exported> {
    let part = part_path(out);
    let _cleanup = Cleanup { job_dir: job_dir.to_owned(), part: part.clone() };
    tokio::fs::create_dir_all(job_dir).await.map_err(|e| AppError::io(job_dir.display(), e))?;
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| AppError::io(parent.display(), e))?;
    }
    let (v, a) = snapshot_and_join(dir, snap, job_dir, &progress, cancel).await?;
    let second_half: ProgressFn = {
        let progress = progress.clone();
        std::sync::Arc::new(move |f| progress(0.5 + f * 0.5))
    };

    let (encoder, fell_back) = match plan {
        Plan::Copy => {
            let duration = f64::from(snap.chunks()) * seg_s;
            mux_copy(paths, &v, &a, title, &part, duration, second_half, cancel).await?;
            ("copy".to_owned(), false)
        }
        Plan::Encode { choice, fallback, opts, src, range } => {
            let job = EncodeJob { paths, opts, src: *src, range, title, v: &v, a: &a, part: &part };
            match encode(&job, choice, second_half.clone(), cancel).await {
                Ok(()) => (choice.encoder.clone(), false),
                Err(e @ AppError::Ffmpeg(_)) if choice.is_hardware() => {
                    let Some(sw) = fallback else { return Err(e) };
                    log::warn!("{} failed, retrying with {}: {e}", choice.encoder, sw.encoder);
                    let _ = tokio::fs::remove_file(&part).await;
                    second_half(0.0);
                    encode(&job, sw, second_half, cancel).await?;
                    (sw.encoder.clone(), true)
                }
                Err(e) => return Err(e),
            }
        }
    };

    tokio::fs::rename(&part, out).await.map_err(|e| AppError::io(out.display(), e))?;
    progress(1.0);
    let size_bytes = tokio::fs::metadata(out).await.map_err(|e| AppError::io(out.display(), e))?.len();
    Ok(Exported { size_bytes, encoder, fell_back })
}

/// Copies the chunks into `job_dir` and joins them into `v.mp4` and `a.mp4` (progress 0..0.5).
async fn snapshot_and_join(
    dir: &Path,
    snap: &Snap,
    job_dir: &Path,
    progress: &ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<(PathBuf, PathBuf)> {
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
    Ok((v, a))
}

/// Fast: mux with stream copy.
#[allow(clippy::too_many_arguments)]
async fn mux_copy(
    paths: &FfmpegPaths,
    v: &Path,
    a: &Path,
    title: &str,
    part: &Path,
    duration_s: f64,
    progress: ProgressFn,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let mut cmd = ffmpeg::ffmpeg_command(paths, true);
    cmd.arg("-i").arg(v).arg("-i").arg(a);
    cmd.args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy", "-tag:v", "hvc1", "-movflags", "+faststart"]);
    cmd.arg("-metadata").arg(format!("title={title}"));
    cmd.args(["-f", "mp4", "-y"]).arg(part).stdout(Stdio::null());
    let mut child = ffmpeg::spawn(&mut cmd)?;
    let stderr = ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"), Some((duration_s, progress)));
    let status = ffmpeg::wait_or_kill(&mut child, cancel).await?;
    ffmpeg::check_exit(status, stderr, "export").await
}

/// What both attempts of a precise encode share.
struct EncodeJob<'a> {
    paths: &'a FfmpegPaths,
    opts: &'a PreciseOpts,
    src: Option<(u32, u32)>,
    range: &'a PreciseRange,
    title: &'a str,
    v: &'a Path,
    a: &'a Path,
    part: &'a Path,
}

/// Precise: re-encode the exact range with `choice`.
async fn encode(job: &EncodeJob<'_>, choice: &EncoderChoice, progress: ProgressFn, cancel: &CancellationToken) -> AppResult<()> {
    let mut cmd = ffmpeg::ffmpeg_command(job.paths, true);
    cmd.args(encode_args(choice, job.opts, job.src, job.range, job.title, job.v, job.a, job.part)?).stdout(Stdio::null());
    let mut child = ffmpeg::spawn(&mut cmd)?;
    let stderr =
        ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"), Some((job.range.duration_s, progress)));
    let status = ffmpeg::wait_or_kill(&mut child, cancel).await?;
    ffmpeg::check_exit(status, stderr, &choice.encoder).await
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
        let done =
            export(&paths, &dir, &snap, 3.0, "test", &out, &job, &Plan::Copy, std::sync::Arc::new(|_| {}), &CancellationToken::new())
                .await
                .unwrap();
        eprintln!("exported {} bytes in {:?}", done.size_bytes, started.elapsed());
        assert_eq!((done.encoder.as_str(), done.fell_back), ("copy", false));
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

    fn snap(first: u32, last: u32, base: u32) -> Snap {
        let s = chunk_range(&inv(base, base + 100, vec![]), 3.0, f64::from(first - base) * 3.0, f64::from(last - base + 1) * 3.0, false, 0)
            .unwrap();
        assert_eq!((s.first, s.last), (first, last));
        s
    }

    #[test]
    fn precise_trim_offset() {
        // Session starts at chunk 3418, selection 10.5 → 40.2 s: chunks 3421..=3431 start at 9 s.
        let s = snap(3421, 3431, 3418);
        let r = precise_range(&s, 10.5, 40.2, 1.0 / 60.0);
        assert!((r.offset_s - 1.5).abs() < 1e-9);
        assert!((r.duration_s - 29.7).abs() < 1e-9);
        assert_eq!((r.actual_in_s, r.actual_out_s), (10.5, 40.2));
        // Reversed range is swapped.
        assert_eq!(precise_range(&s, 40.2, 10.5, 1.0 / 60.0), r);
    }

    #[test]
    fn precise_clamps_and_keeps_one_frame() {
        let s = snap(4, 6, 1); // 9 s .. 18 s
        let frame = 1.0 / 60.0;
        // Out point past a dropped live chunk is clamped to the chunks that are there.
        let r = precise_range(&s, 12.0, 25.0, frame);
        assert_eq!((r.actual_in_s, r.actual_out_s), (12.0, 18.0));
        // Empty selection becomes one frame.
        let r = precise_range(&s, 12.0, 12.0, frame);
        assert!((r.duration_s - frame).abs() < 1e-9 && r.offset_s == 3.0);
        // …also at the very end, where it grows backwards.
        let r = precise_range(&s, 18.0, 18.0, frame);
        assert!((r.duration_s - frame).abs() < 1e-9 && (r.actual_out_s - 18.0).abs() < 1e-9);
    }

    fn opts(max_height: Option<u32>, fps: Option<u32>, crop: bool) -> PreciseOpts {
        PreciseOpts { codec: VideoCodec::H264, max_height, fps, crop_16x9: crop, target: Quality::MaxSizeMb { mb: 10 } }
    }

    #[test]
    fn filter_chains() {
        let wide = Some((5120, 1440));
        assert_eq!(filter_chain(wide, &opts(None, None, true), "yuv420p"), "crop=2560:1440,scale=out_range=tv,format=yuv420p");
        assert_eq!(
            filter_chain(wide, &opts(Some(720), Some(60), true), "yuv420p"),
            "crop=2560:1440,scale=-2:720:out_range=tv,fps=60,format=yuv420p"
        );
        assert_eq!(filter_chain(wide, &opts(Some(1080), None, false), "nv12"), "scale=-2:1080:out_range=tv,format=nv12");
        // Never upscales; odd heights are rounded down to even.
        let hd = Some((1920, 1080));
        assert_eq!(filter_chain(hd, &opts(Some(1440), None, true), "yuv420p"), "scale=out_range=tv,format=yuv420p");
        assert_eq!(filter_chain(hd, &opts(Some(719), None, false), "yuv420p"), "scale=-2:718:out_range=tv,format=yuv420p");
        // Slightly wider than 16:9 (within 1 %): no crop. Crop widths are even.
        assert_eq!(filter_chain(Some((1930, 1080)), &opts(None, None, true), "yuv420p"), "scale=out_range=tv,format=yuv420p");
        assert!(filter_chain(Some((3440, 1441)), &opts(None, None, true), "yuv420p").starts_with("crop=2560:1441,"));
        // Unknown size: expressions.
        assert_eq!(
            filter_chain(None, &opts(Some(720), None, true), "yuv420p"),
            "crop='min(iw,trunc(ih*16/9/2)*2)':ih,scale=-2:'min(ih,720)':out_range=tv,format=yuv420p"
        );
    }

    #[test]
    fn encode_command() {
        let choice = EncoderChoice { encoder: "hevc_nvenc".into(), hwaccel: Some("cuda") };
        let mut o = opts(None, None, false);
        o.codec = VideoCodec::Hevc;
        o.target = Quality::Crf { q: 22 };
        let r = PreciseRange { offset_s: 1.5, duration_s: 2.7, actual_in_s: 10.5, actual_out_s: 13.2 };
        let a = encode_args(&choice, &o, Some((2560, 1440)), &r, "T", Path::new("v.mp4"), Path::new("a.mp4"), Path::new("o.mp4"))
            .unwrap();
        let j = a.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");
        assert!(j.starts_with("-hwaccel cuda -i v.mp4 -i a.mp4 -ss 1.500000 -t 2.700000 -map 0:v:0 -map 1:a:0"), "{j}");
        assert!(j.contains("-vf scale=out_range=tv,format=yuv420p -c:v hevc_nvenc -rc vbr -cq 22 -b:v 0 -preset p5 -tag:v hvc1"), "{j}");
        assert!(j.ends_with("-c:a aac -b:a 160k -movflags +faststart -metadata title=T -f mp4 -y o.mp4"), "{j}");
        // Too little bitrate fails before ffmpeg runs.
        o.target = Quality::MaxSizeMb { mb: 1 };
        let long = PreciseRange { duration_s: 600.0, ..r };
        assert!(encode_args(&choice, &o, None, &long, "T", Path::new("v"), Path::new("a"), Path::new("o")).is_err());
        let vaapi = EncoderChoice { encoder: "h264_vaapi".into(), hwaccel: Some("vaapi") };
        let j = encode_args(&vaapi, &opts(None, None, false), None, &r, "T", Path::new("v"), Path::new("a"), Path::new("o")).unwrap();
        assert_eq!(j[..4], ["-vaapi_device", "/dev/dri/renderD128", "-hwaccel", "vaapi"].map(OsString::from));
    }

    #[test]
    fn mode_serde() {
        let fast: ExportMode = serde_json::from_str(r#"{"kind":"fast"}"#).unwrap();
        assert_eq!(fast, ExportMode::Fast);
        let json = r#"{"kind":"precise","codec":"h264","maxHeight":720,"fps":60,"crop16x9":false,"target":{"kind":"maxSizeMb","mb":10}}"#;
        let precise: ExportMode = serde_json::from_str(json).unwrap();
        assert_eq!(precise, ExportMode::Precise(opts(Some(720), Some(60), false)));
        assert_eq!(serde_json::to_string(&precise).unwrap(), json);
        let prefs: ExportPrefs = serde_json::from_str(r#"{"mode":"precise","precise":null}"#).unwrap();
        assert_eq!(prefs.mode, Some(ExportModeKind::Precise));
    }

    #[test]
    fn recent_exports_newest_first_trimmed() {
        let entry = |i: u32| RecentExport {
            path: PathBuf::from(format!("clip{i}.mp4")),
            game_name: "Game".into(),
            in_s: 0.0,
            out_s: 1.0,
            mode: ExportModeKind::Fast,
            size_bytes: 1,
            finished_at: String::new(),
            fireshare_url: None,
        };
        let mut list = Vec::new();
        for i in 0..25 {
            list = push_recent(list, entry(i));
        }
        assert_eq!(list.len(), RECENT_EXPORTS);
        assert_eq!(list[0], entry(24));
        assert_eq!(list[19], entry(5));
        // Exporting to the same file again moves it to the front instead of duplicating it.
        let list = push_recent(list, entry(10));
        assert_eq!(list.len(), RECENT_EXPORTS);
        assert_eq!(list[0], entry(10));
        assert_eq!(list.iter().filter(|e| e.path == entry(10).path).count(), 1);
    }

    /// Needs ffmpeg/ffprobe on PATH and a real bg folder with at least 3 chunks:
    /// `STEAMREPLAY_SESSION_DIR=<…/video/bg_…> cargo test precise_export_session -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn precise_export_session() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let dir = PathBuf::from(dir);
        let (inv, size) = crate::steam::recordings::inventory(&dir).unwrap();
        let paths = ffmpeg::locate(None).unwrap();
        let cancel = CancellationToken::new();
        let caps = encoders::Capabilities::probe(&paths, &cancel).await;
        let mpd = std::fs::read_to_string(dir.join("session.mpd")).ok().and_then(|x| crate::steam::mpd::parse_mpd(&x).ok());
        let src = mpd.as_ref().and_then(|m| m.video.width.zip(m.video.height));
        let (in_s, out_s) = (1.5, 4.2);
        let snap = chunk_range(&inv, 3.0, in_s, out_s, false, size).unwrap();
        let o = opts(Some(720), None, false);
        let range = precise_range(&snap, in_s, out_s, 1.0 / 60.0);
        let mut choices = vec![encoders::software(VideoCodec::H264, &caps).unwrap()];
        if let Some(hw) = encoders::choose(crate::media::preview::Backend::Cuda, VideoCodec::H264, &caps).filter(|c| c.is_hardware()) {
            choices.push(hw);
        }
        // A hardware encoder that fails must fall back to software.
        choices.push(EncoderChoice { encoder: "h264_broken_nvenc".into(), hwaccel: None });
        for choice in choices {
            let tmp = tempfile::tempdir().unwrap();
            let out = tmp.path().join("out.mp4");
            let job = tmp.path().join("job");
            let plan = Plan::Encode { choice: choice.clone(), fallback: encoders::software(VideoCodec::H264, &caps), opts: o, src, range };
            let started = std::time::Instant::now();
            let done = export(&paths, &dir, &snap, 3.0, "test", &out, &job, &plan, std::sync::Arc::new(|_| {}), &cancel)
                .await
                .unwrap();
            eprintln!("{}: {} bytes in {:?}, fell back: {}", done.encoder, done.size_bytes, started.elapsed(), done.fell_back);
            assert_eq!(done.fell_back, choice.encoder == "h264_broken_nvenc");
            assert!(!job.exists());
            assert!(!part_path(&out).exists());

            let mut cmd = ffmpeg::ffprobe_command(&paths);
            cmd.args(["-show_entries", "stream=codec_name,height,color_range:format=duration", "-of", "csv=p=0"]).arg(&out);
            let text = String::from_utf8(ffmpeg::run(cmd, None, None, &cancel).await.unwrap()).unwrap();
            eprintln!("{text}");
            let mut lines = text.lines();
            assert_eq!(lines.next().unwrap(), "h264,720,tv");
            assert!(lines.next().unwrap().starts_with("aac"));
            let duration: f64 = lines.next().unwrap().trim().parse().unwrap();
            assert!((duration - (out_s - in_s)).abs() <= 1.0 / 60.0 + 0.03, "duration {duration}");
        }
    }
}
