//! The `replay://` URI scheme. `replay://localhost/preview/<urlencoded id>/<s>.mp4` returns
//! segment `s` of an open preview session (Windows: `http://replay.localhost/…`).

use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager};

use super::{MIME, PreviewState};
use crate::error::AppError;

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

pub async fn handle(app: &AppHandle, request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
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
    let data = match tokio::fs::read(&path).await {
        Ok(d) => d,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, format!("{}: {e}", path.display())),
    };
    let len = data.len();
    // The MSE player fetches whole segments; ranges are for a plain `<video src>`.
    let range = request.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| parse_range(v, len));
    match range {
        Some((start, end)) => response(StatusCode::PARTIAL_CONTENT, MIME)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Cow::Owned(data[start..=end].to_vec()))
            .expect("valid response"),
        None => response(StatusCode::OK, MIME)
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Cow::Owned(data))
            .expect("valid response"),
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
