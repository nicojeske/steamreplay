//! Thin `#[tauri::command]` wrappers. The work happens in `steam/`.

use std::path::PathBuf;

use serde::Serialize;
use specta::Type;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

use crate::error::{AppError, AppResult};
use crate::steam::locate::{self, RootCheck, SteamAccount};
use crate::steam::recordings::{self, SessionDetail, SessionSummary};
use crate::steam::diagnostics;

const SETTINGS_STORE: &str = "settings.json";
const OVERRIDE_KEY: &str = "recordingsRootOverride";

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryInfo {
    pub checked_roots: Vec<RootCheck>,
    pub steam_roots: Vec<PathBuf>,
    pub accounts: Vec<SteamAccount>,
    pub recordings_root_override: Option<PathBuf>,
}

fn read_override(app: &AppHandle) -> AppResult<Option<PathBuf>> {
    let store = app.store(SETTINGS_STORE).map_err(|e| AppError::Store(e.to_string()))?;
    Ok(store
        .get(OVERRIDE_KEY)
        .and_then(|v| v.as_str().map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty()))
}

fn discover(app: &AppHandle) -> AppResult<DiscoveryInfo> {
    let recordings_root_override = read_override(app)?;
    let checked_roots = locate::check_roots(&locate::candidate_roots());
    let steam_roots = locate::steam_roots(&checked_roots);
    let accounts = locate::discover_accounts(&steam_roots, recordings_root_override.as_deref());
    Ok(DiscoveryInfo { checked_roots, steam_roots, accounts, recordings_root_override })
}

/// Runs filesystem work off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Io(format!("background task failed: {e}")))?
}

#[tauri::command]
#[specta::specta]
pub async fn get_discovery_info(app: AppHandle) -> AppResult<DiscoveryInfo> {
    blocking(move || discover(&app)).await
}

#[tauri::command]
#[specta::specta]
pub async fn list_accounts(app: AppHandle) -> AppResult<Vec<SteamAccount>> {
    blocking(move || Ok(discover(&app)?.accounts)).await
}

/// All sessions of all accounts, newest first. Also refreshes the game name cache.
#[tauri::command]
#[specta::specta]
pub async fn list_sessions(app: AppHandle) -> AppResult<Vec<SessionSummary>> {
    blocking(move || {
        let info = discover(&app)?;
        let names = locate::game_names(&info.steam_roots, true);
        Ok(recordings::list_sessions(&info.accounts, &names))
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn get_session(app: AppHandle, id: String) -> AppResult<SessionDetail> {
    blocking(move || {
        let info = discover(&app)?;
        let names = locate::game_names(&info.steam_roots, false);
        recordings::get_session(&info.accounts, &names, &id)
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn set_recordings_root_override(app: AppHandle, path: Option<PathBuf>) -> AppResult<()> {
    let store = app.store(SETTINGS_STORE).map_err(|e| AppError::Store(e.to_string()))?;
    match path {
        Some(p) => {
            if !p.is_dir() {
                return Err(AppError::NotFound(format!("{} is not a folder", p.display())));
            }
            store.set(OVERRIDE_KEY, p.to_string_lossy().into_owned());
        }
        None => {
            store.delete(OVERRIDE_KEY);
        }
    }
    store.save().map_err(|e| AppError::Store(e.to_string()))
}

/// Text for the "Copy diagnostics" button.
#[tauri::command]
#[specta::specta]
pub async fn get_diagnostics(app: AppHandle) -> AppResult<String> {
    blocking(move || {
        let info = discover(&app)?;
        let names = locate::game_names(&info.steam_roots, true);
        let sessions = recordings::list_sessions(&info.accounts, &names);
        Ok(diagnostics::report(
            &info.checked_roots,
            &info.steam_roots,
            info.recordings_root_override.as_deref(),
            &info.accounts,
            &sessions,
        ))
    })
    .await
}
