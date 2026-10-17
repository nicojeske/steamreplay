# M10: Fireshare upload

## 1. Goal and user-visible outcome
The team shares clips on our Fireshare server (the fork at `nicojeske/fireshare`). Before M10, sharing a clip meant exporting it, opening Fireshare in a browser, uploading the file, then choosing the game and tags. After M10 SteamReplay does this itself:
- **Settings → Fireshare:** enter the server URL and an upload token. "Test connection" shows "Connected as <user>".
- **Export & upload:** a second button in the Export tab. A dialog asks for a title, the game and tags, then one queued job exports the selection and uploads it.
- **Upload an existing export:** "Upload to Fireshare" on a finished job, in the export-done toast and on the recent exports.
- **When it is done:** a toast with **Copy link** (`<server>/w/<video id>`) and **Open**. The recent export remembers its link, so its row shows "Copy link" from then on.
- **Duplicates:** a clip that is already on the server is not sent again. The user gets its existing link.

Only uploading is in scope: no browsing, editing or deleting of clips on the server.

## 2. Prerequisites
- M1–M9 done. M9 provides the global export queue (`src/state/exports.ts`), toasts and the settings dialog.
- A Fireshare server with upload tokens (upstream ≥ 1.8, which the fork has). The user's account needs the `upload` permission. A token is created in Fireshare under **Account → Upload tokens**.
- The server API this milestone uses, in `app/server/fireshare/api/upload_tokens.py` of the fork:

| Route | Use |
|---|---|
| `GET /api/upload/token` | Checks the token. Returns `{ok, username, default_folder, supported_video_types, …}` |
| `GET /api/upload/token/options` | Returns `{default_folder, folders, folder_rules:{video:[{folder, game_id, game}]}, games:[{id, name, steamgriddb_id}], tags:[{id, name, color}]}` |
| `GET /api/upload/token/exists?video_id=<32 hex>` | Returns `{exists:false}` or `{exists:true, video_id, title, url:"/w/<id>"}` |
| `POST /api/upload/token/chunked` | Multipart fields: `blob`, `chunkPart` (1-based), `totalChunks`, `checkSum` (`[A-Za-z0-9_-]`), `fileName`, `fileSize`, `title`, `game_id`, `tag_ids` (comma-separated), `folder`. Responses: 202 `{status:"partial", received, total}` per chunk, and 201 `{status:"accepted", filename, folder}` on the last one. 409 is a duplicate `{error:"duplicate", video_id, url}`, 415 means the file has no video stream, 400 is a bad type or game, 401 a bad token, 429 too many bad tokens (`Retry-After`). |

Authentication is `Authorization: Bearer fsk_…`. CORS does not matter, because the requests come from Rust, not from the webview.

## 3. Scope / non-goals
**In scope:**
- Fireshare settings: the server URL and token, and a connection test.
- An upload dialog for the title, game and tags.
- Upload jobs in the global queue: export and upload together, or upload only.
- Chunked upload with retries, progress and cancel.
- A duplicate check before uploading.
- Share links, and links stored on recent exports.

**Non-goals:**
- Listing, editing, deleting, or setting privacy or description on server clips. A token cannot do these.
- Creating Fireshare games or tags; a token cannot do this either. New games are added in the Fireshare web UI.
- Session or Discord login.
- More than one server.
- Uploading images.

**No changes to the Fireshare fork are needed.** One optional tweak there: `proxy_request_buffering off` for `/api/upload/token/chunked` in `app/nginx/prod.conf`, as `/api/uploadChunked` already has. Without it, nginx buffers each 8 MiB chunk before passing it on, which only costs a little latency.

## 4. Design

### 4.1 Dependencies
- `reqwest` 0.13 with `default-features = false` and features `rustls-no-provider`, `multipart`, `json`, `stream`.
- `rustls` 0.23 with feature `ring`.
- The TLS stack is the one the updater already brings in (rustls with ring). The client installs the ring provider if nothing has yet. That avoids OpenSSL on Linux and a second TLS stack.
- Certificates are checked against the OS trust store (`rustls-platform-verifier`). A self-signed server works only if its CA is trusted by the OS.
- `xxhash-rust` with feature `xxh3`.
- `bytes` and `futures-util`, to stream a chunk's body for progress.
- Not `reqwest`'s `query` feature: the one query parameter (a hex digest) is put into the URL directly.

