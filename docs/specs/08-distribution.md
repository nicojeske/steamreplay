# M8: Distribution: installer, updates, releases

## 1. Goal and user-visible outcome
A teammate downloads one installer from the GitHub Releases page, installs it without admin rights, and gets a SteamReplay entry with a real icon in the Start menu. After that, new versions arrive by themselves: the app says "Version x.y.z is available" and installs it with one click. The version is shown in the app. Linux gets an AppImage that also updates itself. A README explains installation, including the SmartScreen warning, because the build is not signed.

## 2. Prerequisites
- M1–M7 done (this milestone is last on purpose).
- **The user makes the repo `nicojeske/steamreplay` public before this milestone.** The updater downloads `latest.json` and the installers from GitHub Releases without a token, which only works on a public repo. Before making it public:
  - run `gitleaks detect` (or `trufflehog git file://.`) over the whole history and fix anything it finds,
  - check that no sample recordings or personal paths are in the fixtures beyond what `00-overview.md` already shows.
- The ffmpeg licence handling from M2 is already in place: `src-tauri/resources/ffmpeg/NOTICE.txt`, the generated `LICENSE.txt`, and `scripts/fetch-ffmpeg.ts` with a pinned GPL build. A public repo that ships GPL ffmpeg binaries is fine with this NOTICE (source link included).
- `00-overview.md`: the CI and platform rules.

## 3. Scope / non-goals
**In scope:**
- app icon,
- NSIS settings,
- a single source for the version,
- updater with a signing key,
- release workflow on `v*` tags,
- version in the UI,
- `README.md` and `docs/windows-test.md`,
- Linux AppImage in releases.

**Non-goals:**
- **Code signing.** There is no certificate. Document SmartScreen instead, and leave a hook in CI for later.
- MSI installer,
- Microsoft Store,
- winget or Flatpak,
- an Arch PKGBUILD (optional, only if there is time left),
- macOS,
- telemetry or crash reporting.

## 4. Design

### 4.1 Current state
- `src-tauri/tauri.conf.json`:
  - `version: "0.1.0"`, `identifier: "de.steamreplay.app"`,
  - `bundle.targets: "all"` with the **default Tauri icons**,
  - no `windows`/`nsis`, `plugins` or updater settings.
- `src-tauri/tauri.windows.conf.json` adds the ffmpeg/ffprobe sidecars and the licence resources.
- `.github/workflows/build.yml`:
  - builds `nsis` on `windows-latest` and `deb,appimage` on `ubuntu-22.04`, on every push and PR,
  - uploads them as workflow artifacts,
  - publishes no releases.
- The version `0.1.0` is set separately in `tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`.
- The UI shows no app version anywhere. The only version shown is ffmpeg's, in `SettingsDialog.tsx:96`.

### 4.2 Icon
- Add a source `src-tauri/app-icon.svg`, a simple flat waveform with a play triangle that stays readable at 16 px.
- Generate every size with `pnpm tauri icon src-tauri/app-icon.svg`. This overwrites `src-tauri/icons/*`. Commit both the SVG and the generated files.
- Use the same icon as the favicon in `index.html`.

### 4.3 Bundle settings (`tauri.conf.json`)
```jsonc
"version": "../package.json",              // single source of truth
"bundle": {
  "publisher": "SteamReplay",
  "shortDescription": "Find and export the loud moments in Steam game recordings",
  "copyright": "…",
  "createUpdaterArtifacts": true,
  "windows": {
    "webviewInstallMode": { "type": "embedBootstrapper" },
    "nsis": { "installMode": "currentUser", "startMenuFolder": "SteamReplay", "installerIcon": "icons/icon.ico" }
  },
  "linux": { "appimage": { "bundleMediaFramework": true } }
}
```
- `installMode: currentUser` installs under `%LOCALAPPDATA%` with no UAC prompt.
- The embedded bootstrapper installs WebView2 only on the rare machine that lacks it.
- `bundleMediaFramework` puts the GStreamer plugins into the AppImage. The MSE preview (M4) needs them, so they cannot be left to the host. This makes the AppImage much bigger. Check that the preview still plays from the AppImage on CachyOS (see risks).
- **Version bump:** add a `pnpm bump <x.y.z>` script (`scripts/bump.ts`, Node type-stripping like `fetch-ffmpeg.ts`). It updates `package.json` and `src-tauri/Cargo.toml`, refreshes `Cargo.lock` (`cargo update -p steamreplay --offline`), commits as `release: vX.Y.Z` and tags `vX.Y.Z`. It does not push; the user pushes.

