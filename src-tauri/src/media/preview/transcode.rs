//! One preview segment: Steam's HEVC chunks → 720p H.264 fragmented MP4, with the original AAC
//! copied. Timestamps are kept (`-copyts`) and shifted to video time, so the player can append
//! segments to one `SourceBuffer` without offset math, and the audio keeps its small lead-in.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use super::SegmentPlan;
use crate::error::{AppError, AppResult};
use crate::media::encoders::Capabilities;
use crate::media::chunks::{AUDIO_INIT, VIDEO_INIT, audio_chunk, video_chunk};
use crate::media::ffmpeg::{self, FfmpegPaths};

/// How a segment is decoded, scaled and encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    /// NVDEC + `scale_cuda` + NVENC (NVIDIA, Windows and Linux).
    Cuda,
    /// D3D11VA decode + AMF encode (AMD on Windows). Not tested on real hardware.
    D3d11vaAmf,
    /// D3D11VA decode + Quick Sync encode (Intel on Windows). Not tested on real hardware.
    D3d11vaQsv,
    /// VAAPI decode, scale and encode (AMD/Intel on Linux).
    Vaapi,
    /// Quick Sync decode and encode.
    Qsv,
    /// libx264. Works everywhere, but needs many CPU cores.
    Software,
}

impl Backend {
    pub fn is_hardware(self) -> bool {
        self != Self::Software
    }

    /// Candidates in the order they are tried.
    pub fn candidates() -> &'static [Backend] {
        if cfg!(windows) {
            &[Self::Cuda, Self::D3d11vaAmf, Self::D3d11vaQsv, Self::Software]
        } else {
            &[Self::Cuda, Self::Vaapi, Self::Qsv, Self::Software]
        }
    }

    /// `(hwaccel, encoder)` that `ffmpeg -hwaccels` / `-encoders` must list.
    fn needs(self) -> (Option<&'static str>, &'static str) {
        match self {
            Self::Cuda => (Some("cuda"), "h264_nvenc"),
            Self::D3d11vaAmf => (Some("d3d11va"), "h264_amf"),
            Self::D3d11vaQsv => (Some("d3d11va"), "h264_qsv"),
            Self::Vaapi => (Some("vaapi"), "h264_vaapi"),
            Self::Qsv => (Some("qsv"), "h264_qsv"),
            Self::Software => (None, "libx264"),
        }
    }

    /// Options before `-i`.
    fn input_args(self) -> &'static [&'static str] {
        match self {
            Self::Cuda => &["-hwaccel", "cuda", "-hwaccel_output_format", "cuda"],
            Self::D3d11vaAmf | Self::D3d11vaQsv => &["-hwaccel", "d3d11va"],
            Self::Vaapi => &["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi"],
            Self::Qsv => &["-hwaccel", "qsv"],
            Self::Software => &[],
        }
    }

    /// Filter and encoder options. Every path ends in 8-bit 4:2:0 (Steam writes full-range
    /// `yuvj420p`, which some encoders refuse).
    fn encode_args(self) -> &'static [&'static str] {
        match self {
            Self::Cuda => &["-vf", "scale_cuda=-2:720:format=yuv420p", "-c:v", "h264_nvenc", "-preset", "p2", "-cq", "23"],
            Self::D3d11vaAmf => &["-vf", "scale=-2:720,format=yuv420p", "-c:v", "h264_amf", "-quality", "speed", "-b:v", "6M"],
            Self::D3d11vaQsv | Self::Qsv => {
                &["-vf", "scale=-2:720,format=nv12", "-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", "23"]
            }
            Self::Vaapi => &["-vf", "scale_vaapi=w=-2:h=720:format=nv12", "-c:v", "h264_vaapi", "-qp", "22"],
            Self::Software => &["-vf", "scale=-2:720,format=yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22"],
        }
    }
}

