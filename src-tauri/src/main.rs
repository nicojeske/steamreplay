// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    configure_webkit_renderer();
    steamreplay_lib::run()
}

/// Keeps WebKitGTK's DMA-BUF renderer usable where we know how (spec 11 §8).
///
/// On NVIDIA + Wayland the renderer crashes with "Error 71 (Protocol error)":
/// NVIDIA's EGL attaches explicit sync (`wp_linux_drm_syncobj_surface_v1`) to
/// the GTK3 toplevel, then GTK3 commits a plain wl_shm frame on the same surface
/// without an acquire point, which the compositor rejects. Turning off NVIDIA's
/// explicit sync avoids that and keeps zero-copy compositing (5K video: 60 fps
/// instead of ~40). Everywhere else we keep the conservative fallback: no
/// DMA-BUF renderer. Variables the user already set are left alone.
#[cfg(target_os = "linux")]
fn configure_webkit_renderer() {
    use std::env;
    if env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some() {
        return;
    }
    let wayland = env::var_os("WAYLAND_DISPLAY").is_some()
        && !env::var("GDK_BACKEND").is_ok_and(|b| b.starts_with("x11"));
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists();
    // SAFETY: runs first in `main`, before any other thread exists.
    unsafe {
        if wayland && nvidia && env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none() {
            env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        } else if !(wayland && nvidia) {
            env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }
}
