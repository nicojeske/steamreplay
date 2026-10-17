# Windows test checklist

Run this on a Windows machine that has never had SteamReplay installed (or uninstall it first). You need a few Steam background recordings on it. Takes about 20 minutes. Write your results into the table at the end and send it back (or open a PR).

## Steps

1. **Install.** Download the setup `.exe` from the [latest release](https://github.com/nicojeske/steamreplay/releases/latest) and run it. SmartScreen shows "Windows protected your PC". *More info → Run anyway* works. There is **no** UAC (admin) prompt.
2. **Start menu.** A *SteamReplay* entry with the blue waveform/play icon is in the Start menu. The same icon shows in the taskbar and the window title.
3. **Recordings found.** The list shows your background recordings and clips with game names, dates and lengths. If not: *Settings → Copy diagnostics* and send that along.
4. **Waveform and loud parts.** Open a recording. The waveform loads, Steam markers show on it, the loud-parts list is filled, and the tuning sliders change it.
5. **Preview.** The video plays and stays in sync with the timeline when you click around. Note the backend shown in *Settings → Video preview* (e.g. "NVIDIA (CUDA / NVENC)").
6. **Fast export.** Set in and out points (`I` / `O`), export in *Fast* mode. The file plays in VLC.
7. **Precise export.** Export the same range with the *Discord* preset. It plays in *Films & TV* (Windows media player) and as an upload in Discord.
8. **Still-recording session.** Start a game with background recording on, then press *Refresh*. The new session shows up. Export the last few seconds of it: the export works (the newest few seconds, which Steam is still writing, are left out).
9. **Version.** *Settings → About* shows the version from the release page.
10. **Update.** Install the *previous* release first, start it, wait a few seconds. It offers "SteamReplay x.y.z is available". *Install and restart* ends on the new version (step 9), and your settings (e.g. export folder, loud-part tuning) are still there.
11. **Uninstall.** *Settings → Apps → SteamReplay → Uninstall*. The Start menu entry and `%LOCALAPPDATA%\SteamReplay` are gone. Only the app data under `%APPDATA%\de.steamreplay.app` and `%LOCALAPPDATA%\de.steamreplay.app` may stay.

## Results

| Tester | GPU / driver | Windows version | Preview backend | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | | | | | | |
