//! Thin `#[tauri::command]` wrappers. The work happens in `steam/` and `media/`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_store::StoreExt;

use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::{self, FfmpegPaths};
use crate::media::pyramid::Samples;
use crate::media::spikes::{self, Spike, SpikeParams};
use crate::media::{self, Analysis, MediaState, Progress, WaveformMeta};
use crate::steam::locate::{self, RootCheck, SteamAccount};
use crate::steam::recordings::{self, SessionDetail, SessionSummary};
use crate::steam::diagnostics;

const SETTINGS_STORE: &str = "settings.json";
const OVERRIDE_KEY: &str = "recordingsRootOverride";
const FFMPEG_DIR_KEY: &str = "ffmpegDir";

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryInfo {
    pub checked_roots: Vec<RootCheck>,
    pub steam_roots: Vec<PathBuf>,
    pub accounts: Vec<SteamAccount>,
    pub recordings_root_override: Option<PathBuf>,
}

fn read_path_setting(app: &AppHandle, key: &str) -> AppResult<Option<PathBuf>> {
    let store = app.store(SETTINGS_STORE).map_err(|e| AppError::Store(e.to_string()))?;
    Ok(store
        .get(key)
        .and_then(|v| v.as_str().map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty()))
}

fn discover(app: &AppHandle) -> AppResult<DiscoveryInfo> {
    let recordings_root_override = read_path_setting(app, OVERRIDE_KEY)?;
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

fn session(app: &AppHandle, id: &str) -> AppResult<recordings::SessionDetail> {
    let info = discover(app)?;
    let names = locate::game_names(&info.steam_roots, false);
    recordings::get_session(&info.accounts, &names, id)
}

fn waveform_cache_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app.path().app_cache_dir().map_err(|e| AppError::Io(format!("no cache folder: {e}")))?;
    Ok(dir.join("steamreplay").join("waveform"))
}

async fn ffmpeg_paths(app: &AppHandle, state: &MediaState) -> AppResult<FfmpegPaths> {
    if let Some(p) = state.cached_ffmpeg() {
        return Ok(p);
    }
    let app = app.clone();
    let paths = blocking(move || ffmpeg::locate(read_path_setting(&app, FFMPEG_DIR_KEY)?.as_deref())).await?;
    log::info!("using {} ({})", paths.ffmpeg.display(), paths.version);
    state.set_ffmpeg(Some(paths.clone()));
    Ok(paths)
}

/// The loaded analysis, or the one in the disk cache. Never decodes.
async fn loaded(app: &AppHandle, state: &MediaState, id: &str) -> AppResult<Arc<Analysis>> {
    if let Some(a) = state.get(id, None) {
        return Ok(a);
    }
    let (app, id_owned) = (app.clone(), id.to_owned());
    let a = blocking(move || {
        let detail = session(&app, &id_owned)?;
        media::load_cached(&waveform_cache_dir(&app)?, &detail)
            .ok_or_else(|| AppError::NotFound(format!("{id_owned} has not been analysed yet")))
    })
    .await?;
    let a = Arc::new(a);
    state.put(id, a.clone());
    Ok(a)
}