### 4.2 Video id (`src-tauri/src/fireshare/hash.rs`)
- Fireshare's id is `xxh3_128_hexdigest(first 16 MiB of the file)` (`util.video_id`). The token upload's 201 response does not include it, so SteamReplay computes it itself and uses it for:
  - the share link `/w/<id>`,
  - the duplicate check before uploading.
- `video_id(path) -> String` reads at most 16 MiB and returns `format!("{:032x}", xxh3_128(..))`. This is the same big-endian hex as Python's `hexdigest`.
- The tests compare against vectors produced with Python `xxhash` 4.0.1 for 0 bytes, 1000 bytes, and 20 MiB of generated data.

### 4.3 Client (`src-tauri/src/fireshare/client.rs`)
- **`Client::new(url, token)`:**
  - `normalize_url` trims whitespace and trailing `/` and requires `http://` or `https://`. Plain `http` is allowed for a LAN server.
  - Connect timeout 10 s. Each request has a 300 s timeout, so an 8 MiB chunk still fits at about 250 kbit/s.
  - User agent `SteamReplay/<version>`.
- **`check()`** returns the `username`. **`options()`** returns `FireshareOptions`. **`exists(id)`** returns `Option<String>` with the server-relative URL.
- **`upload(path, meta, on_progress, cancel)`:**
  1. Hash the file (`video_id`). If this app run already uploaded it to this server, or `exists` finds it, return `UploadResult { duplicate: true, url }` without sending anything.
     - The memory of this run's uploads is needed because the server only knows a clip once its background scan has run, about 1 s after the upload. The manual test showed that a second upload in that window lands as a second file (`clip-xxxxxx.mp4`).
  2. Split the file into `ceil(size / 8 MiB)` chunks (`chunk_plan`). A 0-byte file is an error. `checkSum` is `sr<video id><8 random hex>`.
  3. Send the chunks **one after another**. Two requests that complete the set at the same time can make the server fail. Each body is streamed in 256 KiB slices, and `on_progress(bytes sent / size)` reports from those.
  4. On a 202, if `received < chunkPart` the server lost the set (for example after its daily sweep). Check `exists`: if the clip is there, an earlier last chunk did go through and only its answer was lost, so the upload is done. If not, restart **once** with a new `checkSum`.
  5. Errors:
     - Network errors and 5xx get 3 attempts per chunk, after 1 s, 3 s and 9 s.
     - A 429 waits `Retry-After`, capped at 60 s, then retries.
     - Other statuses fail at once with a readable message (§4.4).
  6. A 201 gives `duplicate:false`. A 409 gives `duplicate:true` with the `url` from its body.
  7. Cancelling (`CancellationToken`, raced against each request with `tokio::select!`) returns `AppError::Cancelled`. Parts already sent stay on the server until its startup and daily sweep removes them.
- **Metadata sent with every chunk:**
  - `fileName` is the local file name.
  - `title` is sent if not empty.
  - `game_id`.
  - `tag_ids`, comma-separated.
  - `folder`, if the chosen game has a folder rule (`folder_rules.video`). The clip then lands in the game's own folder, as a clip uploaded in the web UI would. Otherwise the server's default folder is used.
- The share URL is `<base>/w/<video id>`, built by `share_url(base, path)` from the server-relative path.

### 4.4 Errors
- New `AppError::Fireshare(String)`, kind `fireshare`. The messages:

| Case | Message |
|---|---|
| Not configured | "Fireshare is not set up. Add the server and token in Settings → Fireshare." |
| 401 | "Fireshare rejected the upload token. It may have been deleted, or the account lost the upload permission." |
| 403 / 404 on the token routes | "This server has no upload-token API. Is it Fireshare 1.8 or newer?" |
| 415 | "Fireshare says the file has no video stream." |
| 400 | The server's text or `message`, for example "No game named …" |
| Connection error | "Cannot reach <url>: <reason>" (covers DNS, refused connections, and TLS errors such as self-signed certificates) |
| Other | `HTTP <status>: <body, first 200 chars>` |

### 4.5 Settings and commands
The keys live in `settings.json` through `read_setting` / `write_setting`:
- `fireshareUrl`: string.
- `fireshareToken`: string. It is **stored in plain text**, as the rest of the settings are. A token can only upload and can be revoked in Fireshare at any time; using the OS keyring was judged not worth the platform work.
- `fireshareGameMap`: `{ <Steam game name>: <Fireshare game id> }`, the last game chosen for each Steam game.

