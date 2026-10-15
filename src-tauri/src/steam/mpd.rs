//! Parsing of Steam's `session.mpd` (MPEG-DASH manifest).

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StreamInfo {
    pub rep_id: String,
    pub codecs: String,
    pub bandwidth: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MpdInfo {
    /// `duration / timescale` of the segment template, in seconds.
    #[specta(type = specta_typescript::Number)]
    pub segment_duration_s: f64,
    pub start_number: u32,
    /// `Period@start`: where segment `start_number` sits in the original recording.
    /// For clips this is the clip's position inside the source session.
    #[specta(type = specta_typescript::Number)]
    pub period_start_s: f64,
    pub video: StreamInfo,
    pub audio: StreamInfo,
    pub media_template: String,
    pub init_template: String,
}

// Raw serde shapes. Unknown attributes and elements (xmlns, profiles, ...) are ignored.

#[derive(Deserialize)]
struct RawMpd {
    #[serde(rename = "Period", default)]
    periods: Vec<RawPeriod>,
}

#[derive(Deserialize)]
struct RawPeriod {
    #[serde(rename = "@start")]
    start: Option<String>,
    #[serde(rename = "AdaptationSet", default)]
    sets: Vec<RawAdaptationSet>,
}

#[derive(Deserialize)]
struct RawAdaptationSet {
    #[serde(rename = "@contentType")]
    content_type: Option<String>,
    #[serde(rename = "@mimeType")]
    mime_type: Option<String>,
    #[serde(rename = "SegmentTemplate")]
    segment_template: Option<RawSegmentTemplate>,
    #[serde(rename = "Representation", default)]
    reps: Vec<RawRepresentation>,
}

#[derive(Deserialize)]
struct RawRepresentation {
    #[serde(rename = "@id")]
    id: String,
    #[serde(rename = "@mimeType")]
    mime_type: Option<String>,
    #[serde(rename = "@codecs")]
    codecs: Option<String>,
    #[serde(rename = "@bandwidth")]
    bandwidth: Option<u32>,
    #[serde(rename = "@width")]
    width: Option<u32>,
    #[serde(rename = "@height")]
    height: Option<u32>,
    #[serde(rename = "@audioSamplingRate")]
    audio_sampling_rate: Option<u32>,
    #[serde(rename = "AudioChannelConfiguration")]
    audio_channels: Option<RawAudioChannels>,
    #[serde(rename = "SegmentTemplate")]
    segment_template: Option<RawSegmentTemplate>,
}

#[derive(Deserialize)]
struct RawAudioChannels {
    #[serde(rename = "@value")]
    value: Option<String>,
}

#[derive(Deserialize, Clone)]
struct RawSegmentTemplate {
    #[serde(rename = "@timescale")]
    timescale: Option<u64>,
    #[serde(rename = "@duration")]
    duration: Option<u64>,
    #[serde(rename = "@initialization")]
    initialization: Option<String>,
    #[serde(rename = "@media")]
    media: Option<String>,
    #[serde(rename = "@startNumber")]
    start_number: Option<u32>,
}

#[derive(PartialEq)]
enum Kind {
    Video,
    Audio,
}

fn set_kind(set: &RawAdaptationSet, rep: &RawRepresentation) -> Option<Kind> {
    let hint = set
        .content_type
        .as_deref()
        .or(set.mime_type.as_deref())
        .or(rep.mime_type.as_deref())?;
    if hint.starts_with("video") {
        Some(Kind::Video)
    } else if hint.starts_with("audio") {
        Some(Kind::Audio)
    } else {
        None
    }
}

pub fn parse_mpd(xml: &str) -> AppResult<MpdInfo> {
    let raw: RawMpd = quick_xml::de::from_str(xml)
        .map_err(|e| AppError::Parse(format!("session.mpd: {e}")))?;
    let period = raw
        .periods
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Parse("session.mpd: no Period".into()))?;
    let period_start_s = match period.start.as_deref() {
        Some(s) => parse_iso_duration(s)
            .ok_or_else(|| AppError::Parse(format!("session.mpd: bad Period start '{s}'")))?,
        None => 0.0,
    };

    let mut video = None;
    let mut audio = None;
    for set in &period.sets {
        for rep in &set.reps {
            let tpl = rep.segment_template.clone().or_else(|| set.segment_template.clone());
            match set_kind(set, rep) {
                Some(Kind::Video) if video.is_none() => video = Some((stream_info(rep), tpl)),
                Some(Kind::Audio) if audio.is_none() => audio = Some((stream_info(rep), tpl)),
                _ => {}
            }
        }
    }
    let (video, video_tpl) =
        video.ok_or_else(|| AppError::Parse("session.mpd: no video representation".into()))?;
    let (audio, _) =
        audio.ok_or_else(|| AppError::Parse("session.mpd: no audio representation".into()))?;
    let tpl = video_tpl
        .ok_or_else(|| AppError::Parse("session.mpd: video has no SegmentTemplate".into()))?;

    let timescale = tpl.timescale.unwrap_or(1).max(1);
    let duration = tpl
        .duration
        .ok_or_else(|| AppError::Parse("session.mpd: SegmentTemplate has no duration".into()))?;

    Ok(MpdInfo {
        segment_duration_s: duration as f64 / timescale as f64,
        start_number: tpl.start_number.unwrap_or(1),
        period_start_s,
        video,
        audio,
        media_template: tpl.media.unwrap_or_default(),
        init_template: tpl.initialization.unwrap_or_default(),
    })
}

