mod commands;
mod error;
mod media;
mod steam;

use tauri::{Manager, RunEvent};
use tauri_specta::{Builder, collect_commands};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![
        commands::get_discovery_info,
        commands::list_accounts,
        commands::list_sessions,
        commands::get_session,
        commands::set_recordings_root_override,
        commands::get_diagnostics,
        commands::analyze_audio,
        commands::get_waveform,
        commands::get_spikes,
        commands::cancel_job,
        commands::get_ffmpeg_info,
        commands::set_ffmpeg_dir,
        commands::get_preview_info,
        commands::preview_playhead,
        commands::preview_close,
        commands::get_preview_settings,
        commands::set_preview_settings,
        commands::clear_preview_cache,
        commands::export_snap,
        commands::default_export_path,
        commands::export_clip,
    ])
}

#[cfg(debug_assertions)]
fn export_bindings(builder: &Builder<tauri::Wry>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/bindings.ts");
    builder
        .export(specta_typescript::Typescript::default(), path)
        .expect("failed to export TypeScript bindings");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout))
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: None }))
                .build(),
        )
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(media::MediaState::default())
        .setup(|app| {
            app.manage(commands::init_preview(app.handle())?);
            commands::clean_export_cache(app.handle());
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("replay", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                responder.respond(media::preview::protocol::handle(&app, request).await);
            });
        })
        .invoke_handler(builder.invoke_handler())
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                // Kill running preview transcodes; child processes would outlive the app on Windows.
                app.state::<media::preview::PreviewState>().shutdown();
            }
        });
}

#[cfg(all(test, debug_assertions))]
mod tests {
    /// Keeps `src/bindings.ts` current in `cargo test` without starting the app.
    #[test]
    fn export_bindings() {
        super::export_bindings(&super::specta_builder());
    }
}
