//! Listing background recordings and clips, chunk inventory and timeline mapping.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{NaiveDateTime, TimeDelta};
use regex::Regex;
use serde::Serialize;
use specta::Type;

use super::locate::{SteamAccount, game_name};
use super::mpd::{MpdInfo, read_mpd};
use super::timeline::{Marker, NAME_TS_FORMAT, Timeline, parse_timeline_filename, read_timeline};
use crate::error::{AppError, AppResult};

/// Used when `session.mpd` is missing or unreadable. Steam always writes 3 s segments.
const DEFAULT_SEGMENT_S: f64 = 3.0;

static VIDEO_CHUNK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^chunk-stream0-(\d+)\.m4s$").expect("valid regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Background,
    Clip,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    /// Stable: `<accountId>/<bg|clip>/<folder name>`.
    pub id: String,
    pub kind: SessionKind,
    pub account_id: String,
    pub app_id: u32,
    pub game_name: String,
    /// ISO 8601 local time of video time 0 (the first chunk on disk).
    pub started_at: String,
    /// From the chunk inventory, not `mediaPresentationDuration`.
    #[specta(type = specta_typescript::Number)]
    pub duration_s: f64,
    pub marker_count: u32,
    #[specta(type = specta_typescript::Number)]
    pub size_bytes: u64,
    /// The folder that holds `session.mpd`.
    pub dir: PathBuf,
}

/// Based on the video stream files that exist on disk.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ChunkInventory {
    pub first: u32,
    pub last: u32,
    pub count: u32,
    pub missing: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub mpd: Option<MpdInfo>,
    pub mpd_error: Option<String>,
    pub chunks: ChunkInventory,
    /// Seconds between the bg folder timestamp and video time 0.
    #[specta(type = specta_typescript::Number)]
    pub offset_s: f64,
    pub markers: Vec<Marker>,
    /// Timeline entries that fall before or after the video on disk.
    pub markers_outside: u32,
    pub timeline_file: Option<PathBuf>,
    /// Seconds the timeline starts before the bg folder.
    #[specta(type = Option<specta_typescript::Number>)]
    pub timeline_lead_s: Option<f64>,
    /// Timeline `endtime` in seconds since the timeline start.
    #[specta(type = Option<specta_typescript::Number>)]
    pub timeline_end_s: Option<f64>,
}

/// `<prefix><appid>_<YYYYMMDD_HHMMSS>` → `(appid, local time)`. Used for `bg_` and `clip_` folders.
pub fn parse_folder_name(name: &str, prefix: &str) -> Option<(u32, NaiveDateTime)> {
    let rest = name.get(..prefix.len()).filter(|p| p.eq_ignore_ascii_case(prefix)).map(|_| &name[prefix.len()..])?;
    let (appid, ts) = rest.split_once('_')?;
    Some((appid.parse().ok()?, NaiveDateTime::parse_from_str(ts, NAME_TS_FORMAT).ok()?))
}

/// Lists the folder once: video chunk numbers and the total size of all files.
pub fn inventory(dir: &Path) -> AppResult<(ChunkInventory, u64)> {
    let rd = std::fs::read_dir(dir).map_err(|e| AppError::io(dir.display(), e))?;
    let mut numbers = BTreeSet::new();
    let mut size = 0u64;
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        size += meta.len();
        if let Some(n) = entry
            .file_name()
            .to_str()
            .and_then(|n| VIDEO_CHUNK.captures(n))
            .and_then(|c| c[1].parse::<u32>().ok())
        {
            numbers.insert(n);
        }
    }
    Ok((chunk_inventory(&numbers), size))
}

pub fn chunk_inventory(numbers: &BTreeSet<u32>) -> ChunkInventory {
    let (Some(&first), Some(&last)) = (numbers.first(), numbers.last()) else {
        return ChunkInventory::default();
    };
    ChunkInventory {
        first,
        last,
        count: numbers.len() as u32,
        missing: (first..=last).filter(|n| !numbers.contains(n)).collect(),
    }
}