/// The ffmpeg arguments for one segment. Video comes from stdin, audio from `audio`.
/// `v0` is the start time of the session's first chunk; subtracting it gives video time.
pub fn args(backend: Backend, audio: Option<&Path>, v0: f64, out: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-copyts".into()];
    a.extend(backend.input_args().iter().map(OsString::from));
    a.extend(["-i", "pipe:0"].map(OsString::from));
    if let Some(audio) = audio {
        a.extend(["-i".into(), audio.as_os_str().to_owned()]);
        a.extend(["-map", "0:v:0", "-map", "1:a:0"].map(OsString::from));
    } else {
        a.extend(["-map", "0:v:0"].map(OsString::from));
    }
    a.extend(backend.encode_args().iter().map(OsString::from));
    // One keyframe per second at 60 fps, no B-frames: cheap seeking and a fixed codec string
    // (`avc1.640033`, see `MIME`). Level 5.1 covers 2560×720 at 60 fps (5120×1440 sources).
    a.extend(["-g", "60", "-bf", "0", "-profile:v", "high", "-level", "5.1"].map(OsString::from));
    if audio.is_some() {
        // The original AAC lines up with the chunks; re-encoding added priming samples at
        // every segment start.
        a.extend(["-c:a", "copy"].map(OsString::from));
    }
    a.extend(["-output_ts_offset".into(), format!("{}", -v0).into()]);
    // `frag_discont` + `avoid_negative_ts disabled` keep the shifted timestamps in the
    // fragments; without them every segment would start at 0.
    a.extend(
        [
            "-avoid_negative_ts",
            "disabled",
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof+frag_discont",
            "-f",
            "mp4",
            "-y",
        ]
        .map(OsString::from),
    );
    a.push(out.as_os_str().to_owned());
    a
}

/// Unique names for temp files of concurrent jobs.
fn temp_name(suffix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}-{suffix}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Deletes the listed files when dropped, so errors and cancels leave nothing behind.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// What every segment of a session shares.
pub struct Source<'a> {
    pub paths: &'a FfmpegPaths,
    /// The bg folder with the chunks.
    pub dir: &'a Path,
    /// Start time of the first chunk on disk; subtracted to get video time.
    pub v0: f64,
    pub tmp_dir: &'a Path,
}

/// Transcodes `plan` into `out`. Writes `<out>.part` first and renames it when ffmpeg succeeded.
pub async fn transcode(
    src: &Source<'_>,
    backend: Backend,
    plan: &SegmentPlan,
    out: &Path,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let Source { paths, dir, v0, tmp_dir } = *src;
    if plan.chunks.is_empty() {
        return Err(AppError::NotFound(format!("segment {} has no chunks on disk", plan.index)));
    }
    tokio::fs::create_dir_all(tmp_dir).await.map_err(|e| AppError::io(tmp_dir.display(), e))?;
    if let Some(parent) = out.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| AppError::io(parent.display(), e))?;
    }
    let part = PathBuf::from(format!("{}.part", out.display()));
    let audio_path = tmp_dir.join(temp_name("a.mp4"));
    let _cleanup = Cleanup(vec![part.clone(), audio_path.clone()]);

    // Audio: init + the chunks that exist. Small (about 50 KB per chunk).
    let audio_chunks: Vec<String> =
        plan.chunks.iter().map(|&n| audio_chunk(n)).filter(|name| dir.join(name).is_file()).collect();
    let audio = if audio_chunks.is_empty() {
        None
    } else {
        let mut names = vec![AUDIO_INIT.to_owned()];
        names.extend(audio_chunks);
        let data = crate::media::chunks::read_files(dir, &names).await?;
        tokio::fs::write(&audio_path, data).await.map_err(|e| AppError::io(audio_path.display(), e))?;
        Some(audio_path.as_path())
    };

    let mut cmd = ffmpeg::ffmpeg_command(paths, false);
    cmd.args(args(backend, audio, v0, &part)).stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = ffmpeg::spawn(&mut cmd)?;

    // Video: init + chunks streamed into stdin (about 8 MB per chunk).
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let files: Vec<PathBuf> = std::iter::once(dir.join(VIDEO_INIT))
        .chain(plan.chunks.iter().map(|&n| dir.join(video_chunk(n))))
        .collect();
    let writer = tokio::spawn(async move {
        for f in files {
            let data = tokio::fs::read(&f).await.map_err(|e| AppError::io(f.display(), e))?;
            if stdin.write_all(&data).await.is_err() {
                break; // ffmpeg exited; its stderr tells why
            }
        }
        Ok::<_, AppError>(())
    });
    let stderr = ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"), None);

    let status = ffmpeg::wait_or_kill(&mut child, cancel).await;
    let write_result = writer.await.map_err(|e| AppError::Io(format!("writer task: {e}")))?;
    ffmpeg::check_exit(status?, stderr, "preview transcode").await?;
    write_result?;
    tokio::fs::rename(&part, out).await.map_err(|e| AppError::io(out.display(), e))?;
    Ok(())
}

