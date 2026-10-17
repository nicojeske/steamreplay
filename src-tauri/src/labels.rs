//! M14 labelling: candidate pools written by the experiment scripts (`experiments/moments/`) and
//! the ratings given in the app. Files live in `app_data_dir/labels/`, never in Steam's folders.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;

use crate::error::{AppError, AppResult};

const VERSION: u32 = 1;

/// A region to rate. The pool does not say which method found it, so the rating is blind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PoolCandidate {
    pub id: String,
    #[specta(type = specta_typescript::Number)]
    pub start_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub end_s: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Good,
    Meh,
    Bad,
    /// A good moment that no candidate covers; for recall.
    Missed,
}

/// Times are stored with the label, so it still matches by overlap after the pool is rebuilt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    /// `None` for `Missed`.
    pub candidate_id: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub start_s: f64,
    #[specta(type = specta_typescript::Number)]
    pub end_s: f64,
    pub rating: Rating,
}

#[derive(Serialize, Deserialize)]
struct PoolFile {
    version: u32,
    candidates: Vec<PoolCandidate>,
}

#[derive(Serialize, Deserialize)]
struct LabelsFile {
    version: u32,
    labels: Vec<Label>,
}

/// `115461900/bg/bg_1445790_…` → `115461900__bg__bg_1445790_…`. The scripts use the same rule.
pub fn file_stem(id: &str) -> String {
    id.replace(['/', '\\'], "__")
}

fn pool_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.pool.json", file_stem(id)))
}

fn labels_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{}.labels.json", file_stem(id)))
}

/// `None` when the session has no pool. A broken file is logged and treated as missing.
pub fn load_pool(dir: &Path, id: &str) -> Option<Vec<PoolCandidate>> {
    let path = pool_path(dir, id);
    let data = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<PoolFile>(&data) {
        Ok(f) => {
            let mut c = f.candidates;
            c.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
            Some(c)
        }
        Err(e) => {
            log::warn!("ignoring label pool {}: {e}", path.display());
            None
        }
    }
}

/// Empty when there are no labels yet. A broken file is logged and treated as empty.
pub fn load_labels(dir: &Path, id: &str) -> Vec<Label> {
    let path = labels_path(dir, id);
    let Ok(data) = std::fs::read(&path) else { return Vec::new() };
    serde_json::from_slice::<LabelsFile>(&data).map(|f| f.labels).unwrap_or_else(|e| {
        log::warn!("ignoring labels {}: {e}", path.display());
        Vec::new()
    })
}

/// Same target: the same candidate, or for `Missed` the same start.
fn same_target(a: &Label, b: &Label) -> bool {
    match (&a.candidate_id, &b.candidate_id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.start_s == b.start_s,
        _ => false,
    }
}

/// Replaces the label for the same target, or removes it.
pub fn apply(labels: &mut Vec<Label>, label: Label, remove: bool) {
    labels.retain(|l| !same_target(l, &label));
    if !remove {
        labels.push(label);
    }
}

pub fn save_labels(dir: &Path, id: &str, labels: &[Label]) -> AppResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| AppError::io(dir.display(), e))?;
    let path = labels_path(dir, id);
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&LabelsFile { version: VERSION, labels: labels.to_vec() })
        .map_err(|e| AppError::Io(e.to_string()))?;
    std::fs::write(&tmp, data).map_err(|e| AppError::io(tmp.display(), e))?;
    std::fs::rename(&tmp, &path).map_err(|e| AppError::io(path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "115461900/bg/bg_1445790_20261015_210120";

    fn label(cid: Option<&str>, start_s: f64, rating: Rating) -> Label {
        Label { candidate_id: cid.map(str::to_owned), start_s, end_s: start_s + 6.0, rating }
    }

    #[test]
    fn stem() {
        assert_eq!(file_stem(ID), "115461900__bg__bg_1445790_20261015_210120");
    }

    #[test]
    fn labels_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_labels(dir.path(), ID).is_empty());
        let mut l = Vec::new();
        apply(&mut l, label(Some("c1"), 10.0, Rating::Good), false);
        apply(&mut l, label(Some("c1"), 10.0, Rating::Bad), false);
        apply(&mut l, label(None, 50.0, Rating::Missed), false);
        apply(&mut l, label(None, 70.0, Rating::Missed), false);
        apply(&mut l, label(None, 70.0, Rating::Missed), true);
        assert_eq!(l, vec![label(Some("c1"), 10.0, Rating::Bad), label(None, 50.0, Rating::Missed)]);
        save_labels(&dir.path().join("labels"), ID, &l).unwrap();
        assert_eq!(load_labels(&dir.path().join("labels"), ID), l);
    }

    #[test]
    fn pool_missing_corrupt_and_sorted() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_pool(dir.path(), ID), None);
        std::fs::write(pool_path(dir.path(), ID), b"{ not json").unwrap();
        assert_eq!(load_pool(dir.path(), ID), None);
        std::fs::write(
            pool_path(dir.path(), ID),
            br#"{"version":1,"candidates":[{"id":"b","startS":30,"endS":36},{"id":"a","startS":5.5,"endS":9}]}"#,
        )
        .unwrap();
        let p = load_pool(dir.path(), ID).unwrap();
        assert_eq!(p.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // A corrupt labels file reads as empty.
        std::fs::write(labels_path(dir.path(), ID), b"[]").unwrap();
        assert!(load_labels(dir.path(), ID).is_empty());
    }
}
