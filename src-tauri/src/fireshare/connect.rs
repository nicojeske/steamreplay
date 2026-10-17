//! Connecting to Fireshare without a hand-made token (M12): a device-code flow. The app asks
//! for a code, the user allows it on `<server>/connect?code=…` in the browser, and the app
//! collects an upload token of its own by polling.

use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::client::{error_chain, http_client, normalize_url, status_error};
use crate::error::{AppError, AppResult};

/// Polls never come faster than this, whatever the server asks for.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// A started request, waiting for the user's decision.
#[derive(Debug, Clone)]
pub struct Pending {
    pub base: String,
    device_code: String,
    pub user_code: String,
    /// The page the user allows the app on.
    pub verification_url: String,
    expires_at: Instant,
    interval: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Connected {
    pub username: String,
    pub secret: String,
}

#[derive(Deserialize)]
struct StartReply {
    device_code: String,
    user_code: String,
    verification_path: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct PollReply {
    status: String,
    username: Option<String>,
    token: Option<TokenReply>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct TokenReply {
    secret: String,
}

/// `SteamReplay on DESKTOP-X`, the name of the token on the server.
pub fn client_name() -> String {
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_default();
    match host.trim() {
        "" => "SteamReplay".to_owned(),
        h => format!("SteamReplay on {h}"),
    }
}

fn unreachable(base: &str, e: &reqwest::Error) -> AppError {
    AppError::Fireshare(format!("Cannot reach {base}: {}", error_chain(e)))
}

fn no_connect_api() -> AppError {
    AppError::Fireshare("This Fireshare can't connect apps yet. Update the server, or paste an upload token under Advanced.".into())
}

fn retry_after(resp: &reqwest::Response) -> Option<Duration> {
    let secs = resp.headers().get("retry-after")?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

/// Starts a request on the server at `url`.
pub async fn start(url: &str, client_name: &str) -> AppResult<Pending> {
    let base = normalize_url(url)?;
    let resp = http_client()?
        .post(format!("{base}/api/connect/start"))
        .json(&json!({ "client_name": client_name }))
        .send()
        .await
        .map_err(|e| unreachable(&base, &e))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| unreachable(&base, &e))?;
    match status.as_u16() {
        200..=299 => {}
        404 | 405 => return Err(no_connect_api()),
        429 => return Err(AppError::Fireshare("Too many connection attempts. Try again in a few minutes.".into())),
        _ => return Err(status_error(status, &body)),
    }
    // An HTML page here is a server without the route behind a catch-all.
    let reply: StartReply = serde_json::from_str(&body).map_err(|_| no_connect_api())?;
    Ok(Pending {
        verification_url: format!("{base}/{}", reply.verification_path.trim_start_matches('/')),
        base,
        device_code: reply.device_code,
        user_code: reply.user_code,
        expires_at: Instant::now() + Duration::from_secs(reply.expires_in),
        interval: Duration::from_secs(reply.interval).max(MIN_INTERVAL),
    })
}

/// Polls until the user allowed or denied the request, it expired, or `cancel` fires. Network
/// errors are retried until the request expires: a laptop that drops off Wi-Fi for a moment
/// keeps waiting.
pub async fn wait(pending: &Pending, cancel: &CancellationToken) -> AppResult<Connected> {
    let http = http_client()?;
    let expired = || AppError::Fireshare("The code expired before it was approved. Click Connect again.".into());
    loop {
        let req = http
            .post(format!("{}/api/connect/token", pending.base))
            .json(&json!({ "device_code": pending.device_code }));
        let sent = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            r = req.send() => r,
        };
        let mut pause = pending.interval;
        match sent {
            Err(e) => log::warn!("Fireshare connect poll failed, retrying: {}", error_chain(&e)),
            Ok(resp) if resp.status() == StatusCode::TOO_MANY_REQUESTS => {
                pause = retry_after(&resp).unwrap_or(pending.interval).max(pending.interval);
            }
            Ok(resp) if resp.status().is_server_error() => log::warn!("Fireshare connect poll: HTTP {}", resp.status()),
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.map_err(|e| unreachable(&pending.base, &e))?;
                if matches!(status.as_u16(), 404 | 405) {
                    return Err(no_connect_api());
                }
                if !status.is_success() {
                    return Err(status_error(status, &body));
                }
                let reply: PollReply = serde_json::from_str(&body).map_err(|_| status_error(status, &body))?;
                match reply.status.as_str() {
                    "approved" => {
                        let (Some(username), Some(token)) = (reply.username, reply.token) else {
                            return Err(status_error(status, &body));
                        };
                        return Ok(Connected { username, secret: token.secret });
                    }
                    "denied" => {
                        let why = reply.error.map(|e| format!(" {e}")).unwrap_or_default();
                        return Err(AppError::Fireshare(format!("The connection was denied in Fireshare.{why}")));
                    }
                    "expired" => return Err(expired()),
                    _ => {}
                }
            }
        }
        if Instant::now() + pause >= pending.expires_at {
            return Err(expired());
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            _ = tokio::time::sleep(pause) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_server::serve;
    use super::*;

    fn message(e: AppError) -> String {
        match e {
            AppError::Fireshare(m) => m,
            other => panic!("not a Fireshare error: {other:?}"),
        }
    }

    const STARTED: &str = r#"{"device_code":"dev123","user_code":"BCDF-GH23","verification_path":"/connect?code=BCDF-GH23","expires_in":600,"interval":2}"#;

    async fn started(base: &str) -> Pending {
        let mut p = start(base, "SteamReplay on test").await.unwrap();
        p.interval = Duration::ZERO;
        p
    }

    #[tokio::test]
    async fn polls_until_approved() {
        let (base, seen) = serve(
            vec![
                (201, STARTED),
                (200, r#"{"status":"pending"}"#),
                (0, ""),
                (200, r#"{"status":"pending"}"#),
                (200, r#"{"status":"approved","username":"nico","token":{"id":1,"name":"x","secret":"fsk_new"}}"#),
            ],
            None,
        )
        .await;
        let p = started(&base).await;
        assert_eq!(p.user_code, "BCDF-GH23");
        assert_eq!(p.verification_url, format!("{base}/connect?code=BCDF-GH23"));
        let c = wait(&p, &CancellationToken::new()).await.unwrap();
        assert_eq!(c, Connected { username: "nico".into(), secret: "fsk_new".into() });
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        assert!(seen[0].starts_with("POST /api/connect/start ") && seen[0].contains(r#""client_name":"SteamReplay on test""#));
        assert!(seen[1].starts_with("POST /api/connect/token ") && seen[1].contains(r#""device_code":"dev123""#));
    }

    #[tokio::test]
    async fn reports_denied_and_expired() {
        let (base, _) = serve(
            vec![(201, STARTED), (200, r#"{"status":"denied"}"#), (201, STARTED), (200, r#"{"status":"expired"}"#)],
            None,
        )
        .await;
        let e = wait(&started(&base).await, &CancellationToken::new()).await.unwrap_err();
        assert!(message(e).contains("denied"));
        let e = wait(&started(&base).await, &CancellationToken::new()).await.unwrap_err();
        assert!(message(e).contains("expired"));
    }

    #[tokio::test]
    async fn explains_a_server_without_connect() {
        let (base, _) = serve(vec![(404, "<html>Not found</html>"), (200, "<html>app</html>")], None).await;
        assert!(message(start(&base, "x").await.unwrap_err()).contains("can't connect apps"));
        assert!(message(start(&base, "x").await.unwrap_err()).contains("can't connect apps"));
    }

    #[tokio::test]
    async fn stops_waiting_when_cancelled() {
        let (base, _) = serve(vec![(201, STARTED), (200, r#"{"status":"pending"}"#)], None).await;
        let mut p = started(&base).await;
        p.interval = Duration::from_secs(30);
        let cancel = CancellationToken::new();
        let c = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c.cancel();
        });
        let t = Instant::now();
        assert!(matches!(wait(&p, &cancel).await, Err(AppError::Cancelled)));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    /// Connects to a real Fireshare; approve the printed code in the browser within 10 minutes:
    /// `FIRESHARE_URL=… cargo test live_connect -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_connect() {
        let Ok(url) = std::env::var("FIRESHARE_URL") else {
            eprintln!("FIRESHARE_URL not set, skipping");
            return;
        };
        let p = start(&url, &client_name()).await.unwrap();
        println!("approve {} on {}", p.user_code, p.verification_url);
        let c = wait(&p, &CancellationToken::new()).await.unwrap();
        println!("connected as {} with {}…", c.username, &c.secret[..12]);
        let client = super::super::Client::new(&url, &c.secret).unwrap();
        assert_eq!(client.check().await.unwrap(), c.username);
        client.revoke().await.unwrap();
        assert!(client.check().await.is_err(), "the token still works after revoke");
    }

    #[test]
    fn names_the_client() {
        assert!(client_name().starts_with("SteamReplay"));
    }
}
