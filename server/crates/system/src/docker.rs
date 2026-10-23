//! A small Docker Engine API client over the unix socket (`/var/run/docker.sock`), for "Update now"
//! (update.rs) and the self-update helper (selfupdate.rs). HTTP/1.1 through hyper on a `UnixStream`, one
//! connection per request. Paths are unversioned: the daemon answers in its own current API version.
use http_body_util::{BodyExt, Full};
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