/// Decodes the audio (or loads it from the cache) and returns the waveform layout.
/// `job_id` is chosen by the frontend, so it can call `cancel_job` while this runs.
#[tauri::command]
#[specta::specta]
pub async fn analyze_audio(
    app: AppHandle,
    state: State<'_, MediaState>,
    id: String,
    job_id: String,
    progress: Channel<Progress>,
) -> AppResult<WaveformMeta> {
    let detail = {
        let (app, id) = (app.clone(), id.clone());
        blocking(move || session(&app, &id)).await?
    };
    let range = (detail.chunks.first, detail.chunks.last);
    if let Some(a) = state.get(&id, Some(range)) {
        return Ok(a.meta());
    }
    let cache_dir = waveform_cache_dir(&app)?;
    let cached = {
        let (dir, d) = (cache_dir.clone(), detail.clone());
        blocking(move || Ok(media::load_cached(&dir, &d))).await?
    };
    let analysis = match cached {
        Some(a) => a,
        None => {
            let paths = ffmpeg_paths(&app, &state).await?;
            let job = state.start_job(&job_id);
            let on_progress: ffmpeg::ProgressFn = Arc::new(move |fraction| {
                let _ = progress.send(Progress { fraction });
            });
            match media::analyze(&paths, &cache_dir, &detail, on_progress.clone(), &job.token).await {
                Err(e @ AppError::Cancelled) => return Err(e),
                Err(e) => {
                    // Steam may have pruned chunks while we read them: retry once with a fresh inventory.
                    log::warn!("audio analysis failed, retrying with a fresh inventory: {e}");
                    let (app, id) = (app.clone(), id.clone());
                    let detail = blocking(move || session(&app, &id)).await?;
                    media::analyze(&paths, &cache_dir, &detail, on_progress, &job.token).await?
                }
                Ok(a) => a,
            }
        }
    };
    let analysis = Arc::new(analysis);
    state.put(&id, analysis.clone());
    Ok(analysis.meta())
}

/// Flat `[min, max, rms]*` for `count` bins of `level` from `start_bin`.
#[tauri::command]
#[specta::specta]
pub async fn get_waveform(
    app: AppHandle,
    state: State<'_, MediaState>,
    id: String,
    level: u8,
    start_bin: u32,
    count: u32,
) -> AppResult<Samples> {
    loaded(&app, &state, &id).await?.pyramid.slice(level.into(), start_bin as usize, count as usize)
}

/// Loud regions, best first. `params` defaults to [`SpikeParams::default`].
#[tauri::command]
#[specta::specta]
pub async fn get_spikes(
    app: AppHandle,
    state: State<'_, MediaState>,
    id: String,
    params: Option<SpikeParams>,
) -> AppResult<Vec<Spike>> {
    let a = loaded(&app, &state, &id).await?;
    blocking(move || Ok(spikes::detect(a.pyramid.level0(), &params.unwrap_or_default()))).await
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_job(state: State<'_, MediaState>, job_id: String) -> AppResult<()> {
    if !state.cancel_job(&job_id) {
        log::info!("cancel_job: {job_id} is not running");
    }
    Ok(())
}

/// The ffmpeg that analysis and export use.
#[tauri::command]
#[specta::specta]
pub async fn get_ffmpeg_info(app: AppHandle, state: State<'_, MediaState>) -> AppResult<FfmpegPaths> {
    ffmpeg_paths(&app, &state).await
}

/// Sets (or clears) the folder that holds ffmpeg and ffprobe.
#[tauri::command]
#[specta::specta]
pub async fn set_ffmpeg_dir(app: AppHandle, state: State<'_, MediaState>, path: Option<PathBuf>) -> AppResult<FfmpegPaths> {
    let store = app.store(SETTINGS_STORE).map_err(|e| AppError::Store(e.to_string()))?;
    match path {
        Some(p) => {
            let found = blocking(move || {
                let found = ffmpeg::locate(Some(&p))?;
                if found.source != ffmpeg::FfmpegSource::Settings {
                    return Err(AppError::NotFound(format!("no working ffmpeg and ffprobe in {}", p.display())));
                }
                Ok((p, found))
            })
            .await;
            let (p, found) = found?;
            store.set(FFMPEG_DIR_KEY, p.to_string_lossy().into_owned());
            store.save().map_err(|e| AppError::Store(e.to_string()))?;
            state.set_ffmpeg(Some(found.clone()));
            Ok(found)
        }
        None => {
            store.delete(FFMPEG_DIR_KEY);
            store.save().map_err(|e| AppError::Store(e.to_string()))?;
            state.set_ffmpeg(None);
            ffmpeg_paths(&app, &state).await
        }
    }
}
