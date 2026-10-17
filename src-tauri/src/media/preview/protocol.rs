//! The `replay://` URI scheme (Windows: `http://replay.localhost/…`):
//! - `replay://localhost/preview/<urlencoded id>/<s>.mp4`: segment `s` of an open preview session,
//! - `replay://localhost/chunk/<urlencoded id>/<v|a>/<init|n>.m4s`: a raw Steam DASH file of an
//!   open preview session (direct playback, M11),
//! - `replay://localhost/thumb/<urlencoded id>.jpg`: the library thumbnail of a session (M9).

use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager};

use super::{MIME, PreviewState};
use crate::error::AppError;
use crate::media::chunks;

/// A raw DASH file: video or audio, init segment or chunk number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkFile {
    pub video: bool,
    /// `None` for the init segment.
    pub chunk: Option<u32>,
}

impl ChunkFile {
    pub fn file_name(self) -> String {
        match (self.video, self.chunk) {
            (true, None) => chunks::VIDEO_INIT.to_owned(),
            (false, None) => chunks::AUDIO_INIT.to_owned(),
            (true, Some(n)) => chunks::video_chunk(n),
            (false, Some(n)) => chunks::audio_chunk(n),
        }
    }

    fn mime(self) -> &'static str {
        if self.video { "video/mp4" } else { "audio/mp4" }
    }
}

/// `/chunk/<id>/<v|a>/<init|n>.m4s` → `(id, file)`.
pub fn parse_chunk_path(path: &str) -> Option<(String, ChunkFile)> {
    let rest = path.strip_prefix("/chunk/")?;
    let (rest, file) = rest.rsplit_once('/')?;
    let (id, kind) = rest.rsplit_once('/')?;
    let video = match kind {
        "v" => true,
        "a" => false,
        _ => return None,
    };
    let stem = file.strip_suffix(".m4s")?;
    let chunk = if stem == "init" { None } else { Some(stem.parse().ok()?) };
    let id = percent_decode_str(id).decode_utf8().ok()?.into_owned();
    (!id.is_empty()).then_some((id, ChunkFile { video, chunk }))
}

/// `/preview/<id>/<s>.mp4` → `(id, s)`.
pub fn parse_path(path: &str) -> Option<(String, u32)> {
    let rest = path.strip_prefix("/preview/")?;
    let (id, file) = rest.rsplit_once('/')?;
    let s = file.strip_suffix(".mp4")?.parse().ok()?;
    let id = percent_decode_str(id).decode_utf8().ok()?.into_owned();
    (!id.is_empty()).then_some((id, s))
}

/// `bytes=a-b`, `bytes=a-` or `bytes=-n` → inclusive `(start, end)` inside `len`.
pub fn parse_range(value: &str, len: usize) -> Option<(usize, usize)> {
    let spec = value.trim().strip_prefix("bytes=")?;
    if spec.contains(',') || len == 0 {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", n) => {
            let n: usize = n.parse().ok()?;
            (len.saturating_sub(n), len - 1)
        }
        (a, "") => (a.parse().ok()?, len - 1),
        (a, b) => (a.parse().ok()?, b.parse::<usize>().ok()?.min(len - 1)),
    };
    (start <= end && start < len).then_some((start, end))
}

fn response(status: StatusCode, content_type: &str) -> tauri::http::response::Builder {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        // The dev server (and the app page on Linux) is another origin than `replay://`.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "Content-Length, Content-Range")
}

fn error(status: StatusCode, message: String) -> Response<Cow<'static, [u8]>> {
    response(status, "text/plain; charset=utf-8")
        .body(Cow::Owned(message.into_bytes()))
        .expect("valid response")
}

async fn thumbnail(app: &AppHandle, id: String) -> Response<Cow<'static, [u8]>> {
    let path = match crate::commands::thumbnail(app, id.clone()).await {
        Ok(p) => p,
        Err(AppError::NotFound(m)) => return error(StatusCode::NOT_FOUND, m),
        Err(e) => {
            log::info!("thumbnail of {id}: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    };
    match tokio::fs::read(&path).await {
        Ok(data) => response(StatusCode::OK, "image/jpeg")
            .header(header::CACHE_CONTROL, "max-age=86400")
            .body(Cow::Owned(data))
            .expect("valid response"),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", path.display())),
    }
}

/// The whole file, or the requested range of it.
fn file_response(request: &Request<Vec<u8>>, content_type: &str, data: Vec<u8>) -> Response<Cow<'static, [u8]>> {
    let len = data.len();
    // The MSE player fetches whole files; ranges are for a plain `<video src>`.
    let range = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| parse_range(v, len));
    match range {
        Some((start, end)) => response(StatusCode::PARTIAL_CONTENT, content_type)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Cow::Owned(data[start..=end].to_vec()))
            .expect("valid response"),
        None => response(StatusCode::OK, content_type)
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Cow::Owned(data))
            .expect("valid response"),
    }
}