pub fn read_mpd(path: &Path) -> AppResult<MpdInfo> {
    let xml = std::fs::read_to_string(path).map_err(|e| AppError::io(path.display(), e))?;
    parse_mpd(&xml)
}

fn stream_info(rep: &RawRepresentation) -> StreamInfo {
    StreamInfo {
        rep_id: rep.id.clone(),
        codecs: rep.codecs.clone().unwrap_or_default(),
        bandwidth: rep.bandwidth,
        width: rep.width,
        height: rep.height,
        sample_rate: rep.audio_sampling_rate,
        channels: rep
            .audio_channels
            .as_ref()
            .and_then(|c| c.value.as_deref())
            .and_then(|v| v.trim().parse().ok()),
    }
}

/// Parses an ISO 8601 duration such as `PT2H50M51.0S` or `P1DT2H`, in seconds.
pub fn parse_iso_duration(s: &str) -> Option<f64> {
    let rest = s.trim().strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total = 0.0;
    for (part, units) in [(date, &[('D', 86400.0)][..]), (time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)][..])] {
        let mut num = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                num.push(c);
            } else {
                let factor = units.iter().find(|(u, _)| *u == c)?.1;
                total += num.parse::<f64>().ok()? * factor;
                num.clear();
            }
        }
        if !num.is_empty() {
            return None;
        }
    }
    Some(total)
}

/// Expands a DASH `SegmentTemplate` string. Supports `$RepresentationID$`,
/// `$Number$`, `$Number%0Nd$` and `$$`.
#[allow(dead_code, reason = "segment file names are needed from M3 (export) on")]
pub fn expand_template(template: &str, rep_id: &str, number: u32) -> String {
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('$') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let token = &after[..end];
        match token {
            "" => out.push('$'),
            "RepresentationID" => out.push_str(rep_id),
            "Number" => out.push_str(&number.to_string()),
            t if t.starts_with("Number%0") && t.ends_with('d') => {
                let width: usize = t["Number%0".len()..t.len() - 1].parse().unwrap_or(0);
                out.push_str(&format!("{number:0width$}"));
            }
            other => {
                out.push('$');
                out.push_str(other);
                out.push('$');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn parses_main_sample() {
        let mpd = parse_mpd(&fixture("session_1445790.mpd")).unwrap();
        assert_eq!(mpd.segment_duration_s, 3.0);
        assert_eq!(mpd.start_number, 1);
        assert_eq!(mpd.period_start_s, 0.0);
        assert_eq!(mpd.video.codecs, "hev1.2.4.L123.B0");
        assert_eq!((mpd.video.width, mpd.video.height), (Some(5120), Some(1440)));
        assert_eq!(mpd.audio.codecs, "mp4a.40.2");
        assert_eq!(mpd.audio.sample_rate, Some(48000));
        assert_eq!(mpd.audio.channels, Some(2));
        assert_eq!(mpd.media_template, "chunk-stream$RepresentationID$-$Number%05d$.m4s");
        assert_eq!(mpd.init_template, "init-stream$RepresentationID$.m4s");
    }

    #[test]
    fn parses_pruned_sample() {
        let mpd = parse_mpd(&fixture("session_1623730_pruned.mpd")).unwrap();
        assert_eq!(mpd.start_number, 3418);
        assert_eq!(mpd.period_start_s, 3417.0 * 3.0);
    }

    #[test]
    fn parses_clip_sample() {
        let mpd = parse_mpd(&fixture("session_clip_1478500.mpd")).unwrap();
        assert_eq!(mpd.start_number, 1);
        assert!((mpd.period_start_s - (2.0 * 3600.0 + 17.0 * 60.0 + 39.841)).abs() < 1e-9);
        assert_eq!(mpd.video.width, Some(3840));
    }

    #[test]
    fn iso_durations() {
        assert_eq!(parse_iso_duration("PT0.0S"), Some(0.0));
        assert_eq!(parse_iso_duration("PT2H50M51.0S"), Some(10251.0));
        assert_eq!(parse_iso_duration("PT1H1M57.947S"), Some(3717.947));
        assert_eq!(parse_iso_duration("P1DT1S"), Some(86401.0));
        assert_eq!(parse_iso_duration("PT3"), None);
        assert_eq!(parse_iso_duration("2H"), None);
    }

    #[test]
    fn templates() {
        let media = "chunk-stream$RepresentationID$-$Number%05d$.m4s";
        assert_eq!(expand_template(media, "0", 3418), "chunk-stream0-03418.m4s");
        assert_eq!(expand_template(media, "1", 123456), "chunk-stream1-123456.m4s");
        assert_eq!(expand_template("init-stream$RepresentationID$.m4s", "1", 0), "init-stream1.m4s");
        assert_eq!(expand_template("a$Number$b$$c", "0", 7), "a7b$c");
    }
}
