//! Calls to Fireshare's upload-token API (`/api/upload/token*`).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use reqwest::{RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::sync::CancellationToken;

use super::{FireshareOptions, FolderRule, Game, Tag, UploadMeta, UploadResult, hash};
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::ProgressFn;

/// Chunk size of an upload. Small enough for a reverse proxy's default body limit.
pub const CHUNK_BYTES: u64 = 8 * 1024 * 1024;
/// A chunk's body is streamed in slices of this size, for the progress bar.
const SLICE_BYTES: usize = 256 * 1024;
const RETRY_DELAYS: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(3), Duration::from_secs(9)];
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

pub struct Client {
    base: String,
    token: String,
    http: reqwest::Client,
    chunk_bytes: u64,
    retry_delays: Vec<Duration>,
}

/// `https://clips.example.com/` → `https://clips.example.com`. A scheme is required.
pub fn normalize_url(raw: &str) -> AppResult<String> {
    let url = raw.trim().trim_end_matches('/');
    let lower = url.to_ascii_lowercase();
    let rest = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://"));
    match rest {
        Some(host) if !host.is_empty() => Ok(url.to_owned()),
        _ => Err(AppError::Fireshare("Start the URL with https:// or http://".into())),
    }
}

/// The link to a video page, from a server-relative path such as `/w/<id>`.
pub fn share_url(base: &str, path: &str) -> String {
    format!("{base}/{}", path.trim_start_matches('/'))
}

/// `(offset, length)` of each chunk.
pub fn chunk_plan(size: u64, chunk: u64) -> AppResult<Vec<(u64, u64)>> {
    if size == 0 {
        return Err(AppError::Fireshare("the file is empty".into()));
    }
    Ok((0..size.div_ceil(chunk)).map(|i| (i * chunk, chunk.min(size - i * chunk))).collect())
}

/// What a non-success answer means for the user.
pub fn status_error(status: StatusCode, body: &str) -> AppError {
    let msg = match status.as_u16() {
        401 => "Fireshare rejected the upload token. It may have been deleted, or the account lost the upload permission.".into(),
        403..=405 => "This server has no upload-token API. Is it Fireshare 1.8 or newer?".into(),
        415 => "Fireshare says the file has no video stream.".into(),
        400 => serde_json::from_str::<ServerMessage>(body)
            .ok()
            .and_then(|m| m.message.or(m.error))
            .unwrap_or_else(|| body.trim().to_owned()),
        _ => {
            let text: String = body.trim().chars().take(200).collect();
            format!("HTTP {}: {text}", status.as_u16())
        }
    };
    AppError::Fireshare(msg)
}

#[derive(Deserialize)]
struct ServerMessage {
    message: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct CheckReply {
    username: String,
}

#[derive(Deserialize)]
struct ExistsReply {
    exists: bool,
    url: Option<String>,
}

#[derive(Deserialize)]
struct PartialReply {
    received: u32,
}

#[derive(Deserialize)]
struct DuplicateReply {
    url: Option<String>,
}

#[derive(Deserialize)]
struct RawOptions {
    default_folder: Option<String>,
    #[serde(default)]
    games: Vec<Game>,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    folder_rules: RawFolderRules,
}

#[derive(Deserialize, Default)]
struct RawFolderRules {
    #[serde(default)]
    video: Vec<RawFolderRule>,
}

#[derive(Deserialize)]
struct RawFolderRule {
    folder: String,
    game_id: u32,
}

/// The answer to one chunk.
enum ChunkReply {
    /// 202: the server holds `received` parts of the set.
    Partial(u32),
    /// 201: the last part completed the file.
    Done,
    /// 409: the finished file is already in the library.
    Duplicate(Option<String>),
}

/// Share links of the uploads of this app run, by server and video id. The server only knows
/// an upload once its background scan ran, so `exists` misses a clip uploaded seconds ago and a
/// second upload would land as a second copy.
static UPLOADED: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

fn uploaded(base: &str, video_id: &str) -> Option<String> {
    UPLOADED.lock().expect("uploaded lock").as_ref()?.get(&(base.to_owned(), video_id.to_owned())).cloned()
}

fn remember_upload(base: &str, video_id: &str, url: &str) {
    UPLOADED
        .lock()
        .expect("uploaded lock")
        .get_or_insert_with(HashMap::new)
        .insert((base.to_owned(), video_id.to_owned()), url.to_owned());
}

/// A per-upload id that differs between restarts of the same file.
fn new_check_sum(video_id: &str) -> String {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("sr{video_id}{:08x}", nanos ^ seq.wrapping_mul(0x9E37_79B9))
}

/// The error and its causes, e.g. "error sending request: connection refused".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        out.push_str(": ");
        out.push_str(&s.to_string());
        source = s.source();
    }
    out
}

