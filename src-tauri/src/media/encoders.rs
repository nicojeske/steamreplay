//! What this ffmpeg build can encode, which encoder a precise export uses, and its rate control.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use specta::Type;
use tokio_util::sync::CancellationToken;

use super::ffmpeg::{self, FfmpegPaths};
use super::preview::Backend;
use crate::error::{AppError, AppResult};

/// Audio bitrate of a precise export, in kbit/s.
pub const AUDIO_KBPS: u32 = 160;
/// Below this video bitrate a size-limited export looks too bad to be useful.
pub const MIN_VIDEO_KBPS: u32 = 300;

/// Encoders and hwaccels compiled into an ffmpeg build. Whether the GPU supports them is only
/// known when an encode runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub encoders: HashSet<String>,
    pub hwaccels: HashSet<String>,
}

impl Capabilities {
    /// Parses `ffmpeg -encoders` and `ffmpeg -hwaccels` output.
    pub fn parse(encoders: &str, hwaccels: &str) -> Self {
        Self {
            // " V....D libx264    libx264 H.264 …": the name is the second column.
            encoders: encoders.lines().filter_map(|l| l.split_whitespace().nth(1)).map(str::to_owned).collect(),
            // "Hardware acceleration methods:" followed by one name per line.
            hwaccels: hwaccels
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.ends_with(':'))
                .map(str::to_owned)
                .collect(),
        }
    }

    pub async fn probe(paths: &FfmpegPaths, cancel: &CancellationToken) -> Self {
        Self::parse(&list(paths, "encoders", cancel).await, &list(paths, "hwaccels", cancel).await)
    }

    pub fn has_encoder(&self, name: &str) -> bool {
        self.encoders.contains(name)
    }

    pub fn has_hwaccel(&self, name: &str) -> bool {
        self.hwaccels.contains(name)
    }
}

/// Lines of `ffmpeg -<what>` (`encoders`, `hwaccels`).
async fn list(paths: &FfmpegPaths, what: &str, cancel: &CancellationToken) -> String {
    let mut cmd = ffmpeg::ffmpeg_command(paths, false);
    cmd.arg(format!("-{what}"));
    match ffmpeg::run(cmd, None, None, cancel).await {
        Ok(out) => String::from_utf8_lossy(&out).into_owned(),
        Err(e) => {
            log::warn!("ffmpeg -{what}: {e}");
            String::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 3] = [Self::H264, Self::Hevc, Self::Av1];

    fn prefix(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Av1 => "av1",
        }
    }

    fn software(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::Hevc => "libx265",
            Self::Av1 => "libsvtav1",
        }
    }
}

/// Size or quality target of a precise export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Quality {
    /// Constant quality; lower is better. Mapped to each encoder's own scale.
    Crf { q: u8 },
    /// Average bitrate chosen so the file stays below `mb` megabytes.
    MaxSizeMb { mb: u32 },
}

/// The encoder a precise export runs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderChoice {
    /// ffmpeg encoder name, e.g. `h264_nvenc`.
    pub encoder: String,
    /// `-hwaccel` for decoding. Frames are downloaded to system memory, so one CPU filter chain
    /// works for every backend.
    pub hwaccel: Option<&'static str>,
}

impl EncoderChoice {
    pub fn is_hardware(&self) -> bool {
        !self.encoder.starts_with("lib")
    }
}

/// `(hwaccel, encoder suffix)` of the preview backend's GPU family.
fn family(backend: Backend) -> Option<(&'static str, &'static str)> {
    match backend {
        Backend::Cuda => Some(("cuda", "nvenc")),
        Backend::D3d11vaAmf => Some(("d3d11va", "amf")),
        Backend::D3d11vaQsv => Some(("d3d11va", "qsv")),
        Backend::Qsv => Some(("qsv", "qsv")),
        Backend::Vaapi => Some(("vaapi", "vaapi")),
        Backend::Software => None,
    }
}