/// Seconds from the bg folder timestamp to the start of the first chunk on disk.
pub fn video_offset(mpd: Option<&MpdInfo>, chunks: &ChunkInventory) -> f64 {
    match mpd {
        // Period@start is where segment startNumber sits. For clips it is not a multiple of 3 s.
        Some(m) => {
            let first = if chunks.count > 0 { chunks.first } else { m.start_number };
            m.period_start_s + (f64::from(first) - f64::from(m.start_number)) * m.segment_duration_s
        }
        None if chunks.count > 0 => f64::from(chunks.first - 1) * DEFAULT_SEGMENT_S,
        None => 0.0,
    }
}

pub fn duration(segment_s: f64, chunks: &ChunkInventory) -> f64 {
    if chunks.count == 0 { 0.0 } else { f64::from(chunks.last - chunks.first + 1) * segment_s }
}

/// Converts timeline entries to video time and keeps those inside `[0, duration]`.
/// `video_t = time_ms/1000 − (bg_ts − timeline_ts) − offset`.
pub fn markers_for(
    timeline: &Timeline,
    bg_ts: NaiveDateTime,
    timeline_ts: NaiveDateTime,
    offset_s: f64,
    duration_s: f64,
) -> (Vec<Marker>, u32) {
    let lead = (bg_ts - timeline_ts).as_seconds_f64();
    let mut inside = Vec::new();
    let mut outside = 0;
    for m in timeline.entries.iter().filter_map(|e| Marker::from_entry(e, lead + offset_s)) {
        if (0.0..=duration_s).contains(&m.video_time_s) {
            inside.push(m);
        } else {
            outside += 1;
        }
    }
    inside.sort_by(|a, b| a.video_time_s.total_cmp(&b.video_time_s));
    (inside, outside)
}

/// Timelines in a folder by appid, sorted by start time.
fn timelines_in(dir: &Path) -> HashMap<u32, Vec<(NaiveDateTime, PathBuf)>> {
    let mut out: HashMap<u32, Vec<_>> = HashMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        match parse_timeline_filename(name) {
            Some((appid, ts)) => out.entry(appid).or_default().push((ts, entry.path())),
            None => log::warn!("skipping unexpected timeline file {}", entry.path().display()),
        }
    }
    for list in out.values_mut() {
        list.sort();
    }
    out
}

/// The latest timeline for `appid` that starts at or before `bg_ts`.
fn match_timeline(
    index: &HashMap<u32, Vec<(NaiveDateTime, PathBuf)>>,
    appid: u32,
    bg_ts: NaiveDateTime,
) -> Option<(NaiveDateTime, PathBuf)> {
    index.get(&appid)?.iter().rev().find(|(ts, _)| *ts <= bg_ts).cloned()
}

struct Candidate {
    kind: SessionKind,
    id_name: String,
    appid: u32,
    bg_ts: NaiveDateTime,
    dir: PathBuf,
    timeline: Option<(NaiveDateTime, PathBuf)>,
}

fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().to_str()?.to_owned(), e.path())))
        .collect();
    out.sort();
    out
}

fn candidates(root: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let timelines = timelines_in(&root.join("timelines"));

    for (name, dir) in subdirs(&root.join("video")) {
        let Some((appid, bg_ts)) = parse_folder_name(&name, "bg_") else {
            log::warn!("skipping unexpected folder {}", dir.display());
            continue;
        };
        let timeline = match_timeline(&timelines, appid, bg_ts);
        out.push(Candidate { kind: SessionKind::Background, id_name: name, appid, bg_ts, dir, timeline });
    }

    for (clip_name, clip_dir) in subdirs(&root.join("clips")) {
        if parse_folder_name(&clip_name, "clip_").is_none() {
            log::warn!("skipping unexpected folder {}", clip_dir.display());
            continue;
        }
        let bgs: Vec<_> = subdirs(&clip_dir.join("video"))
            .into_iter()
            .filter_map(|(n, d)| parse_folder_name(&n, "bg_").map(|p| (p, d)))
            .collect();
        let [((appid, bg_ts), dir)] = bgs.as_slice() else {
            log::warn!("clip {} has {} bg folders, expected 1", clip_dir.display(), bgs.len());
            continue;
        };
        let clip_timelines = timelines_in(&clip_dir.join("timelines"));
        let timeline = match_timeline(&clip_timelines, *appid, *bg_ts);
        out.push(Candidate {
            kind: SessionKind::Clip,
            id_name: clip_name,
            appid: *appid,
            bg_ts: *bg_ts,
            dir: dir.clone(),
            timeline,
        });
    }
    out
}