impl Client {
    pub fn new(url: &str, token: &str) -> AppResult<Self> {
        // Same provider as the updater; whichever builds a client first installs it.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            // An 8 MiB chunk at ~250 kbit/s.
            .timeout(Duration::from_secs(300))
            .user_agent(concat!("SteamReplay/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AppError::Fireshare(format!("could not set up HTTP: {}", error_chain(&e))))?;
        Ok(Self {
            base: normalize_url(url)?,
            token: token.trim().to_owned(),
            http,
            chunk_bytes: CHUNK_BYTES,
            retry_delays: RETRY_DELAYS.to_vec(),
        })
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.http.get(format!("{}{path}", self.base)).bearer_auth(&self.token)
    }

    fn unreachable(&self, e: &reqwest::Error) -> AppError {
        AppError::Fireshare(format!("Cannot reach {}: {}", self.base, error_chain(e)))
    }

    async fn send(&self, req: RequestBuilder, cancel: &CancellationToken) -> AppResult<Response> {
        tokio::select! {
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            r = req.send() => r.map_err(|e| self.unreachable(&e)),
        }
    }

    /// The body of a 2xx answer as JSON, else the matching error.
    async fn json<T: serde::de::DeserializeOwned>(&self, resp: Response) -> AppResult<T> {
        let status = resp.status();
        let body = resp.text().await.map_err(|e| self.unreachable(&e))?;
        if !status.is_success() {
            return Err(status_error(status, &body));
        }
        serde_json::from_str(&body).map_err(|_| status_error(status, &body))
    }

    /// The token's user name.
    pub async fn check(&self) -> AppResult<String> {
        let resp = self.send(self.get("/api/upload/token"), &CancellationToken::new()).await?;
        Ok(self.json::<CheckReply>(resp).await?.username)
    }

    /// Games, tags and folder rules an upload may use.
    pub async fn options(&self) -> AppResult<FireshareOptions> {
        let resp = self.send(self.get("/api/upload/token/options"), &CancellationToken::new()).await?;
        let raw: RawOptions = self.json(resp).await?;
        Ok(FireshareOptions {
            default_folder: raw.default_folder,
            games: raw.games,
            tags: raw.tags,
            folder_rules: raw.folder_rules.video.into_iter().map(|r| FolderRule { folder: r.folder, game_id: r.game_id }).collect(),
        })
    }

    /// The share link of the video with this id, if the server has it.
    pub async fn exists(&self, video_id: &str, cancel: &CancellationToken) -> AppResult<Option<String>> {
        // A hex digest, nothing to escape.
        let req = self.get(&format!("/api/upload/token/exists?video_id={video_id}"));
        let reply: ExistsReply = self.json(self.send(req, cancel).await?).await?;
        Ok(reply.exists.then(|| share_url(&self.base, reply.url.as_deref().unwrap_or(&format!("/w/{video_id}")))))
    }

    /// Uploads the file in chunks, unless the server already has it.
    pub async fn upload(
        &self,
        path: &Path,
        meta: &UploadMeta,
        on_progress: &ProgressFn,
        cancel: &CancellationToken,
    ) -> AppResult<UploadResult> {
        let video_id = {
            let p = path.to_owned();
            tokio::task::spawn_blocking(move || hash::video_id(&p))
                .await
                .map_err(|e| AppError::Io(format!("background task failed: {e}")))??
        };
        let result = |url: Option<String>, duplicate| UploadResult {
            url: url.unwrap_or_else(|| share_url(&self.base, &format!("/w/{video_id}"))),
            video_id: video_id.clone(),
            duplicate,
        };
        if let Some(url) = uploaded(&self.base, &video_id) {
            return Ok(result(Some(url), true));
        }
        if let Some(url) = self.exists(&video_id, cancel).await? {
            return Ok(result(Some(url), true));
        }
        let done = self.send_file(path, &video_id, meta, on_progress, cancel).await?;
        let url = done.0.map(|u| share_url(&self.base, &u));
        let r = result(url, done.1);
        remember_upload(&self.base, &video_id, &r.url);
        Ok(r)
    }

    /// Sends the chunks. Returns the server-relative link if the server named one, and whether
    /// the clip was already there.
    async fn send_file(
        &self,
        path: &Path,
        video_id: &str,
        meta: &UploadMeta,
        on_progress: &ProgressFn,
        cancel: &CancellationToken,
    ) -> AppResult<(Option<String>, bool)> {
        let size = tokio::fs::metadata(path).await.map_err(|e| AppError::io(path.display(), e))?.len();
        let plan = chunk_plan(size, self.chunk_bytes)?;
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "clip.mp4".into());
        let mut file = tokio::fs::File::open(path).await.map_err(|e| AppError::io(path.display(), e))?;
        let mut restarted = false;
        'set: loop {
            let check_sum = new_check_sum(video_id);
            for (i, &(offset, len)) in plan.iter().enumerate() {
                let part = i as u32 + 1;
                let mut buf = vec![0; len as usize];
                file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| AppError::io(path.display(), e))?;
                file.read_exact(&mut buf).await.map_err(|e| AppError::io(path.display(), e))?;
                let chunk = Chunk {
                    data: Bytes::from(buf),
                    part,
                    total: plan.len() as u32,
                    check_sum: &check_sum,
                    file_name: &file_name,
                    size,
                    offset,
                };
                match self.send_chunk(&chunk, meta, on_progress, cancel).await? {
                    ChunkReply::Partial(received) if received >= part => {}
                    ChunkReply::Partial(received) => {
                        // The server lost the set (its sweep ran), or an earlier last chunk did
                        // complete the file and only its answer was lost.
                        if self.exists(video_id, cancel).await?.is_some() {
                            return Ok((None, false));
                        }
                        if restarted {
                            return Err(AppError::Fireshare(format!(
                                "the server lost the uploaded parts twice (it holds {received} of {part})"
                            )));
                        }
                        log::warn!("fireshare holds {received} of {part} parts; restarting the upload");
                        restarted = true;
                        continue 'set;
                    }
                    ChunkReply::Done => {
                        on_progress(1.0);
                        return Ok((None, false));
                    }
                    ChunkReply::Duplicate(url) => return Ok((url, true)),
                }
            }
            return Err(AppError::Fireshare("the server did not confirm the last part".into()));
        }
    }

    /// Sends one chunk, retrying network errors, 5xx and 429.
    async fn send_chunk(
        &self,
        chunk: &Chunk<'_>,
        meta: &UploadMeta,
        on_progress: &ProgressFn,
        cancel: &CancellationToken,
    ) -> AppResult<ChunkReply> {
        let mut attempt = 0;
        loop {
            let req = self
                .http
                .post(format!("{}/api/upload/token/chunked", self.base))
                .bearer_auth(&self.token)
                .multipart(chunk.form(meta, on_progress)?);
            let wait = match self.send(req, cancel).await {
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(e) => Err(e),
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = resp
                        .headers()
                        .get(reqwest::header::RETRY_AFTER)
                        .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok())
                        .map(|s| Duration::from_secs(s).min(MAX_RETRY_AFTER));
                    let body = resp.text().await.map_err(|e| self.unreachable(&e))?;
                    match status.as_u16() {
                        201 => return Ok(ChunkReply::Done),
                        202 => {
                            let reply: PartialReply = serde_json::from_str(&body).map_err(|_| status_error(status, &body))?;
                            return Ok(ChunkReply::Partial(reply.received));
                        }
                        409 => {
                            let reply = serde_json::from_str::<DuplicateReply>(&body).ok();
                            return Ok(ChunkReply::Duplicate(reply.and_then(|r| r.url)));
                        }
                        429 => Ok(retry_after.unwrap_or(MAX_RETRY_AFTER)),
                        500..=599 => Err(status_error(status, &body)),
                        _ => return Err(status_error(status, &body)),
                    }
                }
            };
            let Some(&delay) = self.retry_delays.get(attempt) else {
                return Err(match wait {
                    Err(e) => e,
                    Ok(_) => AppError::Fireshare("Fireshare is busy (429); try again later".into()),
                });
            };
            let delay = wait.as_ref().map_or(delay, |&d| d);
            if let Err(e) = &wait {
                log::warn!("fireshare chunk {} of {} failed, retrying: {e}", chunk.part, chunk.total);
            }
            attempt += 1;
            tokio::select! {
                _ = cancel.cancelled() => return Err(AppError::Cancelled),
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }
}

