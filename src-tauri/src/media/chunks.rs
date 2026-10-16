//! Steam's DASH file names and helpers to read them. Used by audio analysis and the preview.

use std::path::Path;

use tokio_util::sync::CancellationToken;

use super::ffmpeg::{self, FfmpegPaths};
use crate::error::{AppError, AppResult};

pub const AUDIO_INIT: &str = "init-stream1.m4s";
pub const VIDEO_INIT: &str = "init-stream0.m4s";

pub fn audio_chunk(n: u32) -> String {
    format!("chunk-stream1-{n:05}.m4s")
}

pub fn video_chunk(n: u32) -> String {
    format!("chunk-stream0-{n:05}.m4s")
}

/// The files concatenated, in order.
pub async fn read_files(dir: &Path, names: &[String]) -> AppResult<Vec<u8>> {
    let mut out = Vec::new();
    for name in names {
        let p = dir.join(name);
        out.extend(tokio::fs::read(&p).await.map_err(|e| AppError::io(p.display(), e))?);
    }
    Ok(out)
}

/// `start_time` of the single stream in `init + chunk`, in seconds. Steam's chunks carry the
/// time since the recording started, so for chunk `n` this is about `(n − 1) · 3 s`.
pub async fn probe_start(
    paths: &FfmpegPaths,
    dir: &Path,
    init: &str,
    chunk: &str,
    cancel: &CancellationToken,
) -> AppResult<f64> {
    let data = read_files(dir, &[init.to_owned(), chunk.to_owned()]).await?;
    let mut cmd = ffmpeg::ffprobe_command(paths);
    cmd.args(["-show_entries", "stream=start_time", "-of", "csv=p=0", "-i", "pipe:0"]);
    let out = ffmpeg::run(cmd, Some(data), None, cancel).await?;
    String::from_utf8_lossy(&out)
        .lines()
        .find_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok())
        .ok_or_else(|| AppError::Ffmpeg(format!("no start time for {chunk}")))
}