fn scan(account: &SteamAccount, c: Candidate, names: &HashMap<u32, String>) -> SessionDetail {
    let (chunks, size_bytes) = inventory(&c.dir).unwrap_or_else(|e| {
        log::warn!("{e}");
        (ChunkInventory::default(), 0)
    });
    let mpd_path = c.dir.join("session.mpd");
    let (mpd, mpd_error) = if mpd_path.is_file() {
        match read_mpd(&mpd_path) {
            Ok(m) => (Some(m), None),
            Err(e) => {
                log::warn!("{e}");
                (None, Some(e.to_string()))
            }
        }
    } else {
        (None, Some("session.mpd not found".to_owned()))
    };
    let segment_s = mpd.as_ref().map_or(DEFAULT_SEGMENT_S, |m| m.segment_duration_s);
    let duration_s = duration(segment_s, &chunks);
    let offset_s = video_offset(mpd.as_ref(), &chunks);
    let started_at = c.bg_ts + TimeDelta::milliseconds((offset_s * 1000.0).round() as i64);

    let (markers, markers_outside, timeline_lead_s, timeline_end_s) = match &c.timeline {
        Some((tl_ts, path)) => match read_timeline(path) {
            Ok(tl) => {
                let (m, out) = markers_for(&tl, c.bg_ts, *tl_ts, offset_s, duration_s);
                (m, out, Some((c.bg_ts - *tl_ts).as_seconds_f64()), tl.endtime.map(|ms| ms / 1000.0))
            }
            Err(e) => {
                log::warn!("{e}");
                (Vec::new(), 0, None, None)
            }
        },
        None => (Vec::new(), 0, None, None),
    };

    let kind_tag = match c.kind {
        SessionKind::Background => "bg",
        SessionKind::Clip => "clip",
    };
    let summary = SessionSummary {
        id: format!("{}/{}/{}", account.account_id, kind_tag, c.id_name),
        kind: c.kind,
        account_id: account.account_id.clone(),
        app_id: c.appid,
        game_name: game_name(names, c.appid),
        started_at: started_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
        duration_s,
        marker_count: markers.len() as u32,
        size_bytes,
        dir: c.dir,
    };
    SessionDetail {
        summary,
        mpd,
        mpd_error,
        chunks,
        offset_s,
        markers,
        markers_outside,
        timeline_file: c.timeline.map(|(_, p)| p),
        timeline_lead_s,
        timeline_end_s,
    }
}

/// All sessions of all accounts, newest first.
pub fn list_sessions(accounts: &[SteamAccount], names: &HashMap<u32, String>) -> Vec<SessionSummary> {
    let mut out: Vec<_> = accounts
        .iter()
        .flat_map(|a| candidates(&a.recordings_root).into_iter().map(move |c| scan(a, c, names).summary))
        .collect();
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at).then_with(|| a.id.cmp(&b.id)));
    out
}

