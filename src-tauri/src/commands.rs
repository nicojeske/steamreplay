//! Thin `#[tauri::command]` wrappers. The work happens in `steam/` and `media/`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use specta::Type;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_store::StoreExt;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::{self, FfmpegPaths};
use crate::media::pyramid::Samples;
use crate::media::spikes::{self, Spike, SpikeParams};
use crate::media::preview::{self, Backend, PreviewCache, PreviewInfo, PreviewState, SessionSetup};
use crate::media::export::{self, ExportRequest, ExportResult, Snap};
use crate::media::{self, Analysis, MediaState, Progress, WaveformMeta, chunks};
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

// --- preview (M4) -------------------------------------------------------------------------------

const PREVIEW_BACKEND_KEY: &str = "previewBackend";
const PREVIEW_DETECTED_KEY: &str = "previewBackendDetected";
const PREVIEW_CACHE_KEY: &str = "previewCacheMb";

fn read_setting<T: serde::de::DeserializeOwned>(app: &AppHandle, key: &str) -> Option<T> {
    let store = app.store(SETTINGS_STORE).ok()?;
    serde_json::from_value(store.get(key)?).ok()
}

fn write_setting<T: Serialize>(app: &AppHandle, key: &str, value: Option<T>) -> AppResult<()> {
    let store = app.store(SETTINGS_STORE).map_err(|e| AppError::Store(e.to_string()))?;
    match value.map(serde_json::to_value) {
        Some(Ok(v)) => store.set(key, v),
        Some(Err(e)) => return Err(AppError::Store(e.to_string())),
        None => {
            store.delete(key);
        }
    }
    store.save().map_err(|e| AppError::Store(e.to_string()))
}

/// Builds the preview state at startup: cache folder and size limit from the settings.
pub fn init_preview(app: &AppHandle) -> AppResult<PreviewState> {
    let root = app.path().app_cache_dir().map_err(|e| AppError::Io(format!("no cache folder: {e}")))?;
    let limit = read_setting(app, PREVIEW_CACHE_KEY).unwrap_or(preview::cache::DEFAULT_LIMIT_MB);
    Ok(PreviewState::new(PreviewCache::new(root.join("steamreplay").join("preview"), limit)))
}

/// The settings override, else the detected backend, else detects it now (a 1-chunk transcode
/// per candidate; the first CUDA start can take a few seconds).
async fn preview_backend(
    app: &AppHandle,
    preview: &PreviewState,
    paths: &FfmpegPaths,
    detail: &SessionDetail,
) -> Backend {
    if let Some(b) = read_setting::<Backend>(app, PREVIEW_BACKEND_KEY) {
        return b;
    }
    if let Some(b) = *preview.detected.lock().expect("detected lock") {
        return b;
    }
    if let Some(b) = read_setting::<Backend>(app, PREVIEW_DETECTED_KEY) {
        *preview.detected.lock().expect("detected lock") = Some(b);
        return b;
    }
    let cancel = CancellationToken::new();
    let b = preview::transcode::detect(paths, &detail.summary.dir, detail.chunks.first, &preview.cache.tmp_dir(), &cancel).await;
    *preview.detected.lock().expect("detected lock") = Some(b);
    if let Err(e) = write_setting(app, PREVIEW_DETECTED_KEY, Some(b)) {
        log::warn!("could not store the preview backend: {e}");
    }
    b
}

