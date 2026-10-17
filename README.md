# SteamReplay

Finds the good moments in your **Steam Game Recording** background recordings and exports them as MP4 clips.

- A library of Steam's background recordings and saved clips, with thumbnails, search and game groups.
- An editor-style workspace: a large video preview with volume control, the audio waveform with Steam's markers across the bottom (hover it to see the video at that point), and the loud parts (laughing, shouting, cheering) ranked in a side panel. Detection can be tuned per game.
- Set in and out points and export either **fast** (stream copy, cut on 3 s chunk boundaries) or **precise** (frame-accurate re-encode to H.264, HEVC or AV1, with size presets such as Discord). Exports run in the background while you keep browsing.
- Keyboard-driven (`?` lists every shortcut), with dark and light themes.

Everything runs locally. SteamReplay only reads Steam's folders and never changes them.

<!-- TODO: screenshot (docs/screenshot.png) -->

## Install on Windows

1. Download `SteamReplay_x.y.z_x64-setup.exe` from the [latest release](https://github.com/nicojeske/steamreplay/releases/latest).
2. Run it. It installs for your user only, so it does **not** need admin rights and shows no UAC prompt.
3. **SmartScreen:** Windows may show "Windows protected your PC". Click **More info → Run anyway**. This appears because the installer is not code-signed. It is not a sign of a problem.
4. Start SteamReplay from the Start menu.

ffmpeg comes with the installer. If WebView2 is missing (rare on Windows 10/11), the installer installs it.

If you installed an older test build from a CI artifact, uninstall it first (*Settings → Apps*), because it may be a per-machine install that would sit next to the new one.

Antivirus software may scan the bundled `ffmpeg.exe` the first time it runs. That only makes the first analysis slower.

### Updates
SteamReplay checks for a new version a few seconds after it starts. When there is one, it offers **Install and restart**. If an export is running, the update waits until it is done. You can also check by hand in *Settings → About*, where the installed version is shown too.

## Install on Linux

Download `SteamReplay_x.y.z_amd64.AppImage` from the [latest release](https://github.com/nicojeske/steamreplay/releases/latest), make it executable and run it:

```sh
chmod +x SteamReplay_*.AppImage
./SteamReplay_*.AppImage
```

- It needs a system **ffmpeg** with HEVC, AAC and libx264 (most distro builds have them). The AppImage does not include ffmpeg.
- Without FUSE, run it with `--appimage-extract-and-run`.
- On NVIDIA + Wayland, the app sets `__NV_DISABLE_EXPLICIT_SYNC=1` by itself (avoids the `Error 71` crash and keeps hardware compositing). Elsewhere it sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`. If the window is blank or flickers, start it with `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
- The AppImage updates itself like the Windows build. A `.deb` built from source does not.

## Where files go

| What | Windows | Linux |
|---|---|---|
| Exports (default) | Steam's export folder (*Steam → Settings → Game Recording*), else *Videos*. Can be changed in Settings. | same |
| Caches (waveforms, thumbnails, preview videos) | `%LOCALAPPDATA%\de.steamreplay.app` | `~/.cache/de.steamreplay.app` |
| Settings | `%APPDATA%\de.steamreplay.app` | `~/.local/share/de.steamreplay.app` |
| Logs | `%LOCALAPPDATA%\de.steamreplay.app\logs` | `~/.local/share/de.steamreplay.app/logs` |

You can clear the caches in *Settings → Storage*. Uninstalling does not delete settings or caches.

## Development

Tauri 2 (Rust) with a React and TypeScript frontend, plus ffmpeg as an external process. The design notes are in [`docs/specs/`](docs/specs/00-overview.md).

You need Rust (stable), Node (LTS), pnpm and the [Tauri prerequisites](https://tauri.app/start/prerequisites/). On Linux you also need a system ffmpeg.

```sh
pnpm install
pnpm fetch-ffmpeg     # Windows only: downloads the bundled ffmpeg sidecar
pnpm tauri dev
```

Checks (CI runs the same ones):

```sh
cd src-tauri && cargo clippy --all-targets -- -D warnings && cargo test && cd ..
pnpm lint
pnpm tsc --noEmit
```

`cargo test` regenerates `src/bindings.ts`. Commit it when it changes.

Every push to `main` builds the Windows installer and the Linux bundles as workflow artifacts (`.github/workflows/build.yml`).

## Releasing

1. `pnpm bump 0.3.0`: sets the version in `package.json`, `Cargo.toml` and `Cargo.lock`, commits `release: v0.3.0` and tags `v0.3.0`.
2. `git push --follow-tags`: the `release` workflow builds a **draft** release with the installer, the AppImage, their `.sig` files and `latest.json`. A tag that does not match `package.json` fails the build. Tags with a suffix (`v0.3.0-rc.1`) become pre-releases.
3. Check the draft (install it once), then **publish** it. Installed apps only see published releases.

**Updater signing key.** Updates are signed with a private key stored in the repo secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The public key is in `src-tauri/tauri.conf.json`. **Keep a backup of the private key.** If it is lost, installed copies can never be updated again; everyone would have to reinstall by hand. This key is not Windows code signing. It only proves that an update comes from this repo.

## Licence

SteamReplay is licensed under the [GNU General Public License v3.0](LICENSE).

The Windows build bundles a GPL build of ffmpeg from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds). See `src-tauri/resources/ffmpeg/NOTICE.txt` (installed next to the app) for the version, licence and source.
