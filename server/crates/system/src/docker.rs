//! A small Docker Engine API client over the unix socket (`/var/run/docker.sock`), for "Update now"
//! (update.rs), the self-update helper (selfupdate.rs) and the Apple Music wrapper Encore runs beside itself
//! (encore_requests::wrapper: an image build fed from a file, its streamed answer, a container's logs).
//! HTTP/1.1 through hyper on a `UnixStream`, one connection per request. Paths are unversioned: the daemon
//! answers in its own current API version.
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::Frame;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the socket is mounted when the owner wants "Update now".
pub const SOCKET: &str = "/var/run/docker.sock";

/// Most calls answer at once; stopping a container waits up to its stop timeout (30 s) first.
const TIMEOUT: Duration = Duration::from_secs(60);

/// A Docker Engine API client on one unix socket.
#[derive(Clone, Debug)]
pub struct Docker {
    /// the socket's path
    sock: PathBuf,
}

impl Docker {
    /// A client for the socket at `sock` (nothing is opened until a call).
    pub fn new(sock: impl AsRef<Path>) -> Docker {
        Docker { sock: sock.as_ref().to_path_buf() }
    }

    /// One request: (status, body bytes). Errors are about the socket itself (refused, timed out, cut off).
    pub async fn request(&self, method: &str, path: &str, body: Option<&Value>, timeout: Duration) -> anyhow::Result<(u16, Bytes)> {
        let call = async {
            let stream = tokio::net::UnixStream::connect(&self.sock).await?;
            let (mut tx, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(TokioIo::new(stream)).await?;
            let driver = tokio::spawn(async move {
                let _ = conn.await;
            });
            let bytes = body.map(|b| Bytes::from(b.to_string())).unwrap_or_default();
            let mut req = hyper::Request::builder().method(method).uri(path).header(hyper::header::HOST, "docker");
            if body.is_some() {
                req = req.header(hyper::header::CONTENT_TYPE, "application/json");
            }
            let resp = tx.send_request(req.body(Full::new(bytes))?).await?;
            let status = resp.status().as_u16();
            let out = resp.into_body().collect().await?.to_bytes();
            driver.abort();
            anyhow::Ok((status, out))
        };
        match tokio::time::timeout(timeout, call).await {
            Ok(r) => r,
            Err(_) => anyhow::bail!("no answer within {} s", timeout.as_secs()),
        }
    }

    /// A call with a JSON answer (Null when the answer is empty or not JSON, e.g. 204 or `_ping`'s "OK").
    pub async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<(u16, Value)> {
        let (code, bytes) = self.request(method, path, body, TIMEOUT).await?;
        Ok((code, serde_json::from_slice(&bytes).unwrap_or(Value::Null)))
    }

    /// `GET path`: the status and the JSON answer.
    pub async fn get(&self, path: &str) -> anyhow::Result<(u16, Value)> {
        self.call("GET", path, None).await
    }

    /// `POST path` with an optional JSON body: the status and the JSON answer.
    pub async fn post(&self, path: &str, body: Option<&Value>) -> anyhow::Result<(u16, Value)> {
        self.call("POST", path, body).await
    }

    /// `DELETE path`: the status and the JSON answer.
    pub async fn delete(&self, path: &str) -> anyhow::Result<(u16, Value)> {
        self.call("DELETE", path, None).await
    }

    /// `GET /containers/<id>/json`: Some(inspect) when Docker knows the container.
    pub async fn inspect(&self, id: &str) -> anyhow::Result<Option<Value>> {
        let (code, v) = self.get(&format!("/containers/{}/json", urlencoding::encode(id))).await?;
        match code {
            200 => Ok(Some(v)),
            404 => Ok(None),
            _ => anyhow::bail!("Docker answered {code}: {}", message(&v)),
        }
    }

    /// `GET /images/<name>/json`: whether Docker has the image `name` (`repo:tag`).
    pub async fn image_exists(&self, name: &str) -> anyhow::Result<bool> {
        let (code, v) = self.get(&format!("/images/{}/json", urlencoding::encode(name))).await?;
        match code {
            200 => Ok(true),
            404 => Ok(false),
            _ => anyhow::bail!("Docker answered {code}: {}", message(&v)),
        }
    }

    /// A request whose body is the file at `body` (sent as `content_type`, read in chunks, never whole in
    /// memory), with the answer handed to `on_line` one line at a time as it arrives: an image build's progress
    /// (`POST /build`, one JSON object per line). Gives back the status; the time limit counts the whole call.
    pub async fn stream_file(&self, method: &str, path: &str, body: &Path, content_type: &str, timeout: Duration, mut on_line: impl FnMut(&str) + Send) -> anyhow::Result<u16> {
        use futures_util::TryStreamExt;
        let call = async {
            let file = tokio::fs::File::open(body).await?;
            let len = file.metadata().await?.len();
            // the file as a stream of body frames
            let frames = tokio_util::io::ReaderStream::with_capacity(file, 1 << 16).map_ok(Frame::data);
            let stream = tokio::net::UnixStream::connect(&self.sock).await?;
            let (mut tx, conn) = hyper::client::conn::http1::handshake::<_, StreamBody<_>>(TokioIo::new(stream)).await?;
            let driver = tokio::spawn(async move {
                let _ = conn.await;
            });
            let req = hyper::Request::builder().method(method).uri(path).header(hyper::header::HOST, "docker")
                .header(hyper::header::CONTENT_TYPE, content_type).header(hyper::header::CONTENT_LENGTH, len)
                .body(StreamBody::new(frames))?;
            let resp = tx.send_request(req).await?;
            let status = resp.status().as_u16();
            let mut body = resp.into_body();
            // bytes of a line not finished yet: a chunk may end anywhere
            let mut pending: Vec<u8> = Vec::new();
            while let Some(frame) = body.frame().await {
                let Ok(data) = frame?.into_data() else { continue };
                pending.extend_from_slice(&data);
                while let Some(i) = pending.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = pending.drain(..=i).collect();
                    on_line(String::from_utf8_lossy(&line).trim_end());
                }
            }
            if !pending.is_empty() {
                on_line(String::from_utf8_lossy(&pending).trim_end());
            }
            driver.abort();
            anyhow::Ok(status)
        };
        match tokio::time::timeout(timeout, call).await {
            Ok(r) => r,
            Err(_) => anyhow::bail!("no answer within {} minutes", timeout.as_secs() / 60),
        }
    }

