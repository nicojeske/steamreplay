//! A scripted HTTP server for the Fireshare tests.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A server that answers each request with the next scripted response and records the
/// requests (request line plus body, as text). Status 0 closes the connection without an
/// answer. With `auth`, every request must carry that `Authorization` value.
pub async fn serve(script: Vec<(u16, &'static str)>, auth: Option<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
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
            if let Some(auth) = auth {
                assert!(
                    head.lines().any(|l| l.eq_ignore_ascii_case(&format!("authorization: {auth}"))),
                    "no `{auth}` in {head}"
                );
            }
            let line = head.lines().next().unwrap_or_default().to_owned();
            log.lock().unwrap().push(format!("{line}\n{}", String::from_utf8_lossy(&buf[header_end..])));
            if status == 0 {
                drop(sock);
                continue;
            }
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
