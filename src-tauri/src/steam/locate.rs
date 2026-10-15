//! Finding Steam installs, accounts, their recording folders and game names.
//! Everything here only reads from Steam's folders.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use keyvalues_parser::{Obj, Value as VdfValue};
use serde::Serialize;
use specta::Type;

#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SteamAccount {
    pub account_id: String,
    pub recordings_root: PathBuf,
    pub export_dir: Option<PathBuf>,
}

/// A Steam root candidate and what was found there (for the empty state and diagnostics).
#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RootCheck {
    pub path: PathBuf,
    pub exists: bool,
    pub canonical: Option<PathBuf>,
}

/// Candidate Steam roots for this OS, in priority order. Not checked for existence.
pub fn candidate_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        out.extend(windows_registry_roots());
        out.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
        out.push(PathBuf::from(r"C:\Program Files\Steam"));
    }
    #[cfg(not(windows))]
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".local/share/Steam"));
        out.push(home.join(".steam/steam"));
        out.push(home.join(".steam/root"));
        out.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    out
}

#[cfg(windows)]
fn windows_registry_roots() -> Vec<PathBuf> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    let mut out = Vec::new();
    let lookups = [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Valve\Steam", "InstallPath"),
    ];
    for (hive, key, value) in lookups {
        let found = RegKey::predef(hive)
            .open_subkey(key)
            .and_then(|k| k.get_value::<String, _>(value));
        match found {
            // The value uses forward slashes (`c:/program files (x86)/steam`). Windows accepts
            // them, and canonicalisation turns them into a normal path.
            Ok(s) if !s.trim().is_empty() => out.push(PathBuf::from(s.trim())),
            Ok(_) => {}
            Err(e) => log::debug!("registry {key}\\{value}: {e}"),
        }
    }
    out
}

pub fn check_roots(candidates: &[PathBuf]) -> Vec<RootCheck> {
    candidates
        .iter()
        .map(|p| {
            let canonical = dunce::canonicalize(p).ok().filter(|c| c.is_dir());
            RootCheck { path: p.clone(), exists: canonical.is_some(), canonical }
        })
        .collect()
}

/// Existing Steam roots, canonicalised (symlinks resolved) and without duplicates.
pub fn steam_roots(checks: &[RootCheck]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    checks
        .iter()
        .filter_map(|c| c.canonical.clone())
        .filter(|c| c.join("userdata").is_dir() || c.join("steamapps").is_dir())
        .filter(|c| seen.insert(path_key(c)))
        .collect()
}

/// Key for comparing paths. Windows paths are case-insensitive.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

pub fn localconfig_path(steam_root: &Path, account_id: &str) -> PathBuf {
    steam_root.join("userdata").join(account_id).join("config").join("localconfig.vdf")
}

/// Raw text of the `"GameRecording" { ... }` block in `localconfig.vdf`.
/// Extracted by hand so a quirk elsewhere in the large file cannot break parsing.
pub fn game_recording_block(text: &str) -> Option<&str> {
    let start = text.find("\"GameRecording\"")?;
    let open = start + text[start..].find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in text[open..].char_indices() {
        if in_str {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..open + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

/// String settings of a `GameRecording` block.
pub fn parse_game_recording(block: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    match keyvalues_parser::parse(block) {
        Ok(vdf) => {
            if let Some(obj) = vdf.value.get_obj() {
                for (k, vals) in obj.iter() {
                    if let Some(v) = vals.first().and_then(VdfValue::get_str) {
                        out.insert(k.to_string(), v.to_owned());
                    }
                }
            }
        }
        Err(e) => log::warn!("cannot parse GameRecording block: {e}"),
    }
    out
}

/// If the block points at an existing recordings folder, returns the gamerecordings root.
/// The key name is unknown (Steam's default folder was in use when this was written), so any
/// key that looks like a path and leads to a folder with recordings is accepted.
pub fn custom_recordings_root(settings: &BTreeMap<String, String>) -> Option<PathBuf> {
    settings
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("ExportDirectory"))
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k.contains("path") || k.contains("folder") || k.contains("dir")
        })
        .find_map(|(_, v)| resolve_recordings_root(Path::new(v)))
}

/// Accepts either a gamerecordings folder or its parent.
pub fn resolve_recordings_root(p: &Path) -> Option<PathBuf> {
    if !p.is_dir() {
        return None;
    }
    if p.join("video").is_dir() || p.join("clips").is_dir() {
        return Some(p.to_path_buf());
    }
    let nested = p.join("gamerecordings");
    (nested.join("video").is_dir() || nested.join("clips").is_dir()).then_some(nested)
}