async fn chunk(app: &AppHandle, request: &Request<Vec<u8>>, id: String, file: ChunkFile) -> Response<Cow<'static, [u8]>> {
    let Some(session) = app.state::<PreviewState>().get(&id) else {
        return error(StatusCode::NOT_FOUND, format!("preview of {id} is not open"));
    };
    let path = session.dir().join(file.file_name());
    match tokio::fs::read(&path).await {
        Ok(data) => file_response(request, file.mime(), data),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => error(StatusCode::NOT_FOUND, format!("{} not found", path.display())),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", path.display())),
    }
}

pub async fn handle(app: &AppHandle, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    if let Some(id) = crate::media::thumbs::parse_path(request.uri().path()) {
        return thumbnail(app, id).await;
    }
    if let Some((id, file)) = parse_chunk_path(request.uri().path()) {
        return chunk(app, &request, id, file).await;
    }
    let Some((id, s)) = parse_path(request.uri().path()) else {
        return error(StatusCode::NOT_FOUND, format!("unknown path {}", request.uri().path()));
    };
    let Some(session) = app.state::<PreviewState>().get(&id) else {
        return error(StatusCode::NOT_FOUND, format!("preview of {id} is not open"));
    };
    let path = match session.segment(s).await {
        Ok(p) => p,
        Err(AppError::NotFound(m)) => return error(StatusCode::NOT_FOUND, m),
        Err(AppError::Cancelled) => return error(StatusCode::SERVICE_UNAVAILABLE, "cancelled".to_owned()),
        Err(e) => {
            log::warn!("preview segment {s} of {id}: {e}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    };
    match tokio::fs::read(&path).await {
        Ok(data) => file_response(&request, MIME, data),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(
            parse_path("/preview/10000001%2Fbg%2Fbg_1445790_20261015_210120/12.mp4"),
            Some(("10000001/bg/bg_1445790_20261015_210120".to_owned(), 12))
        );
        assert_eq!(parse_path("/preview/a/b/3.mp4"), Some(("a/b".to_owned(), 3)));
        assert_eq!(parse_path("/preview//3.mp4"), None);
        assert_eq!(parse_path("/preview/x/3.webm"), None);
        assert_eq!(parse_path("/other/x/3.mp4"), None);
        assert_eq!(parse_path("/preview/x/-1.mp4"), None);
    }

    #[test]
    fn chunk_paths() {
        let (id, f) = parse_chunk_path("/chunk/10000001%2Fbg%2Fbg_1/v/init.m4s").unwrap();
        assert_eq!(id, "10000001/bg/bg_1");
        assert_eq!(f, ChunkFile { video: true, chunk: None });
        assert_eq!(f.file_name(), "init-stream0.m4s");
        let (_, f) = parse_chunk_path("/chunk/x/a/3418.m4s").unwrap();
        assert_eq!(f.file_name(), "chunk-stream1-03418.m4s");
        assert_eq!(parse_chunk_path("/chunk/x/v/12.m4s").unwrap().1.file_name(), "chunk-stream0-00012.m4s");
        assert_eq!(parse_chunk_path("/chunk/x/s/1.m4s"), None);
        assert_eq!(parse_chunk_path("/chunk/x/v/1.mp4"), None);
        assert_eq!(parse_chunk_path("/chunk/x/v/-1.m4s"), None);
        assert_eq!(parse_chunk_path("/chunk//v/1.m4s"), None);
        assert_eq!(parse_chunk_path("/chunk/v/1.m4s"), None);
        // Files outside the session folder cannot be named.
        assert_eq!(parse_chunk_path("/chunk/x/v/..%2F..%2Fetc.m4s"), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-1", 1000), None);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }
}