/// The software encoder for `codec`, if this build has it.
pub fn software(codec: VideoCodec, caps: &Capabilities) -> Option<EncoderChoice> {
    let encoder = codec.software();
    caps.has_encoder(encoder).then(|| EncoderChoice { encoder: encoder.to_owned(), hwaccel: None })
}

/// The hardware encoder of the backend's GPU family, else the software one, else `None` (the
/// codec is not offered).
pub fn choose(backend: Backend, codec: VideoCodec, caps: &Capabilities) -> Option<EncoderChoice> {
    if let Some((hwaccel, suffix)) = family(backend) {
        let encoder = format!("{}_{suffix}", codec.prefix());
        if caps.has_encoder(&encoder) {
            return Some(EncoderChoice { encoder, hwaccel: caps.has_hwaccel(hwaccel).then_some(hwaccel) });
        }
    }
    software(codec, caps)
}

/// Video bitrate in kbit/s that keeps `mb` megabytes for `duration_s`, with 5 % headroom for the
/// container and the audio track subtracted.
pub fn size_kbps(mb: u32, duration_s: f64) -> f64 {
    f64::from(mb) * 8.0 * 1000.0 * 0.95 / duration_s.max(0.001) - f64::from(AUDIO_KBPS)
}

/// Rate control and preset options for `encoder`.
pub fn rate_args(encoder: &str, target: Quality, duration_s: f64) -> AppResult<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let nvenc = encoder.ends_with("_nvenc");
    let mut a = match target {
        Quality::MaxSizeMb { mb } => {
            let kbps = size_kbps(mb, duration_s);
            if kbps < f64::from(MIN_VIDEO_KBPS) {
                return Err(AppError::Ffmpeg(format!(
                    "the selection is too long for {mb} MB ({:.0} kbit/s video); shorten it or lower the resolution",
                    kbps.max(0.0)
                )));
            }
            let v = kbps.floor() as u64;
            let mut a = if nvenc { s(&["-rc", "vbr"]) } else { Vec::new() };
            a.extend(["-b:v".into(), format!("{v}k"), "-maxrate".into(), format!("{v}k"), "-bufsize".into(), format!("{}k", 2 * v)]);
            a
        }
        Quality::Crf { q } => {
            let q = q.to_string();
            if nvenc {
                s(&["-rc", "vbr", "-cq", &q, "-b:v", "0"])
            } else if encoder.ends_with("_qsv") {
                s(&["-global_quality", &q])
            } else if encoder.ends_with("_vaapi") {
                s(&["-qp", &q])
            } else if encoder.ends_with("_amf") {
                s(&["-rc", "cqp", "-qp_i", &q, "-qp_p", &q])
            } else {
                s(&["-crf", &q])
            }
        }
    };
    match encoder {
        e if e.ends_with("_nvenc") => a.extend(s(&["-preset", "p5"])),
        "libx264" => a.extend(s(&["-preset", "medium"])),
        "libx265" => a.extend(s(&["-preset", "fast", "-x265-params", "log-level=error"])),
        "libsvtav1" => a.extend(s(&["-preset", "8"])),
        _ => {}
    }
    Ok(a)
}