/// Opens the preview of a session and starts preparing the first segments.
#[tauri::command]
#[specta::specta]
pub async fn get_preview_info(
    app: AppHandle,
    media: State<'_, MediaState>,
    preview: State<'_, PreviewState>,
    id: String,
) -> AppResult<PreviewInfo> {
    let detail = {
        let (app, id) = (app.clone(), id.clone());
        blocking(move || session(&app, &id)).await?
    };
    if detail.chunks.count == 0 {
        return Err(AppError::NotFound(format!("{id} has no video chunks")));
    }
    if let Some(s) = preview.get(&id)
        && s.inv == detail.chunks
    {
        return Ok(s.info());
    }
    let paths = ffmpeg_paths(&app, &media).await?;
    let backend = preview_backend(&app, &preview, &paths, &detail).await;
    let dir = detail.summary.dir.clone();
    let v0 = chunks::probe_start(&paths, &dir, chunks::VIDEO_INIT, &chunks::video_chunk(detail.chunks.first), &CancellationToken::new())
        .await?;
    {
        let (cache, id, first) = (preview.cache.clone(), id.clone(), detail.chunks.first);
        blocking(move || {
            cache.remove_stale_sessions(&id, first);
            Ok(())
        })
        .await?;
    }
    let chunk_s = detail.mpd.as_ref().map_or(3.0, |m| m.segment_duration_s);
    let session = preview.open(SessionSetup { id, dir, inv: detail.chunks, chunk_s, v0, paths, backend });
    session.set_playhead(0.0);
    Ok(session.info())
}

/// Reorders the preview queue around the playhead (video time in seconds).
#[tauri::command]
#[specta::specta]
pub async fn preview_playhead(preview: State<'_, PreviewState>, id: String, t: f64) -> AppResult<()> {
    if let Some(s) = preview.get(&id) {
        s.set_playhead(t);
    }
    Ok(())
}

/// Stops the session's preview jobs.
#[tauri::command]
#[specta::specta]
pub async fn preview_close(preview: State<'_, PreviewState>, id: String) -> AppResult<()> {
    preview.close(&id);
    Ok(())
}

#[derive(Debug, Clone, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSettings {
    /// Chosen in the settings; `None` means detect automatically.
    pub backend: Option<Backend>,
    pub detected: Option<Backend>,
    pub cache_mb: u32,
    #[specta(type = specta_typescript::Number)]
    pub cache_bytes: u64,
}

#[tauri::command]
#[specta::specta]
pub async fn get_preview_settings(app: AppHandle, preview: State<'_, PreviewState>) -> AppResult<PreviewSettings> {
    let cache = preview.cache.clone();
    let cache_bytes = blocking(move || Ok(cache.size())).await?;
    let detected = *preview.detected.lock().expect("detected lock");
    Ok(PreviewSettings {
        backend: read_setting(&app, PREVIEW_BACKEND_KEY),
        detected: detected.or_else(|| read_setting(&app, PREVIEW_DETECTED_KEY)),
        cache_mb: (preview.cache.limit_bytes() / 1024 / 1024) as u32,
        cache_bytes,
    })
}

