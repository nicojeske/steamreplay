# Windows test checklist

Run this on a Windows machine that has never had SteamReplay installed (or uninstall it first). You need a few Steam background recordings on it. Takes about 25 minutes. Write your results into the table at the end and send it back (or open a PR).

## Steps

1. **Install.** Download the setup `.exe` from the [latest release](https://github.com/nicojeske/steamreplay/releases/latest) and run it. SmartScreen shows "Windows protected your PC". *More info → Run anyway* works. There is **no** UAC (admin) prompt.
2. **Start menu.** A *SteamReplay* entry with the blue waveform/play icon is in the Start menu. The same icon shows in the taskbar and the window title.
3. **Recordings found.** The library on the left shows your background recordings and clips, grouped by game, with thumbnails, dates and lengths. If not: *Settings → About → Copy diagnostics* and send that along.
4. **Waveform and loud moments.** Open a recording. The video can play right away while the timeline shows "Analysing audio…". Then the waveform loads, Steam markers show on it, the *Moments* tab is filled, and the sliders under *Detection* change it.
5. **Preview.** The video plays and stays in sync with the timeline when you click around. Note the backend shown in the *Info* tab (e.g. "NVIDIA (CUDA / NVENC)").
6. **Fast export.** Set in and out points (`I` / `O`), export in *Fast* mode from the *Export* tab (or `Ctrl+E`). A toast says "Exported …" and *Show in folder* opens Explorer. The file plays in VLC.
7. **Precise export.** Export the same range with the *Discord* preset. It plays in *Films & TV* (Windows media player) and as an upload in Discord.
8. **Still-recording session.** Start a game with background recording on, then press *Refresh*. The new session shows up. Export the last few seconds of it: the export works (the newest few seconds, which Steam is still writing, are left out).
9. **Version.** *Settings → About* shows the version from the release page.
10. **Update.** Install the *previous* release first, start it, wait a few seconds. It offers "SteamReplay x.y.z is available". *Install and restart* ends on the new version (step 9), and your settings (e.g. export folder, loud-part tuning) are still there.
11. **Keyboard and layout.** `?` lists the shortcuts; try `Space`, `J`/`K`/`L`, `N`/`P`, `I`/`O`, `Ctrl+B` and `Ctrl+,`, also on a German keyboard (`?` is Shift+ß). Click a slider under *Detection* and press `←`/`→`: the slider moves, the video does not seek. Drag the borders between the panels; the sizes are still there after a restart, and so is the last opened recording.
12. **Exports in the background.** Start a *Precise* export, then open another recording right away. The export keeps running; the *Exports* button in the top bar shows its progress and lists it.
13. **Themes.** *Settings → General → Theme*: Light, Dark and System all work, and the waveform changes colour at once.
14. **Fireshare upload.** *Settings → Fireshare*: enter the server and click *Connect*. The browser opens Fireshare's connect page (sign in with Discord if asked) with the code the app shows. Click *Allow*, and the app shows "Connected as <you>" without any token being typed. Select a range, choose *Export & upload to Fireshare*, and pick the game and a tag. The job shows exporting and then uploading. The toast's *Copy link* gives `…/w/<id>`, which plays in the browser within a minute with the title, game and tag. *Upload to Fireshare* on the same recent export says "already on Fireshare". *Disconnect* removes the "SteamReplay on <PC>" token from Fireshare → Settings → Security → Upload tokens.
15. **Uninstall.** *Settings → Apps → SteamReplay → Uninstall*. The Start menu entry and `%LOCALAPPDATA%\SteamReplay` are gone. Only the app data under `%APPDATA%\de.steamreplay.app` and `%LOCALAPPDATA%\de.steamreplay.app` may stay.

## Results

| Tester | GPU / driver | Windows version | Preview backend | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | Notes |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | | | | | | | | | | | |
