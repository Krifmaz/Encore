//! The group's event stream (`GET /api/group/{id}/events?device_id=`, client-shapes §3.17) as an
//! iced subscription: a `state` on connect and on every change, `ended` with a reason, a
//! `: keep-alive` comment every 15 s. A stream silent for 45 s (three keep-alives missed) is
//! taken as dead and opened again; so is one the network or a restarting server dropped (1 s,
//! doubling to 15 s). A 404 / 403 / 410 is said once ([`Ev::Gone`]) and the stream waits: the
//! player joins again with the code or leaves, and a new stream starts under a new key.

use crate::api;
use iced::futures::{SinkExt, Stream, StreamExt};
use iced::Subscription;
use serde_json::Value;
use std::time::Duration;

/// What the stream says.
#[derive(Clone, Debug)]
pub enum Ev {
    /// A group state (§2.2).
    State(Value),
    /// The group is over for this device: `ended`, `removed`, `idle`, `off`, `expired`.
    Ended(String),
    /// The server answered the stream with this status and `detail` (404: dropped, or the group
    /// is gone; 403: group listens are turned off).
    Gone(u16, String),
}

/// What a stream is opened with: the server, the credential header, the group, this device and
/// a generation (a new one after joining again).
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Key {
    pub base: String,
    pub auth: (String, String),
    pub group: String,
    pub device: String,
    pub epoch: u64,
}

pub fn subscription(key: Key) -> Subscription<Ev> {
    Subscription::run_with(key, stream)
}

fn stream(key: &Key) -> impl Stream<Item = Ev> {
    let key = key.clone();
    iced::stream::channel(32, async move |mut out: iced::futures::channel::mpsc::Sender<Ev>| {
        let Ok(client) = reqwest::Client::builder()
            .user_agent(api::user_agent())
            .connect_timeout(Duration::from_secs(8))
            // three missed keep-alives: the connection is dead even if nobody said so
            .read_timeout(Duration::from_secs(45))
            .build()
        else {
            return;
        };
        let url = format!(
            "{}/api/group/{}/events?device_id={}",
            key.base,
            crate::pages::urlencode(&key.group),
            crate::pages::urlencode(&key.device)
        );
        let mut wait = 1u64;
        loop {
            let opened = std::time::Instant::now();
            // on the address in use, asked again at every reconnect (the local network address while it answers)
            let sent = client
                .get(crate::route::net(&url))
                .header("Accept", "text/event-stream")
                .header("Cache-Control", "no-cache")
                .header(key.auth.0.as_str(), key.auth.1.as_str())
                .send()
                .await;
            match sent {
                Ok(res) if res.status().is_success() => {
                    let mut body = res.bytes_stream();
                    let mut buf: Vec<u8> = Vec::new();
                    while let Some(Ok(chunk)) = body.next().await {
                        buf.extend_from_slice(&chunk);
                        while let Some(end) = block_end(&buf) {
                            let block: Vec<u8> = buf.drain(..end).collect();
                            let Some(ev) = parse(&String::from_utf8_lossy(&block)) else {
                                continue;
                            };
                            let over = matches!(ev, Ev::Ended(_));
                            if out.send(ev).await.is_err() {
                                return;
                            }
                            if over {
                                // the group is over for this device: nothing more to read
                                return iced::futures::future::pending().await;
                            }
                        }
                    }
                }
                Ok(res) if matches!(res.status().as_u16(), 401 | 403 | 404 | 410) => {
                    let status = res.status().as_u16();
                    let text = res.text().await.unwrap_or_default();
                    let detail = serde_json::from_str::<Value>(&text)
                        .ok()
                        .map(|v| crate::fmt::s(&v, "detail"))
                        .unwrap_or_default();
                    let _ = out.send(Ev::Gone(status, detail)).await;
                    // the player decides (join again, or leave); a new key opens a new stream
                    return iced::futures::future::pending().await;
                }
                // the server is away or restarting, or the connection dropped: again in a moment
                _ => {}
            }
            if opened.elapsed() > Duration::from_secs(60) {
                wait = 1;
            }
            tokio::time::sleep(Duration::from_secs(wait)).await;
            wait = (wait * 2).min(15);
        }
    })
}

/// Where the first event block in `buf` ends (after its blank line), with `\n` or `\r\n` lines.
fn block_end(buf: &[u8]) -> Option<usize> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| i + 2);
    let crlf = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// One event block (`event:` and `data:` lines; comments ignored).
pub fn parse(block: &str) -> Option<Ev> {
    let mut event = String::new();
    let mut data = String::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with(':') {
            continue;
        }
        if let Some(v) = line.strip_prefix("event:") {
            event = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(v.strip_prefix(' ').unwrap_or(v));
        }
    }
    if data.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(&data).ok()?;
    match event.as_str() {
        "" | "state" => Some(Ev::State(v)),
        "ended" => Some(Ev::Ended(crate::fmt::s(&v, "reason"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_read_like_the_server_writes_them() {
        let state = parse("event: state\ndata: {\"id\":\"g1\",\"rev\":3}\n\n").unwrap();
        assert!(matches!(state, Ev::State(v) if v["rev"] == 3));
        assert!(matches!(parse("event: ended\ndata: {\"reason\":\"removed\"}\n\n"), Some(Ev::Ended(r)) if r == "removed"));
        assert!(parse(": keep-alive\n\n").is_none());
        assert!(parse("event: other\ndata: {}\n\n").is_none());
        assert_eq!(block_end(b": keep-alive\n\nevent: state\n"), Some(14));
        assert_eq!(block_end(b"data: {}\r\n\r\n"), Some(12));
        assert_eq!(block_end(b"data: {}\n"), None);
    }
}