/// The pixel format filter at the end of the chain. QSV and VAAPI only take NV12; VAAPI also
/// needs the frames on the GPU.
pub fn pix_fmt(encoder: &str) -> &'static str {
    if encoder.ends_with("_vaapi") {
        "nv12,hwupload"
    } else if encoder.ends_with("_qsv") {
        "nv12"
    } else {
        "yuv420p"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENCODERS: &str = "Encoders:\n V..... = Video\n ------\n V....D libx264              libx264 H.264\n V....D libx265              libx265 H.265\n V....D h264_nvenc           NVIDIA NVENC H.264 encoder\n V....D hevc_nvenc           NVIDIA NVENC hevc encoder\n A....D aac                  AAC\n";
    const HWACCELS: &str = "Hardware acceleration methods:\ncuda\nvaapi\n\n";

    fn caps() -> Capabilities {
        Capabilities::parse(ENCODERS, HWACCELS)
    }

    #[test]
    fn parses_lists() {
        let c = caps();
        assert!(c.has_encoder("libx264") && c.has_encoder("hevc_nvenc") && c.has_encoder("aac"));
        assert_eq!(c.hwaccels, HashSet::from(["cuda".to_owned(), "vaapi".to_owned()]));
    }

    #[test]
    fn chooses_encoders() {
        let c = caps();
        let nv = choose(Backend::Cuda, VideoCodec::H264, &c).unwrap();
        assert_eq!((nv.encoder.as_str(), nv.hwaccel), ("h264_nvenc", Some("cuda")));
        assert!(nv.is_hardware());
        // No AV1 encoder at all: not offered.
        assert_eq!(choose(Backend::Cuda, VideoCodec::Av1, &c), None);
        // Missing hardware encoder: software, without hwaccel.
        let amf = choose(Backend::D3d11vaAmf, VideoCodec::Hevc, &c).unwrap();
        assert_eq!((amf.encoder.as_str(), amf.hwaccel), ("libx265", None));
        assert!(!amf.is_hardware());
        assert_eq!(choose(Backend::Software, VideoCodec::H264, &c).unwrap().encoder, "libx264");
        // Encoder present but hwaccel missing: CPU decode.
        let mut c2 = c.clone();
        c2.encoders.insert("h264_qsv".into());
        let qsv = choose(Backend::D3d11vaQsv, VideoCodec::H264, &c2).unwrap();
        assert_eq!((qsv.encoder.as_str(), qsv.hwaccel), ("h264_qsv", None));
        assert_eq!(software(VideoCodec::Hevc, &c).unwrap().encoder, "libx265");
    }

    #[test]
    fn rate_control() {
        let j = |e: &str, t: Quality| rate_args(e, t, 30.0).unwrap().join(" ");
        // 10 MB over 30 s: 10·8000·0.95/30 − 160 = 2373 kbit/s.
        let size = Quality::MaxSizeMb { mb: 10 };
        assert_eq!(j("h264_nvenc", size), "-rc vbr -b:v 2373k -maxrate 2373k -bufsize 4746k -preset p5");
        assert_eq!(j("libx264", size), "-b:v 2373k -maxrate 2373k -bufsize 4746k -preset medium");
        assert_eq!(j("h264_amf", size), "-b:v 2373k -maxrate 2373k -bufsize 4746k");
        let crf = Quality::Crf { q: 22 };
        assert_eq!(j("hevc_nvenc", crf), "-rc vbr -cq 22 -b:v 0 -preset p5");
        assert_eq!(j("libx264", crf), "-crf 22 -preset medium");
        assert_eq!(j("libx265", crf), "-crf 22 -preset fast -x265-params log-level=error");
        assert_eq!(j("libsvtav1", crf), "-crf 22 -preset 8");
        assert_eq!(j("h264_qsv", crf), "-global_quality 22");
        assert_eq!(j("hevc_vaapi", crf), "-qp 22");
        assert_eq!(j("av1_amf", crf), "-rc cqp -qp_i 22 -qp_p 22");
        // 10 MB over 5 minutes: 253 − 160 < 300 kbit/s.
        assert!(matches!(rate_args("libx264", size, 300.0), Err(AppError::Ffmpeg(_))));
        assert!(rate_args("libx264", Quality::Crf { q: 22 }, 3600.0).is_ok());
    }

    #[test]
    fn pixel_formats() {
        assert_eq!(pix_fmt("h264_nvenc"), "yuv420p");
        assert_eq!(pix_fmt("hevc_qsv"), "nv12");
        assert_eq!(pix_fmt("h264_vaapi"), "nv12,hwupload");
    }

    #[test]
    fn serde_shapes() {
        assert_eq!(serde_json::to_string(&VideoCodec::H264).unwrap(), r#""h264""#);
        let q: Quality = serde_json::from_str(r#"{"kind":"maxSizeMb","mb":10}"#).unwrap();
        assert_eq!(q, Quality::MaxSizeMb { mb: 10 });
        assert_eq!(serde_json::to_string(&Quality::Crf { q: 22 }).unwrap(), r#"{"kind":"crf","q":22}"#);
    }
}