### 4.4 Updater
- Dependencies: `tauri-plugin-updater` and `tauri-plugin-process` (Rust and JS packages). Register them in `src-tauri/src/lib.rs` next to the other plugins. The updater plugin is desktop-only: `#[cfg(desktop)]` / `.setup` per the plugin docs.
- Capabilities (`src-tauri/capabilities/default.json`): `updater:default` and `process:allow-restart`.
- Config:
  ```jsonc
  "plugins": { "updater": {
      "pubkey": "<contents of the .pub file>",
      "endpoints": ["https://github.com/nicojeske/steamreplay/releases/latest/download/latest.json"],
      "windows": { "installMode": "passive" } } }
  ```
- **Signing key:** generate it with `pnpm tauri signer generate -w ~/.tauri/steamreplay.key`. The private key and its password go into the repo secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; the public key goes into the config. **The user keeps a backup of the private key.** Without it, installed apps can never be updated again.
  - Updater signing is **not** Windows code signing. It only proves that the update came from us.
- **UI** (`src/lib/updater.ts` + `Toasts.tsx`):
  - 5 s after startup, call `check()` once in the background. Errors (offline, rate limit) are only logged, never shown.
  - If an update exists: a persistent toast "SteamReplay x.y.z is available — Install and restart / Later". Install calls `downloadAndInstall` with a progress bar in the toast, then `relaunch()`.
  - If an export is running, Install waits until the job queue is empty.
  - The settings dialog shows "Version 0.x.y" (`getVersion()` from `@tauri-apps/api/app`) and a "Check for updates" button, which shows "up to date" or the same prompt.
  - "Later" hides the toast until the next app start.
- The `.deb` cannot update itself. The updater only covers NSIS and AppImage; `check()` is skipped on Linux when `APPIMAGE` is not set.

### 4.5 Release workflow (`.github/workflows/release.yml`, new)
- Trigger: `push: tags: ['v*']`. Permissions: `contents: write`.
- Matrix: the same two OS entries as `build.yml`. Windows builds `nsis` and Linux builds `appimage` (`deb` is optional as an extra asset).
- Steps:
  1. Copy from `build.yml`: checkout, pnpm/node/rust setup, Linux apt deps, `pnpm install --frozen-lockfile`, and the Windows ffmpeg sidecar cache plus `pnpm fetch-ffmpeg`.
  2. **Check the version:** the tag without its `v` must equal `package.json`'s `version`, otherwise fail.
  3. `tauri-apps/tauri-action@v0` with `tagName: ${{ github.ref_name }}`, `releaseName: SteamReplay ${{ github.ref_name }}`, `releaseDraft: true`, `includeUpdaterJson: true`, and `args: --bundles <matrix>`. The env gets `GITHUB_TOKEN` and the two `TAURI_SIGNING_*` secrets.
  4. A placeholder comment block "Windows code signing (Azure Trusted Signing / signtool) goes here", for later.
- A draft release is only published by hand after a quick check. Once it is published, `releases/latest/download/latest.json` serves it to the updater.
- `build.yml` stays as it is for pushes and PRs. Don't build twice on a tag push: `build.yml` already ignores tags, because it only triggers on `branches: [main]`.

### 4.6 Docs
- **`README.md`** (new):
  - what the tool does, with a screenshot,
  - Windows install from Releases, with the **SmartScreen** steps: "Windows protected your PC → More info → Run anyway". Explain that this happens because the build is unsigned, and only on the first install,
  - updates (automatic),
  - Linux: the AppImage, required `ffmpeg` with libx264, and the env var hint for NVIDIA (see risks),
  - where exports go and where the caches live,
  - development setup (`pnpm install`, `pnpm fetch-ffmpeg` on Windows, `pnpm tauri dev`) and the checks,
  - the release process (`pnpm bump`, push the tag, check the draft, publish),
  - licence and the ffmpeg NOTICE.