Commands (tauri-specta):
- `get_fireshare_config() -> FireshareConfig { url: Option<String>, has_token: bool }`. The token never goes back to the webview.
- `set_fireshare_config(url: Option<String>, token: Option<String>)`. The URL is normalised; `None` or empty clears it. If `token` is `None`, the stored token is kept, so the user can change the URL without typing the token again. To remove the token, pass `Some("")`.
- `fireshare_check() -> String`: the username.
- `fireshare_options() -> FireshareOptions { default_folder, games, tags, folder_rules }`.
- `get_fireshare_game(steam_game) -> Option<u32>` and `set_fireshare_game(steam_game, game_id: Option<u32>)`.
- `fireshare_upload(req: UploadRequest { path, title, game_id, tag_ids, folder }, job_id, progress: Channel<Progress>) -> UploadResult { video_id, url, duplicate }`.
  - It uses `MediaState::start_job(job_id)`, so the existing `cancel_job` works.
  - When it succeeds, the matching `RecentExport` (same path) gets `fireshare_url`.
- `RecentExport.fireshare_url: Option<String>` uses `#[serde(default)]`, so entries stored before M10 still load.

### 4.6 Frontend
**Settings: `components/settings/FireshareSection.tsx`**, the new "Fireshare" section with the `Share2` icon. M12 (`12-fireshare-connect.md`) replaces typing the token with **Connect** in the browser; the fields below live on under "Advanced".
- Fields:
  - Server URL.
  - Token, a password input. Its placeholder reads "•••• saved" when a token is stored.
  - Save.
  - Test connection, which shows "Connected as <user>" or the error.
- Hint: "Create a token in Fireshare under Account → Upload tokens. It can only upload."

**Upload dialog: `components/fireshare/UploadDialog.tsx`**, using `Modal`.
- **Fields:**
  - Title. The default is `<Game> – <date of the clip>`, falling back to the export's finish date.
  - Game: a `<select>` with "No game" plus the server's games.
  - Tags: toggle chips in each tag's colour.
- **Game preselection:**
  1. `get_fireshare_game(steamGame)`.
  2. Otherwise a game whose name equals the Steam game name, ignoring case and punctuation.
  3. Otherwise "No game".
- Confirming stores the choice with `set_fireshare_game`.
- The options load when the dialog opens and are cached for the session in `state/fireshare.ts`, with a reload button.
- **If Fireshare is not configured,** the dialog shows a callout with "Open settings" instead of the form.
- **A store `state/fireshare.ts`** holds the configuration, the cached options, and the open dialog request (`openUpload({ …, onSubmit })`). That way the toast, the job rows and the Export tab share one dialog, which `App.tsx` mounts.

**Queue: `state/exports.ts`**
- A job has an optional `export` part (session, range, output, mode) and an optional `upload` part (`UploadMeta`). An upload-only job carries the `path` instead of the export part.
- Status:
  - `running` gains `phase: "export" | "upload"`.
  - `done` keeps the `ExportResult`, which is `null` for an upload-only job, and gains `path` and `upload?: UploadResult`.
  - `failed` keeps `path` when the export worked but the upload failed, so the row can offer "Retry upload".
- The pump runs the export, then the upload, as one job. `selectBusy` therefore covers uploads, and the updater waits for them.
- Toasts:
  - After an export without an upload: "Exported <name>" with "Show in folder" and "Upload to Fireshare". The upload action appears only when Fireshare is configured.
  - After an upload: "Uploaded to Fireshare" or "Already on Fireshare", with "Copy link" and "Open". The text adds that the clip appears once Fireshare has processed it, which takes a few seconds to a minute.

**Entry points**
- **`ExportTab.tsx`:** an "Export & upload" button below "Export clip", with the same enabled state. It opens the dialog, and its submit queues the combined job.
- **`JobRow.tsx`:**
  - A done job without a link gets an upload icon button.
  - A done job with a link gets "Copy link" and "Open".
  - A running job shows "Exporting…" or "Uploading…" in its meta line.
- **`ExportsMenu.tsx`, recent rows:** the same as a done job, based on `entry.fireshareUrl`.

**Capabilities.** Opening the share URL uses `openUrl` from `@tauri-apps/plugin-opener`, which `opener:default` already allows for `http(s)`. No `http:` plugin is needed.

## 5. Edge cases
- **Files under 16 MiB:** the whole file is hashed, as Fireshare does.
- **Duplicates:**
  - Caught before the upload by this run's memory of uploads, then by `exists`.
  - A 409 is also handled in case two people upload the same clip at once.
  - Two people uploading the same file within a second can still both get through, because Fireshare's own duplicate check waits for the scan. That race is the server's, and it is harmless: the scan adds only one video.
