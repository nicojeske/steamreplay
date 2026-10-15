//! Parsing of Steam's `timeline_<appid><YYYYMMDD_HHMMSS>.json` marker files.

use std::path::Path;

use chrono::NaiveDateTime;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use specta::Type;

use crate::error::{AppError, AppResult};

pub const NAME_TS_FORMAT: &str = "%Y%m%d_%H%M%S";
const NAME_TS_LEN: usize = 15;

#[derive(Debug, Clone, Deserialize)]
pub struct Timeline {
    /// Milliseconds since the timeline start.
    #[serde(default, deserialize_with = "de_opt_num")]
    pub endtime: Option<f64>,
    #[serde(default)]
    pub entries: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Marker {
    /// Steam's `type` field, e.g. `usermarker`, `achievement`, `error`.
    pub kind: String,
    pub title: String,
    /// Seconds since the timeline start (`time` / 1000).
    #[specta(type = specta_typescript::Number)]
    pub timeline_time_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub video_time_s: f64,
    /// The unmodified timeline entry.
    #[specta(type = specta_typescript::Unknown)]
    pub raw: Value,
}

/// Accepts a number, or a string holding a number (Steam stores most numbers as strings).
fn de_opt_num<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    Ok(value_as_f64(&Value::deserialize(d)?))
}

pub fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn parse_timeline(json: &str) -> AppResult<Timeline> {
    serde_json::from_str(json).map_err(|e| AppError::Parse(format!("timeline: {e}")))
}

pub fn read_timeline(path: &Path) -> AppResult<Timeline> {
    let json = std::fs::read_to_string(path).map_err(|e| AppError::io(path.display(), e))?;
    parse_timeline(&json).map_err(|e| AppError::Parse(format!("{}: {e}", path.display())))
}

/// `timeline_<appid><YYYYMMDD_HHMMSS>.json` → `(appid, local start time)`.
/// There is no separator between appid and date, so the last 15 characters are the date.
pub fn parse_timeline_filename(name: &str) -> Option<(u32, NaiveDateTime)> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_prefix("timeline_")?.strip_suffix(".json")?;
    if stem.len() <= NAME_TS_LEN || !stem.is_char_boundary(stem.len() - NAME_TS_LEN) {
        return None;
    }
    let (appid, ts) = stem.split_at(stem.len() - NAME_TS_LEN);
    let appid = appid.parse().ok()?;
    let ts = NaiveDateTime::parse_from_str(ts, NAME_TS_FORMAT).ok()?;
    Some((appid, ts))
}

impl Marker {
    /// Builds a marker from a raw timeline entry. `offset_s` is subtracted from the
    /// timeline time to get video time. Returns `None` for entries without a `time`.
    pub fn from_entry(entry: &Value, offset_s: f64) -> Option<Self> {
        let time_ms = value_as_f64(entry.get("time")?)?;
        let str_field = |k: &str| entry.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
        let title = str_field("title")
            .or_else(|| str_field("achievement_name"))
            .or_else(|| str_field("description"))
            .unwrap_or_default()
            .to_owned();
        let timeline_time_s = time_ms / 1000.0;
        Some(Self {
            kind: str_field("type").unwrap_or("unknown").to_owned(),
            title,
            timeline_time_s,
            video_time_s: timeline_time_s - offset_s,
            raw: entry.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn fixture(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn filename() {
        let (appid, ts) = parse_timeline_filename("timeline_144579020261015_210113.json").unwrap();
        assert_eq!(appid, 1445790);
        assert_eq!(ts, NaiveDate::from_ymd_opt(2026, 10, 15).unwrap().and_hms_opt(21, 1, 13).unwrap());
        assert_eq!(parse_timeline_filename("Timeline_7020261015_210113.JSON").unwrap().0, 70);
        assert!(parse_timeline_filename("timeline_20261015_210113.json").is_none());
        assert!(parse_timeline_filename("timeline_1445790_20261015_210113.json").is_none());
        assert!(parse_timeline_filename("timeline_144579020261015_216113.json").is_none());
        assert!(parse_timeline_filename("other.json").is_none());
    }

    #[test]
    fn numbers_as_strings() {
        let t = parse_timeline(r#"{"daterecorded":"1792098073","endtime":"5986786","entries":[]}"#).unwrap();
        assert_eq!(t.endtime, Some(5986786.0));
        let t = parse_timeline(r#"{"endtime":5986786}"#).unwrap();
        assert_eq!(t.endtime, Some(5986786.0));
        let t = parse_timeline(r#"{"entries":[{"time":"x"}]}"#).unwrap();
        assert_eq!(t.endtime, None);
        assert!(Marker::from_entry(&t.entries[0], 0.0).is_none());
    }

    #[test]
    fn sample_timeline() {
        let t = parse_timeline(&fixture("timeline_144579020261015_210113.json")).unwrap();
        let markers: Vec<_> = t.entries.iter().filter_map(|e| Marker::from_entry(e, 7.0)).collect();
        assert_eq!(markers.iter().filter(|m| m.kind == "usermarker").count(), 5);
        let err = markers.iter().find(|m| m.kind == "error").unwrap();
        assert_eq!(err.title, "#GameRecording_RecordingFailed");
        assert!((err.timeline_time_s - 3735.706).abs() < 1e-9);
        assert!((err.video_time_s - 3728.706).abs() < 1e-9);
        assert_eq!(err.raw["id"], "1");
    }

    #[test]
    fn achievement_title() {
        let t = parse_timeline(&fixture("timeline_147850020260912_225333.json")).unwrap();
        let m = Marker::from_entry(&t.entries[0], 0.0).unwrap();
        assert_eq!(m.kind, "achievement");
        assert_eq!(m.title, "Holster");
    }
}