/// Numeric folders under `<root>/userdata`.
pub fn account_ids(steam_root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(steam_root.join("userdata")) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && n != "0")
        .collect();
    ids.sort();
    ids
}

fn read_settings(steam_root: &Path, account_id: &str) -> BTreeMap<String, String> {
    let path = localconfig_path(steam_root, account_id);
    match std::fs::read_to_string(&path) {
        Ok(text) => game_recording_block(&text).map(parse_game_recording).unwrap_or_default(),
        Err(_) => BTreeMap::new(),
    }
}

/// All accounts that have recordings. With an override, only that folder is used.
pub fn discover_accounts(roots: &[PathBuf], override_root: Option<&Path>) -> Vec<SteamAccount> {
    let mut found = Vec::new();
    for root in roots {
        for id in account_ids(root) {
            let settings = read_settings(root, &id);
            let export_dir = settings
                .get("ExportDirectory")
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let recordings_root = custom_recordings_root(&settings).or_else(|| {
                let p = root.join("userdata").join(&id).join("gamerecordings");
                p.is_dir().then_some(p)
            });
            if let Some(recordings_root) = recordings_root {
                found.push(SteamAccount { account_id: id, recordings_root, export_dir });
            }
        }
    }

    if let Some(ov) = override_root {
        let recordings_root = resolve_recordings_root(ov).unwrap_or_else(|| ov.to_path_buf());
        let canon = dunce::canonicalize(&recordings_root).unwrap_or(recordings_root.clone());
        // Keep the account id and export dir when the override is a known account's folder.
        let known = found.iter().find(|a| {
            dunce::canonicalize(&a.recordings_root).map(|c| path_key(&c)).ok() == Some(path_key(&canon))
        });
        return vec![SteamAccount {
            account_id: known
                .map(|a| a.account_id.clone())
                .or_else(|| account_id_from_path(&canon))
                .unwrap_or_else(|| "custom".into()),
            recordings_root: canon,
            export_dir: known.and_then(|a| a.export_dir.clone()),
        }];
    }

    let mut seen = HashSet::new();
    found.retain(|a| {
        let key = dunce::canonicalize(&a.recordings_root).unwrap_or(a.recordings_root.clone());
        seen.insert(path_key(&key))
    });
    found
}

/// `.../userdata/<id>/gamerecordings` → `<id>`.
fn account_id_from_path(p: &Path) -> Option<String> {
    let parent = p.parent()?;
    let id = parent.file_name()?.to_str()?;
    let is_userdata = parent.parent()?.file_name()?.to_str()?.eq_ignore_ascii_case("userdata");
    (is_userdata && id.chars().all(|c| c.is_ascii_digit())).then(|| id.to_owned())
}

/// Library folders listed in `<root>/steamapps/libraryfolders.vdf`, plus the root itself.
pub fn library_paths(steam_root: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam_root.to_path_buf()];
    let vdf_path = steam_root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(&vdf_path) {
        out.extend(parse_library_folders(&text));
    }
    out
}

pub fn parse_library_folders(text: &str) -> Vec<PathBuf> {
    let Ok(vdf) = keyvalues_parser::parse(text) else {
        log::warn!("cannot parse libraryfolders.vdf");
        return Vec::new();
    };
    let Some(obj) = vdf.value.get_obj() else { return Vec::new() };
    obj.values()
        .flatten()
        .filter_map(VdfValue::get_obj)
        .filter_map(|lib| obj_str(lib, "path"))
        .map(PathBuf::from)
        .collect()
}

fn obj_str<'a>(obj: &'a Obj<'_>, key: &str) -> Option<&'a str> {
    obj.get(key)?.first()?.get_str()
}

/// `"name"` from an `appmanifest_<appid>.acf`.
pub fn parse_app_manifest(text: &str) -> Option<(u32, String)> {
    let vdf = keyvalues_parser::parse(text).ok()?;
    let obj = vdf.value.get_obj()?;
    let appid = obj_str(obj, "appid")?.parse().ok()?;
    Some((appid, obj_str(obj, "name")?.to_owned()))
}