struct Chunk<'a> {
    data: Bytes,
    part: u32,
    total: u32,
    check_sum: &'a str,
    file_name: &'a str,
    /// Of the whole file.
    size: u64,
    offset: u64,
}

impl Chunk<'_> {
    /// The multipart body. The blob is streamed in slices so progress follows the bytes sent.
    fn form(&self, meta: &UploadMeta, on_progress: &ProgressFn) -> AppResult<reqwest::multipart::Form> {
        use reqwest::multipart::{Form, Part};
        let data = self.data.clone();
        let (offset, size) = (self.offset, self.size as f64);
        let progress = on_progress.clone();
        let slices = (0..data.len().div_ceil(SLICE_BYTES)).map(move |i| {
            let end = ((i + 1) * SLICE_BYTES).min(data.len());
            progress((offset + end as u64) as f64 / size);
            Ok::<Bytes, std::io::Error>(data.slice(i * SLICE_BYTES..end))
        });
        let body = reqwest::Body::wrap_stream(futures_util::stream::iter(slices));
        let blob = Part::stream_with_length(body, self.data.len() as u64)
            .file_name("blob")
            .mime_str("application/octet-stream")
            .map_err(|e| AppError::Fireshare(e.to_string()))?;
        let mut form = Form::new()
            .text("chunkPart", self.part.to_string())
            .text("totalChunks", self.total.to_string())
            .text("checkSum", self.check_sum.to_owned())
            .text("fileName", self.file_name.to_owned())
            .text("fileSize", self.size.to_string());
        if let Some(t) = meta.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            form = form.text("title", t.to_owned());
        }
        if let Some(g) = meta.game_id {
            form = form.text("game_id", g.to_string());
        }
        if !meta.tag_ids.is_empty() {
            form = form.text("tag_ids", meta.tag_ids.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
        }
        if let Some(f) = meta.folder.as_deref().filter(|f| !f.is_empty()) {
            form = form.text("folder", f.to_owned());
        }
        Ok(form.part("blob", blob))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url(" https://clips.example.com// ").unwrap(), "https://clips.example.com");
        assert_eq!(normalize_url("http://192.168.1.5:8080").unwrap(), "http://192.168.1.5:8080");
        assert_eq!(normalize_url("HTTPS://Clips.example.com/").unwrap(), "HTTPS://Clips.example.com");
        assert!(normalize_url("clips.example.com").is_err());
        assert!(normalize_url("https://").is_err());
        assert!(normalize_url("").is_err());
    }

    #[test]
    fn builds_share_urls() {
        assert_eq!(share_url("https://c.example", "/w/abc"), "https://c.example/w/abc");
        assert_eq!(share_url("https://c.example", "w/abc"), "https://c.example/w/abc");
    }

    #[test]
    fn plans_chunks() {
        const C: u64 = CHUNK_BYTES;
        assert_eq!(chunk_plan(1, C).unwrap(), vec![(0, 1)]);
        assert_eq!(chunk_plan(C, C).unwrap(), vec![(0, C)]);
        assert_eq!(chunk_plan(C + 1, C).unwrap(), vec![(0, C), (C, 1)]);
        assert_eq!(chunk_plan(10, 4).unwrap(), vec![(0, 4), (4, 4), (8, 2)]);
        assert!(chunk_plan(0, C).is_err());
    }

    fn message(e: AppError) -> String {
        match e {
            AppError::Fireshare(m) => m,
            other => panic!("not a Fireshare error: {other:?}"),
        }
    }

    #[test]
    fn maps_error_statuses() {
        assert!(message(status_error(StatusCode::UNAUTHORIZED, "Invalid upload token.")).contains("rejected the upload token"));
        assert!(message(status_error(StatusCode::NOT_FOUND, "<html>")).contains("no upload-token API"));
        assert!(message(status_error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "")).contains("no video stream"));
        assert_eq!(
            message(status_error(StatusCode::BAD_REQUEST, r#"{"error":"unknown_game","message":"No game named \"X\""}"#)),
            "No game named \"X\""
        );
        assert_eq!(message(status_error(StatusCode::BAD_REQUEST, "Unsupported file type.\n")), "Unsupported file type.");
        let long = "x".repeat(500);
        let m = message(status_error(StatusCode::BAD_GATEWAY, &long));
        assert!(m.starts_with("HTTP 502: ") && m.len() == "HTTP 502: ".len() + 200);
    }

    /// A server that answers each request with the next scripted response and records the
    /// requests (request line plus body, as text).
    async fn mock(script: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in script {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 64 * 1024];
                let header_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    assert!(n > 0, "client closed early");
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                    .unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = sock.read(&mut tmp).await.unwrap();
                    assert!(n > 0, "client closed early");
                    buf.extend_from_slice(&tmp[..n]);
                }
                assert!(head.contains("authorization: Bearer fsk_test") || head.contains("Authorization: Bearer fsk_test"));
                let line = head.lines().next().unwrap_or_default().to_owned();
                log.lock().unwrap().push(format!("{line}\n{}", String::from_utf8_lossy(&buf[header_end..])));
                let reply = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
                sock.shutdown().await.unwrap();
            }
        });
        (base, seen)
    }

    fn test_client(base: &str) -> Client {
        let mut c = Client::new(base, "fsk_test").unwrap();
        c.chunk_bytes = 4;
        c.retry_delays = vec![Duration::ZERO; 3];
        c
    }

    fn clip() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Deadlock_2026.mp4");
        std::fs::write(&path, b"0123456789").unwrap();
        (dir, path)
    }

    fn field(req: &str, name: &str) -> String {
        let marker = format!("name=\"{name}\"");
        let at = req.find(&marker).unwrap_or_else(|| panic!("no {name} in {req}"));
        let start = at + req[at..].find("\r\n\r\n").unwrap() + 4;
        req[start..].split("\r\n").next().unwrap().to_owned()
    }

    use std::path::PathBuf;

    #[tokio::test]
    async fn uploads_in_chunks_and_retries_a_failed_chunk() {
        let (base, seen) = mock(vec![
            (200, r#"{"exists":false}"#),
            (202, r#"{"status":"partial","received":1,"total":3}"#),
            (500, "boom"),
            (202, r#"{"status":"partial","received":2,"total":3}"#),
            (201, r#"{"status":"accepted"}"#),
        ])
        .await;
        let (_dir, path) = clip();
        let meta = UploadMeta { title: Some("Ace".into()), game_id: Some(7), tag_ids: vec![1, 3], folder: Some("Deadlock".into()) };
        let fractions = Arc::new(Mutex::new(Vec::new()));
        let f = fractions.clone();
        let progress: ProgressFn = Arc::new(move |x| f.lock().unwrap().push(x));
        let r = test_client(&base).upload(&path, &meta, &progress, &CancellationToken::new()).await.unwrap();

        let id = hash::video_id(&path).unwrap();
        assert_eq!(r, UploadResult { url: format!("{base}/w/{id}"), video_id: id.clone(), duplicate: false });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        assert!(seen[0].starts_with(&format!("GET /api/upload/token/exists?video_id={id} ")));
        let parts: Vec<String> = seen[1..].iter().map(|r| field(r, "chunkPart")).collect();
        assert_eq!(parts, ["1", "2", "2", "3"]);
        for r in &seen[1..] {
            assert!(r.starts_with("POST /api/upload/token/chunked "));
            assert_eq!(field(r, "totalChunks"), "3");
            assert_eq!(field(r, "fileSize"), "10");
            assert_eq!(field(r, "fileName"), "Deadlock_2026.mp4");
            assert_eq!(field(r, "title"), "Ace");
            assert_eq!(field(r, "game_id"), "7");
            assert_eq!(field(r, "tag_ids"), "1,3");
            assert_eq!(field(r, "folder"), "Deadlock");
            assert_eq!(field(r, "checkSum"), field(&seen[1], "checkSum"));
        }
        assert_eq!(field(&seen[4], "blob"), "89");
        assert_eq!(fractions.lock().unwrap().last(), Some(&1.0));
    }

    #[tokio::test]
    async fn restarts_once_when_the_server_lost_the_parts() {
        let (base, seen) = mock(vec![
            (200, r#"{"exists":false}"#),
            (202, r#"{"received":1,"total":3}"#),
            (202, r#"{"received":1,"total":3}"#),
            (200, r#"{"exists":false}"#),
            (202, r#"{"received":1,"total":3}"#),
            (202, r#"{"received":2,"total":3}"#),
            (201, r#"{"status":"accepted"}"#),
        ])
        .await;
        let (_dir, path) = clip();
        let progress: ProgressFn = Arc::new(|_| {});
        let r = test_client(&base).upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap();
        assert!(!r.duplicate);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 7);
        assert!(seen[3].starts_with("GET /api/upload/token/exists"));
        assert_ne!(field(&seen[1], "checkSum"), field(&seen[4], "checkSum"));
        assert!(!seen[1].contains("name=\"title\""), "an empty title is not sent");
    }

    #[tokio::test]
    async fn skips_the_upload_of_a_known_clip() {
        let (base, seen) = mock(vec![(200, r#"{"exists":true,"url":"/w/abc"}"#)]).await;
        let (_dir, path) = clip();
        let progress: ProgressFn = Arc::new(|_| {});
        let r = test_client(&base).upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap();
        assert!(r.duplicate);
        assert_eq!(r.url, format!("{base}/w/abc"));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn does_not_upload_the_same_clip_twice_before_the_server_scanned_it() {
        let (base, seen) = mock(vec![
            (200, r#"{"exists":false}"#),
            (202, r#"{"received":1,"total":3}"#),
            (202, r#"{"received":2,"total":3}"#),
            (201, r#"{"status":"accepted"}"#),
        ])
        .await;
        let (_dir, path) = clip();
        let progress: ProgressFn = Arc::new(|_| {});
        let client = test_client(&base);
        let first = client.upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap();
        let second = client.upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap();
        assert!(!first.duplicate && second.duplicate);
        assert_eq!(first.url, second.url);
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn reports_a_duplicate_found_after_upload() {
        let (base, _) = mock(vec![
            (200, r#"{"exists":false}"#),
            (202, r#"{"received":1,"total":3}"#),
            (202, r#"{"received":2,"total":3}"#),
            (409, r#"{"error":"duplicate","video_id":"abc","url":"/w/abc"}"#),
        ])
        .await;
        let (_dir, path) = clip();
        let progress: ProgressFn = Arc::new(|_| {});
        let r = test_client(&base).upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap();
        assert!(r.duplicate);
        assert_eq!(r.url, format!("{base}/w/abc"));
    }

    #[tokio::test]
    async fn stops_on_a_bad_token() {
        let (base, seen) = mock(vec![(200, r#"{"exists":false}"#), (401, "Invalid upload token.")]).await;
        let (_dir, path) = clip();
        let progress: ProgressFn = Arc::new(|_| {});
        let e = test_client(&base).upload(&path, &UploadMeta::default(), &progress, &CancellationToken::new()).await.unwrap_err();
        assert!(message(e).contains("rejected the upload token"));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn reads_options() {
        let (base, _) = mock(vec![(
            200,
            r##"{"default_folder":"uploads","folders":{"video":["Deadlock"],"image":[]},
                "folder_rules":{"video":[{"folder":"Deadlock","game_id":2,"game":"Deadlock"}],"image":[]},
                "games":[{"id":2,"name":"Deadlock","steamgriddb_id":5}],
                "tags":[{"id":1,"name":"ace","color":"#f00"},{"id":2,"name":"fail","color":null}]}"##,
        )])
        .await;
        let o = test_client(&base).options().await.unwrap();
        assert_eq!(o.default_folder.as_deref(), Some("uploads"));
        assert_eq!(o.games, vec![Game { id: 2, name: "Deadlock".into() }]);
        assert_eq!(o.tags[1], Tag { id: 2, name: "fail".into(), color: None });
        assert_eq!(o.folder_rules, vec![FolderRule { folder: "Deadlock".into(), game_id: 2 }]);
    }

    #[tokio::test]
    async fn reports_an_unreachable_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let e = test_client(&base).check().await.unwrap_err();
        assert!(message(e).starts_with(&format!("Cannot reach {base}: ")));
    }

    /// Uploads a real file to a real Fireshare, twice (the second time must find it):
    /// `FIRESHARE_URL=… FIRESHARE_TOKEN=fsk_… FIRESHARE_CLIP=<mp4> cargo test live_upload -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_upload() {
        let (Ok(url), Ok(token), Ok(clip)) =
            (std::env::var("FIRESHARE_URL"), std::env::var("FIRESHARE_TOKEN"), std::env::var("FIRESHARE_CLIP"))
        else {
            eprintln!("FIRESHARE_URL, FIRESHARE_TOKEN or FIRESHARE_CLIP not set, skipping");
            return;
        };
        let client = Client::new(&url, &token).unwrap();
        println!("user: {}", client.check().await.unwrap());
        let options = client.options().await.unwrap();
        println!("options: {options:?}");
        let meta = UploadMeta {
            title: Some("SteamReplay live test".into()),
            game_id: options.games.first().map(|g| g.id),
            tag_ids: options.tags.iter().map(|t| t.id).take(1).collect(),
            folder: None,
        };
        let calls = Arc::new(Mutex::new(0u32));
        let c = calls.clone();
        let progress: ProgressFn = Arc::new(move |_| *c.lock().unwrap() += 1);
        let first = client.upload(Path::new(&clip), &meta, &progress, &CancellationToken::new()).await.unwrap();
        println!("first: {first:?} after {} progress reports", calls.lock().unwrap());
        let second = client.upload(Path::new(&clip), &meta, &progress, &CancellationToken::new()).await;
        println!("second: {second:?}");
    }
}