pub fn get_session(accounts: &[SteamAccount], names: &HashMap<u32, String>, id: &str) -> AppResult<SessionDetail> {
    let (account_id, rest) = id.split_once('/').ok_or_else(|| AppError::NotFound(format!("bad session id '{id}'")))?;
    let account = accounts
        .iter()
        .find(|a| a.account_id == account_id)
        .ok_or_else(|| AppError::NotFound(format!("account {account_id} not found")))?;
    let (kind_tag, name) = rest.split_once('/').ok_or_else(|| AppError::NotFound(format!("bad session id '{id}'")))?;
    candidates(&account.recordings_root)
        .into_iter()
        .filter(|c| c.id_name == name)
        .find(|c| matches!((c.kind, kind_tag), (SessionKind::Background, "bg") | (SessionKind::Clip, "clip")))
        .map(|c| scan(account, c, names))
        .ok_or_else(|| AppError::NotFound(format!("session {id} not found")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam::timeline::parse_timeline;
    use chrono::NaiveDate;
    use std::fs;

    fn fixture(name: &str) -> String {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    fn touch_chunks(dir: &Path, numbers: impl IntoIterator<Item = u32>) {
        fs::create_dir_all(dir).unwrap();
        for n in numbers {
            fs::write(dir.join(format!("chunk-stream0-{n:05}.m4s")), b"v").unwrap();
            fs::write(dir.join(format!("chunk-stream1-{n:05}.m4s")), b"a").unwrap();
        }
    }

    #[test]
    fn folder_names() {
        assert_eq!(parse_folder_name("bg_1623730_20261010_164846", "bg_"), Some((1623730, ts(2026, 10, 10, 16, 48, 46))));
        assert_eq!(parse_folder_name("clip_1478500_20260913_011121", "clip_").unwrap().0, 1478500);
        assert!(parse_folder_name("bg_x_20261010_164846", "bg_").is_none());
        assert!(parse_folder_name("bg_1623730", "bg_").is_none());
        assert!(parse_folder_name("clip_1_20261010_164846", "bg_").is_none());
    }

    #[test]
    fn inventory_gaps_and_case() {
        let dir = tempfile::tempdir().unwrap();
        touch_chunks(dir.path(), [3418, 3419, 3422]);
        fs::write(dir.path().join("CHUNK-STREAM0-03423.M4S"), b"v").unwrap();
        fs::write(dir.path().join("init-stream0.m4s"), b"init").unwrap();
        let (inv, size) = inventory(dir.path()).unwrap();
        assert_eq!(inv, ChunkInventory { first: 3418, last: 3423, count: 4, missing: vec![3420, 3421] });
        assert_eq!(size, 3 * 2 + 1 + 4);
        assert_eq!(duration(3.0, &inv), 18.0);

        let empty = tempfile::tempdir().unwrap();
        let (inv, _) = inventory(empty.path()).unwrap();
        assert_eq!(inv.count, 0);
        assert_eq!(duration(3.0, &inv), 0.0);
    }

    #[test]
    fn offsets() {
        let pruned = crate::steam::mpd::parse_mpd(&fixture("session_1623730_pruned.mpd")).unwrap();
        let inv = ChunkInventory { first: 3418, last: 4727, count: 1310, missing: vec![] };
        assert_eq!(video_offset(Some(&pruned), &inv), 3417.0 * 3.0);
        // Steam pruned two more chunks after writing the manifest.
        let inv2 = ChunkInventory { first: 3420, ..inv.clone() };
        assert_eq!(video_offset(Some(&pruned), &inv2), 3419.0 * 3.0);
        assert_eq!(video_offset(None, &inv), 3417.0 * 3.0);

        let clip = crate::steam::mpd::parse_mpd(&fixture("session_clip_1478500.mpd")).unwrap();
        let inv = ChunkInventory { first: 1, last: 2, count: 2, missing: vec![] };
        assert!((video_offset(Some(&clip), &inv) - 8259.841).abs() < 1e-9);
    }

    #[test]
    fn marker_video_time() {
        let tl = parse_timeline(&fixture("timeline_144579020261015_210113.json")).unwrap();
        let bg = ts(2026, 10, 15, 21, 1, 20);
        let tl_ts = ts(2026, 10, 15, 21, 1, 13);
        let (markers, outside) = markers_for(&tl, bg, tl_ts, 0.0, 1240.0 * 3.0);
        // Recording failed at 3735.7 s (the `error` entry), so the video stops at 3720 s.
        // The game went on: 3 of the 5 user markers come after the end of the video.
        assert_eq!(markers.len(), 2);
        assert!(markers.iter().all(|m| m.kind == "usermarker"));
        assert_eq!(outside, 4);
        assert!((markers[0].video_time_s - (2291.279 - 7.0)).abs() < 1e-9);

        // Pruned buffer: the first 1000 s are gone, so the first marker drops out.
        let (markers, outside) = markers_for(&tl, bg, tl_ts, 1000.0 * 3.0, 240.0 * 3.0);
        assert!(markers.iter().all(|m| m.video_time_s >= 0.0 && m.video_time_s <= 720.0));
        assert_eq!(markers.len() as u32 + outside, 6);
    }

    #[test]
    fn scans_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("gamerecordings");
        let bg = root.join("video/bg_1445790_20261015_210120");
        touch_chunks(&bg, 1..=1240);
        fs::write(bg.join("session.mpd"), fixture("session_1445790.mpd")).unwrap();
        let pruned = root.join("video/bg_1623730_20261010_164846");
        touch_chunks(&pruned, 3418..=3420);
        fs::write(pruned.join("session.mpd"), fixture("session_1623730_pruned.mpd")).unwrap();
        fs::create_dir_all(root.join("video/bg_1623730_20261016_100000")).unwrap(); // just created
        fs::create_dir_all(root.join("video/not_a_session")).unwrap();
        fs::create_dir_all(root.join("timelines")).unwrap();
        fs::write(root.join("timelines/timeline_144579020261015_210113.json"), fixture("timeline_144579020261015_210113.json"))
            .unwrap();
        fs::write(root.join("timelines/timeline_144579020261015_220000.json"), "{}").unwrap(); // later: not matched
        let clip = root.join("clips/clip_1478500_20260913_011121");
        touch_chunks(&clip.join("video/bg_1478500_20260912_225341"), 1..=2);
        fs::write(clip.join("video/bg_1478500_20260912_225341/session.mpd"), fixture("session_clip_1478500.mpd")).unwrap();
        fs::create_dir_all(clip.join("timelines")).unwrap();
        fs::write(clip.join("timelines/timeline_147850020260912_225333.json"), fixture("timeline_147850020260912_225333.json"))
            .unwrap();

        let account = SteamAccount { account_id: "1".into(), recordings_root: root.clone(), export_dir: None };
        let mut names = HashMap::new();
        names.insert(1445790, "Make Way".to_owned());
        let list = list_sessions(std::slice::from_ref(&account), &names);
        assert_eq!(list.len(), 4);
        // newest first
        let starts: Vec<_> = list.iter().map(|s| s.started_at.as_str()).collect();
        let mut sorted = starts.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(starts, sorted);

        let main = list.iter().find(|s| s.id == "1/bg/bg_1445790_20261015_210120").unwrap();
        assert_eq!(main.game_name, "Make Way");
        assert_eq!(main.duration_s, 3720.0);
        assert_eq!(main.marker_count, 2);
        assert_eq!(main.started_at, "2026-10-15T21:01:20");

        let p = list.iter().find(|s| s.id == "1/bg/bg_1623730_20261010_164846").unwrap();
        assert_eq!(p.game_name, "App 1623730");
        assert_eq!(p.started_at, "2026-10-10T19:39:37"); // 16:48:46 + 3417 · 3 s

        let empty = list.iter().find(|s| s.id == "1/bg/bg_1623730_20261016_100000").unwrap();
        assert_eq!(empty.duration_s, 0.0);

        let c = list.iter().find(|s| s.kind == SessionKind::Clip).unwrap();
        assert_eq!(c.id, "1/clip/clip_1478500_20260913_011121");
        assert_eq!(c.started_at, "2026-09-13T01:11:20"); // 22:53:41 + 2:17:39.841, matches the clip folder name
        assert_eq!(c.duration_s, 6.0);

        let d = get_session(std::slice::from_ref(&account), &names, &main.id).unwrap();
        assert_eq!(d.markers.len(), 2);
        assert_eq!(d.markers_outside, 4);
        assert_eq!(d.timeline_lead_s, Some(7.0));
        assert!(d.timeline_file.unwrap().ends_with("timeline_144579020261015_210113.json"));
        assert_eq!(d.mpd.unwrap().video.width, Some(5120));

        let d = get_session(std::slice::from_ref(&account), &names, &c.id).unwrap();
        assert_eq!(d.markers.len(), 0);
        assert_eq!(d.markers_outside, 1);

        let d = get_session(std::slice::from_ref(&account), &names, &empty.id).unwrap();
        assert!(d.mpd.is_none());
        assert!(d.mpd_error.is_some());

        assert!(matches!(get_session(&[account], &names, "1/bg/nope"), Err(AppError::NotFound(_))));
    }
}