- **The link right after upload:** the server scans the file in the background, so `/w/<id>` can show "not found" for a few seconds. The toast says so.
- **The token is revoked mid-upload:** the next chunk gets 401, the job fails with the 401 message, and nothing is retried.
- **A game deleted between loading the options and uploading:** 400 "unknown game". The message tells the user to reload the options in the dialog.
- **The export folder is on a drive that disappears before an upload-only job:** an Io error naming the path.
- **A large Fast export (several GB):** about 8 MiB per request with a bounded body, so memory stays at one chunk. A reverse proxy's body limit only needs to allow 8 MiB plus the form overhead.
- **The server answers HTML:** a proxy error page or a login page becomes the "Other" message, and the body is cut to 200 characters.
- **A URL entered without a scheme:** rejected in the settings with "Start the URL with https:// or http://".
- **The app is closed during an upload:** `selectBusy` makes the updater wait. If the app is quit anyway, the parts on the server are removed by the server's sweep.
- **Privacy:** the server's default for new uploads (`video_defaults.private`) applies. A token cannot change it.
- **Fireshare's Discord notification** fires as for any upload. SteamReplay does nothing extra.

## 6. Acceptance criteria
1. After entering the URL and token in Settings → Fireshare, Test connection shows the right username. A wrong token shows the 401 message.
2. Export & upload of a Fast export and of a Precise (Discord) export each queue one job. The job shows Exporting and then Uploading progress, and ends with a toast whose Copy link puts `<server>/w/<id>` on the clipboard. Within about a minute the link plays the clip with the chosen title, game and tags.
3. Uploading the same file again uploads nothing and shows "Already on Fireshare" with the same link.
4. Uploading an earlier export from the recent exports works, and afterwards that row shows Copy link and Open.
5. Cancel during upload ends the job as cancelled within a second.
6. The game picker preselects the last game chosen for that Steam game.
7. Without a Fireshare configuration, no upload actions appear in toasts or rows, and Export & upload opens the dialog with "Open settings".
8. `cargo clippy -D warnings`, `cargo test`, `pnpm lint` and `pnpm tsc --noEmit` pass, and `src/bindings.ts` is current.

## 7. Verification steps
- **`cargo test`:**
  - `hash`: the Python vectors.
  - `client`:
    - `normalize_url`: trailing slashes and spaces, no scheme rejected, `http` allowed.
    - `chunk_plan`: sizes 1, exactly 8 MiB, 8 MiB + 1, and 0, which is an error.
    - `share_url`.
    - Error mapping for 401, 403, 409, 415 and 400 with JSON and with text.
    - A mock server on a local `TcpListener`: a 3-chunk upload answered 202, 500 (retried), 202, 201 makes 4 requests, and the second request is a repeat of chunk 2. A lost set (202 with `received` 0) leads to an `exists` call and then a restart.
- **Manual test on Linux against the fork:**
  - Start it with `docker run -p 8089:80 -e ADMIN_USERNAME=admin -e ADMIN_PASSWORD=admin ghcr.io/nicojeske/fireshare:latest-lite`, with the four volumes.
  - The image may not have the custom-games route yet. Without a SteamGridDB key, add a test game straight into `game_metadata` in `/data/db.sqlite`.
  - Log in as admin, create a game and a tag, and create a token.
  - Go through acceptance criteria 1 to 7.
- **Live test against a real server:** `FIRESHARE_URL=… FIRESHARE_TOKEN=fsk_… FIRESHARE_CLIP=<mp4> cargo test live_upload -- --ignored --nocapture`. It uploads the file twice, and the second upload must report `duplicate: true`.
- **Windows:** the Fireshare step in `docs/windows-test.md`.

## 8. Open questions / risks
- **Plain-text token in `settings.json`.** This is acceptable for an upload-only token. If tokens ever get more rights, move the token to the OS keyring with the `keyring` crate.
- **The server's scan time.** The link works only after the background scan. Waiting for it by polling `exists` would be possible but is not done; the toast explains the delay instead.
- **Progress reflects bytes handed to the socket, not bytes the server has received.** On a slow link the bar can reach a chunk's end before the 202 arrives. This is acceptable.
- **Upstream Fireshare changes to the token API** (field names, the 16 MiB id). The fork is synced with upstream regularly, so check this spec when syncing.