    /// The last `lines` lines a container wrote (stdout and stderr together), as text.
    pub async fn logs(&self, id: &str, lines: usize) -> anyhow::Result<String> {
        let path = format!("/containers/{}/logs?stdout=1&stderr=1&tail={lines}", urlencoding::encode(id));
        let (code, bytes) = self.request("GET", &path, None, TIMEOUT).await?;
        if code >= 400 {
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            anyhow::bail!("Docker answered {code}: {}", message(&v));
        }
        Ok(demux_logs(&bytes))
    }

    /// The container this process runs in, as Docker inspects it: the id Docker mounted /etc/hostname from
    /// (mountinfo), else the hostname (the id's first 12 characters unless the owner set one). None outside a
    /// container, or in one this Docker does not run.
    pub async fn this_container(&self) -> Option<Value> {
        let mountinfo = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
        let hostname = crate::update::hostname();
        let mut candidates: Vec<String> = crate::update::container_id_from_mountinfo(&mountinfo).into_iter().collect();
        if !hostname.is_empty() && !candidates.contains(&hostname) {
            candidates.push(hostname.clone());
        }
        for c in candidates {
            if let Ok(Some(v)) = self.inspect(&c).await {
                if v["Id"].as_str().unwrap_or("").starts_with(&c) || v["Config"]["Hostname"].as_str() == Some(hostname.as_str()) {
                    return Some(v);
                }
            }
        }
        None
    }

    /// `POST /images/create`: pulls `repo:tag` and waits for the progress stream to end. The error is
    /// Docker's own words (an HTTP error, or the `error` line inside a 200 stream).
    pub async fn pull(&self, repo: &str, tag: &str, timeout: Duration) -> Result<(), String> {
        let path = format!("/images/create?fromImage={}&tag={}", urlencoding::encode(repo), urlencoding::encode(tag));
        match self.request("POST", &path, None, timeout).await {
            Ok((code, body)) => match pull_error(code, &body) {
                Some(e) => Err(e),
                None => Ok(()),
            },
            Err(e) => Err(e.to_string()),
        }
    }
}

