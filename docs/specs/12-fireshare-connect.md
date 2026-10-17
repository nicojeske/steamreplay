# M12: Fireshare Connect

## 1. Goal and user-visible outcome
Since M10, a user who wants to upload has to:
1. open Fireshare,
2. create an upload token under Settings → Security → Upload tokens,
3. paste it into SteamReplay.

The team signs in to Fireshare with Discord, so SteamReplay cannot sign in for them with a password. After M12 SteamReplay pairs with the server the way a TV app or the GitHub CLI does:
- **Settings → Fireshare → Connect:**
  - SteamReplay opens `<server>/connect?code=K7QM-2XPD` in the user's browser.
  - If they are not signed in yet, Fireshare sends them to its login, which can be Discord, and brings them back afterwards.
  - The page asks "Allow SteamReplay on DESKTOP-X to upload as nico?".
  - **Allow** gives SteamReplay its own upload token, and the settings show "Connected as nico".
- **Disconnect** removes that token on the server, too.
- **Advanced:** pasting a token by hand, as in M10, stays available for servers without Connect.

## 2. Prerequisites
- M10 done.
- The Fireshare fork (`nicojeske/fireshare`) with the connect routes from this milestone. No upstream server has them; the routes answer 404 there, and SteamReplay says so.
- The user's account needs the `upload` permission, as in M10.

## 3. Scope / non-goals
**In scope:**
- A device-code flow in the fork: a request table, routes, and a `/connect` page.
- "Return to this page" after a password or Discord login in the fork.
- A self-revoke route for a token: `DELETE /api/upload/token`.
- Connect, Disconnect and Advanced in SteamReplay's settings.

**Non-goals:**
- Signing in from inside the app (an embedded login window). Discord discourages it, and the user would have to sign in to Discord again.
- Using the session cookie for uploads. The token routes stay the one upload path.
- A custom URL scheme or a loopback listener. The app polls instead, so there is no firewall prompt and no handler to register.
- A token that expires on its own, or that has scopes. Tokens stay as in M10.

## 4. Design

### 4.1 Server routes (fork, `app/server/fireshare/api/connect.py`)

| Route | Auth | Body / answer |
|---|---|---|
| `POST /api/connect/start` | none, at most 20 per IP in 5 min (429 with `Retry-After`) | `{client_name}` → 201 `{device_code, user_code, verification_path, expires_in, interval}` |
| `GET /api/connect/<user_code>` | session with `upload` | `{client_name, user_code, status, expires_at}`; 404 if unknown or expired |
| `POST /api/connect/<user_code>/approve` | session with `upload` | `{status:"approved"}`; 400 if the user already has 20 tokens; 409 if the request was already decided |
| `POST /api/connect/<user_code>/deny` | session with `upload` | `{status:"denied"}` |
| `POST /api/connect/token` | none (the `device_code` is the credential) | `{device_code}` → `{status:"pending"}`, `{status:"denied"}`, `{status:"expired"}` or `{status:"approved", username, token:{id, name, prefix, created_at, last_used_at, secret}}` |
| `DELETE /api/upload/token` | upload token | Deletes the token that authenticated the request → `{deleted:true}` |

- **`connect_request` table** (migration `x9s0t1u2v3w4`). A table rather than process memory, because gunicorn runs several workers. Columns:
  - `id`.
  - `device_code_hash`: the sha256 of the device code, unique.
  - `user_code`: unique.
  - `client_name`, `created_at`, `expires_at`.
  - `status`: one of pending, approved, denied, claimed.
  - `user_id`: nullable.
- **Codes:**
  - The `device_code` is `secrets.token_urlsafe(32)`. Only its hash is stored, as for tokens.
  - The `user_code` is 8 characters from `BCDFGHJKMNPQRSTVWXZ23456789`, which avoids look-alikes and vowels, so the code never spells a word. It is shown as `XXXX-XXXX`. Lookups ignore case and the dash.
- **Lifetime:** a request lives 10 minutes. Expired and claimed rows are deleted whenever a new request is started.
- **The token is minted when it is collected, not when the request is approved.** Approving only stores the user id. The first `POST /api/connect/token` after approval:
  1. re-reads the user and checks that they are enabled and still have `upload`,
  2. creates the `UploadToken` named after `client_name`,
  3. marks the request `claimed`,
  4. returns the secret.

  The secret is never stored, and it is returned exactly once.
- **The device code itself guards `/api/connect/token`**, with 256 bits of entropy. Unknown device codes count towards the IP throttle of the upload tokens.
- **Phishing:** someone could start a request and send the link to a victim. The page therefore shows the client name and the code, plus "Only allow this if you just clicked Connect in SteamReplay". The request lasts only 10 minutes. This is the same safeguard as GitHub's device flow.

