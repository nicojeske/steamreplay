//! Finding ffmpeg/ffprobe and running them as cancellable child processes.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;

use serde::Serialize;
use specta::Type;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};

/// `CREATE_NO_WINDOW`: no console window flashes up for child processes on Windows.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FfmpegSource {
    Settings,
    Bundled,
    Path,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// First line of `ffmpeg -version`.
    pub version: String,
    pub source: FfmpegSource,
}

pub type ProgressFn = Arc<dyn Fn(f64) + Send + Sync>;

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn std_command(program: &Path) -> std::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn version_of(ffmpeg: &Path) -> AppResult<String> {
    let out = std_command(ffmpeg)
        .args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| AppError::Ffmpeg(format!("{}: {e}", ffmpeg.display())))?;
    if !out.status.success() {
        return Err(AppError::Ffmpeg(format!("{} -version failed", ffmpeg.display())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().trim().to_owned())
}

/// Order: settings override → bundled sidecar next to the executable → `PATH`.
pub fn locate(override_dir: Option<&Path>) -> AppResult<FfmpegPaths> {
    let mut candidates: Vec<(FfmpegSource, PathBuf, PathBuf)> = Vec::new();
    if let Some(dir) = override_dir {
        candidates.push((FfmpegSource::Settings, dir.join(exe("ffmpeg")), dir.join(exe("ffprobe"))));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push((FfmpegSource::Bundled, dir.join(exe("ffmpeg")), dir.join(exe("ffprobe"))));
    }
    if let (Ok(f), Ok(p)) = (which::which("ffmpeg"), which::which("ffprobe")) {
        candidates.push((FfmpegSource::Path, f, p));
    }
    let mut tried = Vec::new();
    for (source, ffmpeg, ffprobe) in candidates {
        if !ffmpeg.is_file() || !ffprobe.is_file() {
            tried.push(ffmpeg.display().to_string());
            continue;
        }
        match version_of(&ffmpeg) {
            Ok(version) => return Ok(FfmpegPaths { ffmpeg, ffprobe, version, source }),
            Err(e) => {
                log::warn!("{e}");
                tried.push(ffmpeg.display().to_string());
            }
        }
    }
    Err(AppError::Ffmpeg(format!(
        "ffmpeg not found (tried: {}). Install ffmpeg or choose its folder in Settings.",
        if tried.is_empty() { "PATH".to_owned() } else { tried.join(", ") }
    )))
}

fn command(program: &Path) -> Command {
    let mut c = Command::new(program);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// `ffmpeg -hide_banner -nostdin -v error`, plus `-progress pipe:2` when `progress` is set
/// (parsed by [`run`]).
pub fn ffmpeg_command(paths: &FfmpegPaths, progress: bool) -> Command {
    let mut c = command(&paths.ffmpeg);
    c.args(["-hide_banner", "-nostdin", "-v", "error"]);
    if progress {
        c.args(["-progress", "pipe:2", "-stats_period", "0.5"]);
    }
    c
}

pub fn ffprobe_command(paths: &FfmpegPaths) -> Command {
    let mut c = command(&paths.ffprobe);
    c.args(["-hide_banner", "-v", "error"]);
    c
}

pub fn spawn(cmd: &mut Command) -> AppResult<Child> {
    cmd.spawn().map_err(|e| AppError::Ffmpeg(format!("could not start {:?}: {e}", cmd.as_std().get_program())))
}

/// Keys that ffmpeg writes with `-progress`. Those lines are not error output.
const PROGRESS_KEYS: &[&str] = &[
    "frame", "fps", "stream_", "bitrate", "total_size", "out_time_us", "out_time_ms", "out_time", "dup_frames",
    "drop_frames", "speed", "progress",
];

/// `out_time_us=12345` → `Some(Some(12345))`; another progress line → `Some(None)`;
/// anything else (an error message) → `None`.
pub fn parse_progress_line(line: &str) -> Option<Option<i64>> {
    let (key, value) = line.split_once('=')?;
    if !PROGRESS_KEYS.iter().any(|k| key.starts_with(k)) || key.contains(' ') {
        return None;
    }
    Some(if key == "out_time_us" { value.trim().parse().ok() } else { None })
}

/// Collects stderr. Progress lines are turned into fractions of `duration_s`, the rest is kept
/// for the error message.
pub fn collect_stderr(stderr: ChildStderr, progress: Option<(f64, ProgressFn)>) -> JoinHandle<String> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            match parse_progress_line(&line) {
                Some(Some(us)) => {
                    if let Some((duration_s, f)) = &progress
                        && *duration_s > 0.0
                    {
                        f((us as f64 / 1e6 / duration_s).clamp(0.0, 1.0));
                    }
                }
                Some(None) => {}
                None => {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
        }
        text
    })
}

/// Waits for the child unless `cancel` fires first. On cancel the process is killed and reaped,
/// so no zombie is left behind.
pub async fn wait_or_kill(child: &mut Child, cancel: &CancellationToken) -> AppResult<ExitStatus> {
    let status = tokio::select! {
        s = child.wait() => Some(s),
        () = cancel.cancelled() => None,
    };
    match status {
        Some(s) => s.map_err(|e| AppError::Ffmpeg(format!("waiting for ffmpeg: {e}"))),
        None => {
            let _ = child.kill().await;
            Err(AppError::Cancelled)
        }
    }
}

/// Turns a failed exit status into an error carrying ffmpeg's stderr.
pub async fn check_exit(status: ExitStatus, stderr: JoinHandle<String>, what: &str) -> AppResult<()> {
    if status.success() {
        return Ok(());
    }
    let text = stderr.await.unwrap_or_default();
    let text = text.trim();
    Err(AppError::Ffmpeg(if text.is_empty() { format!("{what} failed ({status})") } else { format!("{what}: {text}") }))
}

/// Runs a prepared command to completion and returns its stdout.
/// `stdin` is written to the process (write errors are ignored: probes may stop reading early).
/// `progress` is `(duration_s, callback)` for commands made with `ffmpeg_command(_, true)`.
pub async fn run(
    mut cmd: Command,
    stdin: Option<Vec<u8>>,
    progress: Option<(f64, ProgressFn)>,
    cancel: &CancellationToken,
) -> AppResult<Vec<u8>> {
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = spawn(&mut cmd)?;
    if let (Some(data), Some(mut w)) = (stdin, child.stdin.take()) {
        tokio::spawn(async move {
            let _ = w.write_all(&data).await;
        });
    }
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });
    let stderr = collect_stderr(child.stderr.take().expect("stderr is piped"), progress);
    let status = wait_or_kill(&mut child, cancel).await?;
    let out = reader
        .await
        .map_err(|e| AppError::Ffmpeg(format!("reading output: {e}")))?
        .map_err(|e| AppError::Ffmpeg(format!("reading output: {e}")))?;
    check_exit(status, stderr, "ffmpeg").await?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_us=1500000"), Some(Some(1_500_000)));
        assert_eq!(parse_progress_line("out_time_us=N/A"), Some(None));
        assert_eq!(parse_progress_line("progress=continue"), Some(None));
        assert_eq!(parse_progress_line("stream_0_0_q=-1.0"), Some(None));
        assert_eq!(parse_progress_line("pipe:0: Invalid data found when processing input"), None);
        assert_eq!(parse_progress_line("Error opening output file=x"), None);
    }
}