/// A container's log as Docker sends it without a terminal: frames of an 8-byte header (stream 1 or 2, three
/// zero bytes, the length as a big-endian u32) and that many bytes. A log of a container with a terminal has
/// no frames and is taken as it is, as is anything that does not read as frames.
pub fn demux_logs(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while rest.len() >= 8 && matches!(rest[0], 0..=2) && rest[1..4] == [0, 0, 0] {
        let n = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        if rest.len() < 8 + n {
            break;
        }
        out.extend_from_slice(&rest[8..8 + n]);
        rest = &rest[8 + n..];
    }
    // no frames at all (a terminal), or a tail that is not one: kept as written
    out.extend_from_slice(rest);
    String::from_utf8_lossy(&out).into_owned()
}

/// What went wrong in an image build's answer (`POST /build`): the `error` (or `errorDetail.message`) of one
/// of its lines. None for a progress line.
pub fn build_error(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    v["errorDetail"]["message"].as_str().or_else(|| v["error"].as_str()).map(|e| e.trim().to_string()).filter(|e| !e.is_empty())
}

/// The words of an image build's progress line (`{"stream": "Step 3/20 : RUN …\n"}`), trimmed; None when
/// the line has none (an `aux` line with the image id, an empty line).
pub fn build_text(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    v["stream"].as_str().or_else(|| v["status"].as_str()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Docker's `{"message": "..."}` (or the status line when there is none).
pub fn message(v: &Value) -> String {
    v["message"].as_str().map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).unwrap_or_else(|| "no details".into())
}

/// What went wrong with a pull, if anything: an HTTP error (401, 404, 500 with `message`) or, in a 200
/// answer, a progress object with `error` / `errorDetail.message` (the registry refused part-way).
pub fn pull_error(status: u16, body: &[u8]) -> Option<String> {
    if status >= 400 {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let text = String::from_utf8_lossy(body).trim().chars().take(300).collect::<String>();
        return Some(v["message"].as_str().map(|m| m.trim().to_string()).filter(|m| !m.is_empty())
            .unwrap_or(if text.is_empty() { format!("Docker answered {status}") } else { text }));
    }
    let mut last = None;
    for v in serde_json::Deserializer::from_slice(body).into_iter::<Value>().map_while(Result::ok) {
        let e = v["errorDetail"]["message"].as_str().or_else(|| v["error"].as_str());
        if let Some(e) = e.map(str::trim).filter(|e| !e.is_empty()) {
            last = Some(e.to_string());
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_lose_their_frame_headers() {
        let mut framed = vec![1u8, 0, 0, 0, 0, 0, 0, 3];
        framed.extend_from_slice(b"ab\n");
        framed.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 4]);
        framed.extend_from_slice(b"err\n");
        assert_eq!(demux_logs(&framed), "ab\nerr\n");
        // a container with a terminal: no frames
        assert_eq!(demux_logs(b"plain text\n"), "plain text\n");
        // a frame cut short is kept as it came
        assert_eq!(demux_logs(&[1, 0, 0, 0, 0, 0, 0, 9, b'x']), String::from_utf8_lossy(&[1, 0, 0, 0, 0, 0, 0, 9, b'x']));
    }

    #[test]
    fn build_lines_read_as_progress_or_error() {
        assert_eq!(build_text(r#"{"stream":"Step 3/20 : RUN make\n"}"#).as_deref(), Some("Step 3/20 : RUN make"));
        assert_eq!(build_text(r#"{"aux":{"ID":"sha256:x"}}"#), None);
        assert_eq!(build_error(r#"{"errorDetail":{"message":"no space left"},"error":"no space"}"#).as_deref(), Some("no space left"));
        assert_eq!(build_error(r#"{"stream":"ok"}"#), None);
        assert_eq!(build_error("not json"), None);
    }
}
