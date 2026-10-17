fn main() {
    // tauri-build embeds its Common-Controls v6 manifest into the app binary only, so `cargo test`
    // binaries fail on Windows with STATUS_ENTRYPOINT_NOT_FOUND (comctl32 v6 imports such as
    // TaskDialogIndirect). Embed it ourselves through linker args, which reach every target.
    let attrs = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attrs).expect("failed to run tauri-build");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        // Fail the build instead of silently linking without the manifest.
        println!("cargo:rustc-link-arg=/WX");
    }
}