### 4.2 Login return (fork)
- **`next`** is accepted only as a relative path: it starts with `/` and not `//` or `/\`. Anything else falls back to `/`.
- **`GET /api/auth/discord/start?next=…`** stores `login_next` in the session. The callback redirects there instead of `/`. When MFA is required, it goes to `/login?mfa=1&next=…`.
- **`LoginForm.js`**:
  - reads `next` from the query,
  - navigates there after a password or MFA login,
  - passes `next` on to the Discord link.
- **SPA route `/connect`** (`views/Connect.js`):
  - When not signed in, it redirects to `/login?next=<the connect URL>`.
  - The states it shows are loading, ask (Allow / Deny), approved ("You can close this tab"), denied, expired or unknown, no upload permission, and error.

### 4.3 SteamReplay client (`src-tauri/src/fireshare/connect.rs`)
- **`start(base, client_name) -> Pending`.** The client name is `SteamReplay on <COMPUTERNAME|HOSTNAME>`, or `SteamReplay`.
- **`wait(base, &pending, cancel) -> Connected { username, secret }`** polls `/api/connect/token` every `interval` seconds, at least 1 s:
  - until it is approved, denied or expired,
  - or until `expires_in` has run out, which counts as expired,
  - or until it is cancelled.
- **Network errors while polling** are retried until the request expires. A laptop that briefly drops off Wi-Fi does not end the wait.
- **Errors:**

| Case | Message |
|---|---|
| 404 on start | "This Fireshare can't connect apps yet. Update the server, or paste an upload token under Advanced." |
| 429 on start | "Too many connection attempts. Try again in a few minutes." |
| denied | "The connection was denied in Fireshare." |
| expired | "The code expired before it was approved. Click Connect again." |

- **`Client::revoke()`** sends `DELETE /api/upload/token`. A 404 or 405 from an older server is ignored.

### 4.4 Commands
- **`fireshare_connect_start(url) -> ConnectInfo { user_code, verification_url }`:**
  - Normalises the URL and stores it as `fireshareUrl`.
  - Starts a request and keeps its `Pending` in Rust state (`FireshareState`), so the device code never reaches the webview.
  - A new start replaces an older pending request.
- **`fireshare_connect_wait(job_id) -> String`** (the username):
  - Runs under `MediaState::start_job`, so `cancel_job` stops it.
  - On success, it revokes the token stored before, best effort and only logged on failure. Then it stores the new secret as `fireshareToken`.
- **`fireshare_disconnect()`:** `revoke()` (best effort), then clears the token. The URL stays.
- **`get_fireshare_config` / `set_fireshare_config`** stay as they are, for Advanced.
- **The "not set up" message** becomes "Fireshare is not connected. Connect it in Settings → Fireshare."

### 4.5 Frontend (`components/settings/FireshareSection.tsx`)
- **Server:** the URL field, as before.
- **Not connected:** **Connect**.
  1. It calls `fireshareConnectStart` and opens the URL with `openUrl`.
  2. It shows a callout: "Approve SteamReplay in your browser. Code: K7QM-2XPD", with **Open again** and **Cancel**.
  3. It calls `fireshareConnectWait`.
  4. On success, the config is reloaded and the section shows "Connected as <user>".
- **Connected:** "Connected as <user>", checked on mount with `fireshareCheck`, plus **Test connection** and **Disconnect**. If the check fails, for example because the token was revoked, the error is shown along with **Connect**.
- **Advanced** (`<details>`): the M10 token field and Save, with the hint "For servers without Connect. Create a token in Fireshare under Settings → Security → Upload tokens."
- **Closing the settings during a wait** cancels it.
- **The upload dialog's callout** reads "Fireshare is not connected." with "Open settings".

## 5. Edge cases
- **Already connected, Connect again:** the old token is revoked once the new one arrives, so tokens do not pile up.
- **The browser tab is closed without a decision:** the app waits until the code expires (10 min) or the user cancels.
- **Two approvals for one code** (two tabs): the second gets 409, and the page shows the result of the first.
- **The user loses `upload` between Allow and the collection:** the collection answers `denied`.
- **20 tokens already:** Allow answers 400, and the page says to delete an old token in Fireshare first.
- **The server restarts during a wait:** the request is in the database, so it survives the restart.
- **The server lacks a `SECRET_KEY`,** so sessions die on a restart: this only affects the browser login, not the token.
- **Discord sessions end after 24 h:** this does not matter, because the token does not depend on the session.

## 6. Acceptance criteria
1. With a fresh install and a Discord or password account on the fork: entering the URL, clicking Connect and then Allow in the browser shows "Connected as <user>" in SteamReplay. No token is typed.
2. When the browser is not signed in, Connect leads through the login (password or Discord) and back to the Allow page.
3. Deny in the browser shows the denied message in the app. Cancel in the app stops the wait within a second.
4. Disconnect removes the token from the user's list in Fireshare under Settings → Security → Upload tokens.
5. Connecting twice leaves exactly one SteamReplay token on the server.
6. Against a server without the routes, Connect shows the "can't connect apps yet" message, and Advanced still works.
7. Uploading works after Connect exactly as in M10.
8. `cargo clippy -D warnings`, `cargo test`, `pnpm lint` and `pnpm tsc --noEmit` pass, and `src/bindings.ts` is current.

## 7. Verification steps
- **`cargo test`** (mock server, as in `client.rs`):
  - start, then pending, pending, approved returns the secret after 3 polls;
  - a network error while polling is retried;
  - denied, expired and a 404 on start each give their message;
  - cancelling gives `Cancelled`;
  - `revoke` sends a `DELETE` with the bearer token.
- **The fork in Docker:** `docker build -f Dockerfile.lite -t fireshare-dev .`, then run it as in M10 §7. With curl:
  - start, then token (pending);
  - approve with the admin session cookie, then token (approved, `fsk_…`), then token again (`expired`, because the request was claimed);
  - the secret works on `GET /api/upload/token`, and after `DELETE /api/upload/token` it gets 401.
- **End to end:** `pnpm tauri dev` against that container, going through acceptance criteria 1 to 7.
- **Windows:** the Connect step in `docs/windows-test.md`.

## 8. Open questions / risks
- **The device flow can be phished,** as described in §4.1. This is accepted for a team server, with the code shown on both sides and a 10-minute lifetime.
- **The connect routes exist only in the fork.** If upstream adds an app login of its own, switch to it when syncing.