pub fn read_game_names(roots: &[PathBuf]) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    let mut seen = HashSet::new();
    for lib in roots.iter().flat_map(|r| library_paths(r)) {
        let steamapps = lib.join("steamapps");
        let key = dunce::canonicalize(&steamapps).unwrap_or(steamapps.clone());
        if !seen.insert(path_key(&key)) {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&steamapps) else { continue };
        for entry in rd.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !(file_name.starts_with("appmanifest_") && file_name.ends_with(".acf")) {
                continue;
            }
            match std::fs::read_to_string(entry.path()).ok().as_deref().and_then(parse_app_manifest) {
                Some((appid, name)) => {
                    names.entry(appid).or_insert(name);
                }
                None => log::warn!("cannot read {}", entry.path().display()),
            }
        }
    }
    names
}

static NAME_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// appid → game name, cached in memory. `refresh` re-reads the manifests.
pub fn game_names(roots: &[PathBuf], refresh: bool) -> HashMap<u32, String> {
    let mut cache = NAME_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if refresh || cache.is_none() {
        *cache = Some(read_game_names(roots));
    }
    cache.clone().unwrap_or_default()
}

pub fn game_name(names: &HashMap<u32, String>, appid: u32) -> String {
    names.get(&appid).cloned().unwrap_or_else(|| format!("App {appid}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> String {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).unwrap()
    }

    #[test]
    fn game_recording_settings() {
        let text = fixture("localconfig.vdf");
        let block = game_recording_block(&text).unwrap();
        assert!(block.starts_with("\"GameRecording\""));
        assert!(block.ends_with('}'));
        let s = parse_game_recording(block);
        assert_eq!(s["ExportDirectory"], "/home/user/Videos");
        assert_eq!(s["BackgroundMaxKeep"], "300min");
        assert_eq!(s["InstantClipKey"], "Ctrl\tKEY_F11");
        assert!(!s.contains_key("PersonaName"));
        assert_eq!(custom_recordings_root(&s), None);
    }

    #[test]
    fn custom_root_heuristic() {
        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("gamerecordings");
        fs::create_dir_all(rec.join("video")).unwrap();
        let mut s = BTreeMap::new();
        s.insert("ExportDirectory".to_owned(), dir.path().display().to_string());
        assert_eq!(custom_recordings_root(&s), None);
        s.insert("BackgroundRecordPath".to_owned(), dir.path().display().to_string());
        assert_eq!(custom_recordings_root(&s), Some(rec.clone()));
        s.insert("BackgroundRecordPath".to_owned(), rec.display().to_string());
        assert_eq!(custom_recordings_root(&s), Some(rec));
        s.insert("BackgroundRecordPath".to_owned(), "/does/not/exist".to_owned());
        assert_eq!(custom_recordings_root(&s), None);
    }

    #[test]
    fn libraries_and_manifests() {
        let libs = parse_library_folders(&fixture("libraryfolders.vdf"));
        assert_eq!(libs, vec![PathBuf::from("/home/user/.local/share/Steam"), PathBuf::from(r"D:\SteamLibrary")]);
        assert_eq!(parse_app_manifest(&fixture("appmanifest_1445790.acf")), Some((1445790, "Make Way".into())));
        assert_eq!(game_name(&HashMap::new(), 42), "App 42");
    }

    #[test]
    fn accounts_from_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Steam");
        let ud = root.join("userdata");
        fs::create_dir_all(ud.join("111/gamerecordings/video")).unwrap();
        fs::create_dir_all(ud.join("111/config")).unwrap();
        fs::write(ud.join("111/config/localconfig.vdf"), fixture("localconfig.vdf")).unwrap();
        fs::create_dir_all(ud.join("222/config")).unwrap(); // no recordings
        fs::create_dir_all(ud.join("ac")).unwrap(); // not numeric
        fs::create_dir_all(root.join("steamapps")).unwrap();
        fs::write(root.join("steamapps/appmanifest_1445790.acf"), fixture("appmanifest_1445790.acf")).unwrap();

        let checks = check_roots(&[root.clone(), root.clone(), dir.path().join("nope")]);
        let roots = steam_roots(&checks);
        assert_eq!(roots.len(), 1);
        assert!(!checks[2].exists);

        let accounts = discover_accounts(&roots, None);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].account_id, "111");
        assert_eq!(accounts[0].export_dir, Some(PathBuf::from("/home/user/Videos")));

        let ov = discover_accounts(&roots, Some(&ud.join("111")));
        assert_eq!(ov.len(), 1);
        assert_eq!(ov[0].account_id, "111");
        assert!(ov[0].recordings_root.ends_with("gamerecordings"));

        let names = read_game_names(&roots);
        assert_eq!(names.get(&1445790).map(String::as_str), Some("Make Way"));
    }
}