- **`docs/windows-test.md`** (new), a checklist a teammate can follow on a clean machine:
  1. install from Releases (SmartScreen, no UAC prompt),
  2. Start-menu entry and icon,
  3. recordings are found,
  4. waveform and spikes,
  5. preview plays, and the detected backend is noted,
  6. fast export plays in VLC,
  7. precise Discord preset plays in Films & TV and Discord,
  8. live session grows,
  9. settings: the version is shown,
  10. update from the previous release to the current one,
  11. uninstall leaves no files behind except the app data, if chosen.
  It has a table for results: name, GPU, driver, Windows version, backend, pass/fail per step.
- Update `00-overview.md` "Windows builds" to mention Releases and the updater.

## 5. Edge cases
- **No network or GitHub rate limit:** the update check fails silently, and the manual check shows "could not check".
- **The update is downloaded while an export or preview transcode runs:** install only after the user confirms and the export queue is empty. Kill the preview ffmpeg processes on exit (this already happens through `kill_on_drop`).
- **Per-user install, but an older per-machine install exists** (from earlier CI artifacts): the NSIS currentUser installer installs side by side. The README says to uninstall the old one first.
- **Antivirus or Defender flags the unsigned installer or the ffmpeg sidecar:** documented in the README. A Defender scan on the first start only slows things down. Make sure the startup ffmpeg check does not block the UI.
- **Lost signing key:** see 4.4. Document it in the README's release section.
- **AppImage on a system without FUSE:** README hint `--appimage-extract-and-run`.

## 6. Acceptance criteria
- [ ] The repo is public and the secret scan is clean (done by the user and noted here).
- [ ] The custom icon shows in the installer, the Start menu, the taskbar and the window title (Windows), and in the AppImage desktop entry.
- [ ] Pushing tag `v0.2.0` gives a draft release with the NSIS `.exe`, the `.AppImage`, their `.sig` files and `latest.json`. A tag that does not match `package.json` fails the workflow.
- [ ] **Windows:** the installer runs on a clean teammate machine without admin rights (no UAC prompt), and SmartScreen "Run anyway" works as the README describes.
- [ ] **Windows:** with `v0.2.0` installed, publishing `v0.2.1` makes the app offer the update. "Install and restart" ends on 0.2.1, with settings and caches kept.
- [ ] The version is shown in the settings dialog and matches the release.
- [ ] The Linux AppImage runs on the dev machine (CachyOS, NVIDIA): sessions list, waveform, preview playback and export all work.
- [ ] `docs/windows-test.md` has been run by at least one teammate, with the results filled in.
- [ ] clippy, tests, tsc and lint are clean; CI is green.

## 7. Verification steps
- **Locally:**
  - `pnpm tauri build --bundles appimage` on CachyOS, then run the AppImage,
  - `pnpm bump 0.2.0` on a scratch branch to check the version files (then discard it),
  - `tauri.conf.json` is valid with `pnpm tauri info`.
- **CI:**
  - push a tag `v0.2.0-rc.1` (the version check allows pre-release suffixes when `package.json` matches) and check the draft release assets and `latest.json` contents: URLs and signatures,
  - delete the draft afterwards.
- **Update round trip:** install rc.1 on a Windows VM or a teammate machine, publish rc.2 (as a non-draft pre-release; temporarily point the endpoint at that release's `latest.json`, or use a real release), and check that the app updates itself.
- **Windows:** a teammate runs `docs/windows-test.md`.

## 8. Open questions / risks
- **WebKitGTK in an AppImage on Arch/NVIDIA** often shows a blank window or crashes with DMA-BUF. If so, set `WEBKIT_DISABLE_DMABUF_RENDERER=1` in the app before the webview starts (Linux only, `lib.rs`), and document it. Also check that the bundled GStreamer plugins play the MSE preview. The AppImage is built on Ubuntu 22.04, which has an older GStreamer than CachyOS.
- **SmartScreen reputation:** unsigned builds warn on every new version until they build up reputation. If the team gets annoyed, look at Azure Trusted Signing (about $10/month), using the CI hook from 4.5.
- **`tauri-action` and the `../package.json` version path:** check that the action reads the resolved version. If not, keep the version in `tauri.conf.json` and let `pnpm bump` update it too.
- **Public repo:** the specs contain the dev machine's account id and paths (`00-overview.md`, sample data). Decide whether to redact them before going public.