/// Backends whose hwaccel and encoder this ffmpeg build has, in the order to try them.
pub fn available(caps: &Capabilities) -> Vec<Backend> {
    Backend::candidates()
        .iter()
        .copied()
        .filter(|b| {
            let (hw, enc) = b.needs();
            caps.has_encoder(enc) && hw.is_none_or(|h| caps.has_hwaccel(h))
        })
        .collect()
}

/// Tries a 1-chunk transcode with each backend and returns the first that works.
/// Falls back to software when nothing else does (it is tried last anyway).
pub async fn detect(
    paths: &FfmpegPaths,
    caps: &Capabilities,
    dir: &Path,
    first_chunk: u32,
    tmp_dir: &Path,
    cancel: &CancellationToken,
) -> Backend {
    let plan = SegmentPlan { index: 0, chunks: vec![first_chunk], partial_last: Some(first_chunk) };
    for backend in available(caps) {
        let out = tmp_dir.join(temp_name("probe.mp4"));
        let started = std::time::Instant::now();
        let attempt = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            transcode(&Source { paths, dir, v0: 0.0, tmp_dir }, backend, &plan, &out, cancel),
        )
        .await;
        let _ = tokio::fs::remove_file(&out).await;
        match attempt {
            Ok(Ok(())) => {
                log::info!("preview backend {backend:?} works ({:?} for 1 chunk)", started.elapsed());
                return backend;
            }
            Ok(Err(AppError::Cancelled)) => break,
            Ok(Err(e)) => log::info!("preview backend {backend:?} failed: {e}"),
            Err(_) => log::info!("preview backend {backend:?} timed out"),
        }
    }
    Backend::Software
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(a: &[OsString]) -> String {
        a.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn args_per_backend() {
        let out = Path::new("out.mp4");
        let audio = Path::new("a.mp4");
        let cuda = joined(&args(Backend::Cuda, Some(audio), 10251.0025, out));
        assert!(cuda.starts_with("-copyts -hwaccel cuda -hwaccel_output_format cuda -i pipe:0 -i a.mp4 -map 0:v:0 -map 1:a:0"));
        assert!(cuda.contains("scale_cuda=-2:720:format=yuv420p -c:v h264_nvenc"));
        assert!(cuda.contains("-c:a copy -output_ts_offset -10251.0025 "));
        assert!(cuda.ends_with("+frag_keyframe+empty_moov+default_base_moof+frag_discont -f mp4 -y out.mp4"));

        let sw = joined(&args(Backend::Software, None, 0.0, out));
        assert!(sw.starts_with("-copyts -i pipe:0 -map 0:v:0 -vf scale=-2:720,format=yuv420p -c:v libx264"));
        assert!(!sw.contains("-c:a"));
        assert!(sw.contains("-profile:v high -level 5.1"));

        for b in Backend::candidates() {
            let a = joined(&args(*b, Some(audio), 0.0, out));
            assert!(a.contains(b.needs().1), "{b:?}: {a}");
        }
    }

    #[test]
    fn available_backends() {
        let caps = Capabilities::parse(
            " V....D libx264  H.264\n V....D h264_nvenc  NVENC\n V....D h264_vaapi  VAAPI\n",
            "Hardware acceleration methods:\ncuda\n",
        );
        // VAAPI encoder without the vaapi hwaccel is not usable for the preview.
        assert_eq!(available(&caps), [Backend::Cuda, Backend::Software]);
        assert_eq!(available(&Capabilities::default()), []);
    }

    /// Needs ffmpeg/ffprobe on PATH and a real bg folder with at least 2 chunks, e.g. the saved
    /// clip: `STEAMREPLAY_SESSION_DIR=<…/clips/clip_1478500_…/video/bg_…> cargo test preview_fixture -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn preview_fixture() {
        let Ok(dir) = std::env::var("STEAMREPLAY_SESSION_DIR") else {
            eprintln!("STEAMREPLAY_SESSION_DIR not set, skipping");
            return;
        };
        let dir = PathBuf::from(dir);
        let (inv, _) = crate::steam::recordings::inventory(&dir).unwrap();
        let paths = ffmpeg::locate(None).unwrap();
        let cancel = CancellationToken::new();
        let v0 = crate::media::chunks::probe_start(&paths, &dir, VIDEO_INIT, &video_chunk(inv.first), &cancel)
            .await
            .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let plan = super::super::segment_plan(&inv, 0).unwrap();
        let mut backends = vec![Backend::Software];
        if available(&Capabilities::probe(&paths, &cancel).await).contains(&Backend::Cuda) {
            backends.push(Backend::Cuda);
        }
        for backend in backends {
            for shift in [0.0, 12.0] {
                let out = tmp.path().join(format!("{backend:?}-{shift}.mp4"));
                let started = std::time::Instant::now();
                transcode(&Source { paths: &paths, dir: &dir, v0: v0 - shift, tmp_dir: tmp.path() }, backend, &plan, &out, &cancel).await.unwrap();
                eprintln!("{backend:?}: {:?}", started.elapsed());
                let mut cmd = ffmpeg::ffprobe_command(&paths);
                cmd.args(["-show_entries", "stream=codec_name,profile,level,height,start_time", "-of", "csv=p=0"]).arg(&out);
                let probe = String::from_utf8(ffmpeg::run(cmd, None, None, &cancel).await.unwrap()).unwrap();
                eprintln!("{probe}");
                let mut lines = probe.lines();
                let video: Vec<&str> = lines.next().unwrap().split(',').collect();
                assert_eq!(&video[..4], ["h264", "High", "720", "51"]); // ffprobe orders height before level
                let start: f64 = video[4].parse().unwrap();
                assert!((start - shift).abs() < 0.05, "start {start}, expected {shift}");
                assert!(lines.next().unwrap().starts_with("aac"));
                // The first packet must carry the shifted time, not just the stream header.
                let mut cmd = ffmpeg::ffprobe_command(&paths);
                cmd.args(["-select_streams", "v:0", "-show_entries", "packet=pts_time", "-of", "csv=p=0"]).arg(&out);
                let pts = String::from_utf8(ffmpeg::run(cmd, None, None, &cancel).await.unwrap()).unwrap();
                let times: Vec<f64> = pts.lines().filter_map(|l| l.trim().parse().ok()).collect();
                let first = times.iter().copied().fold(f64::INFINITY, f64::min);
                let last = times.iter().copied().fold(0.0, f64::max);
                assert!((first - shift).abs() < 0.05, "first packet {first}");
                let expected = plan.chunks.len() as f64 * 3.0;
                assert!((last - first - expected).abs() < 0.1, "covers {} s, expected ≈ {expected}", last - first);
            }
        }
    }
}