/// `backend: None` returns to automatic detection, which runs again the next time a preview
/// opens.
#[tauri::command]
#[specta::specta]
pub async fn set_preview_settings(
    app: AppHandle,
    preview: State<'_, PreviewState>,
    backend: Option<Backend>,
    cache_mb: u32,
) -> AppResult<()> {
    let cache_mb = cache_mb.max(100);
    write_setting(&app, PREVIEW_BACKEND_KEY, backend)?;
    write_setting(&app, PREVIEW_CACHE_KEY, Some(cache_mb))?;
    match backend {
        Some(b) => preview.set_backend(b),
        None => {
            *preview.detected.lock().expect("detected lock") = None;
            write_setting::<Backend>(&app, PREVIEW_DETECTED_KEY, None)?;
        }
    }
    preview.cache.set_limit_mb(cache_mb);
    let cache = preview.cache.clone();
    blocking(move || {
        cache.evict();
        Ok(())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn clear_preview_cache(preview: State<'_, PreviewState>) -> AppResult<()> {
    let cache = preview.cache.clone();
    blocking(move || {
        cache.clear();
        Ok(())
    })
    .await
}

// --- export (M3) --------------------------------------------------------------------------------

fn export_cache_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let dir = app.path().app_cache_dir().map_err(|e| AppError::Io(format!("no cache folder: {e}")))?;
    Ok(dir.join("steamreplay").join("export"))
}

/// Removes job folders left behind by a crash.
pub fn clean_export_cache(app: &AppHandle) {
    if let Ok(dir) = export_cache_dir(app) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// `session.mpd` changed in the last few seconds: the game is still recording.
fn is_live(detail: &SessionDetail) -> bool {
    std::fs::metadata(detail.summary.dir.join("session.mpd"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < export::LIVE_MPD_AGE_S)
}

fn snap(detail: &SessionDetail, in_s: f64, out_s: f64) -> AppResult<Snap> {
    let seg = detail.mpd.as_ref().map_or(3.0, |m| m.segment_duration_s);
    export::chunk_range(&detail.chunks, seg, in_s, out_s, is_live(detail), detail.summary.size_bytes)
}

/// The account's `ExportDirectory`, else the system video folder, else `~/Videos`.
fn export_dir(app: &AppHandle, account_id: &str) -> AppResult<PathBuf> {
    let info = discover(app)?;
    if let Some(d) = info.accounts.into_iter().find(|a| a.account_id == account_id).and_then(|a| a.export_dir) {
        return Ok(d);
    }
    app.path()
        .video_dir()
        .or_else(|_| app.path().home_dir().map(|h| h.join("Videos")))
        .map_err(|e| AppError::Io(format!("no export folder: {e}")))
}

/// Where the selection is cut, plus an estimated file size.
#[tauri::command]
#[specta::specta]
pub async fn export_snap(app: AppHandle, id: String, in_s: f64, out_s: f64) -> AppResult<Snap> {
    blocking(move || snap(&session(&app, &id)?, in_s, out_s)).await
}

/// `<export folder>/<Game>_<date>.mp4` for the snapped in point, made unique.
#[tauri::command]
#[specta::specta]
pub async fn default_export_path(app: AppHandle, id: String, in_s: f64) -> AppResult<PathBuf> {
    blocking(move || {
        let detail = session(&app, &id)?;
        let s = snap(&detail, in_s, in_s)?;
        let dir = export_dir(&app, &detail.summary.account_id)?;
        let name = export::default_file_name(&detail.summary.game_name, &detail.summary.started_at, s.actual_in_s);
        Ok(export::dedupe(&dir.join(name)))
    })
    .await
}

/// Exports the selection with stream copy. `job_id` is chosen by the frontend for `cancel_job`.
#[tauri::command]
#[specta::specta]
pub async fn export_clip(
    app: AppHandle,
    state: State<'_, MediaState>,
    req: ExportRequest,
    job_id: String,
    progress: Channel<Progress>,
) -> AppResult<ExportResult> {
    let job = state.start_job(&job_id);
    let (detail, snap, out) = {
        let (app, req) = (app.clone(), req.clone());
        blocking(move || {
            let detail = session(&app, &req.id)?;
            let s = snap(&detail, req.in_s, req.out_s)?;
            let out = match req.output {
                Some(p) => p,
                None => {
                    let dir = export_dir(&app, &detail.summary.account_id)?;
                    let name =
                        export::default_file_name(&detail.summary.game_name, &detail.summary.started_at, s.actual_in_s);
                    export::dedupe(&dir.join(name))
                }
            };
            Ok((detail, s, out))
        })
        .await?
    };
    let paths = ffmpeg_paths(&app, &state).await?;
    let job_dir = export_cache_dir(&app)?.join(&job_id);
    let seg = detail.mpd.as_ref().map_or(3.0, |m| m.segment_duration_s);
    let title = format!("{} {}", detail.summary.game_name, detail.summary.started_at);
    let on_progress: ffmpeg::ProgressFn = Arc::new(move |fraction| {
        let _ = progress.send(Progress { fraction });
    });
    let started = std::time::Instant::now();
    let size_bytes =
        export::export(&paths, &detail.summary.dir, &snap, seg, &title, &out, &job_dir, on_progress, &job.token).await?;
    log::info!("exported {} chunks to {} in {:?}", snap.chunks(), out.display(), started.elapsed());
    Ok(ExportResult { path: out, actual_in_s: snap.actual_in_s, actual_out_s: snap.actual_out_s, size_bytes })
}
