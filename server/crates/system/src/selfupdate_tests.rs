//! The self-update planner on a captured `docker inspect` answer (fixtures/docker-inspect.json, Docker 27,
//! a compose container with a named /config volume, a socket bind, a second network with a fixed address),
//! the socket client against a small axum server on a unix socket, and the helper's swap and roll-backs
//! against a mock Docker on that socket.
use super::*;
use axum::body::Bytes;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub(crate) fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/docker-inspect.json")).unwrap()
}

const NEW: &str = "ghcr.io/krifmaz/encore:0.28.0";

/// What the old image (0.27.0) set itself, for `without_image_defaults`.
fn old_image() -> Value {
    json!({ "Id": fixture()["Image"], "Config": {
        "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", "ENCORE_CONFIG_DIR=/config", "ENCORE_STATIC_DIR=/app/static",
                "ENCORE_PORT=8080", "PUID=1000", "PGID=1000", "TZ=UTC", "ENCORE_BUILD=4f1d2c3b5a69788f9e0d1c2b3a4f5e6d7c8b9a01"],
        "Cmd": ["encore"], "Entrypoint": ["/usr/bin/tini", "--", "/entrypoint.sh"], "WorkingDir": "/", "ExposedPorts": { "8080/tcp": {} },
        "Healthcheck": { "Test": ["CMD-SHELL", "curl -fsS http://127.0.0.1:8080/api/health || exit 1"], "Interval": 30000000000u64, "Timeout": 5000000000u64,
                         "StartPeriod": 20000000000u64, "Retries": 3 },
        "Labels": { "org.opencontainers.image.created": "2026-10-11T07:55:10.412Z", "org.opencontainers.image.source": "https://github.com/Krifmaz/Encore",
                    "org.opencontainers.image.version": "0.27.0" },
        "Volumes": { "/config": {} } } })
}

#[test]
fn image_references_split_into_repo_and_tag() {
    let s = |r: &str| { let (a, b) = parse_image(r); (a, b) };
    assert_eq!(s("ghcr.io/krifmaz/encore:0.28.0"), ("ghcr.io/krifmaz/encore".into(), "0.28.0".into()));
    assert_eq!(s("ghcr.io/krifmaz/encore"), ("ghcr.io/krifmaz/encore".into(), "latest".into()));
    assert_eq!(s("ghcr.io/krifmaz/encore:latest"), ("ghcr.io/krifmaz/encore".into(), "latest".into()));
    assert_eq!(s("localhost:5000/encore"), ("localhost:5000/encore".into(), "latest".into()), "a registry port is not a tag");
    assert_eq!(s("localhost:5000/encore:dev"), ("localhost:5000/encore".into(), "dev".into()));
    assert_eq!(s("encore"), ("encore".into(), "latest".into()));
    assert_eq!(s(" ghcr.io/krifmaz/encore@sha256:abc "), ("ghcr.io/krifmaz/encore".into(), "sha256:abc".into()));
}

/// Everything the owner set carries over with the new image; Docker's bookkeeping and defaults do not.
#[test]
fn the_new_container_keeps_the_settings_of_the_old_one() {
    let ins = fixture();
    let b = plan_create_body(&ins, NEW);
    assert_eq!(b["Image"], json!(NEW));
    assert_eq!(b["Env"], ins["Config"]["Env"]);
    assert_eq!(b["Cmd"], json!(["encore"]));
    assert_eq!(b["Entrypoint"], json!(["/usr/bin/tini", "--", "/entrypoint.sh"]));
    assert_eq!(b["Labels"]["com.docker.compose.service"], json!("encore"), "compose still knows its container");
    assert_eq!(b["ExposedPorts"], json!({ "8080/tcp": {} }));
    assert_eq!(b["StopSignal"], json!("SIGTERM"));
    assert!(b.get("Hostname").is_none(), "the hostname was Docker's (the id's first 12 characters)");
    assert!(b.get("Volumes").is_none() && b.get("AttachStdout").is_none());
    let hc = &b["HostConfig"];
    assert_eq!(hc["Binds"], json!(["/srv/music:/music:rw", "/var/run/docker.sock:/var/run/docker.sock:rw", "encore_config:/config:rw"]));
    assert_eq!(hc["Mounts"], json!([{ "Type": "bind", "Source": "/srv/downloads", "Target": "/downloads" }]), "/config is in Binds: no second mount");
    assert_eq!(hc["PortBindings"], json!({ "8080/tcp": [{ "HostIp": "", "HostPort": "8080" }] }));
    assert_eq!(hc["RestartPolicy"], json!({ "Name": "unless-stopped", "MaximumRetryCount": 0 }));
    assert_eq!(hc["NetworkMode"], json!("encore_default"));
    assert_eq!(hc["Devices"][0]["PathOnHost"], json!("/dev/dri"));
    assert_eq!(hc["CapAdd"], json!(["SYS_NICE"]));
    assert_eq!(hc["ExtraHosts"], json!(["host.docker.internal:host-gateway"]));
    assert_eq!(hc["LogConfig"]["Config"]["max-size"], json!("10m"));
    assert_eq!((hc["ShmSize"].as_u64(), hc["Memory"].as_u64(), hc["NanoCpus"].as_u64(), hc["MemorySwap"].as_u64()),
               (Some(67108864), Some(2147483648), Some(2000000000), Some(4294967296)));
    for gone in ["MaskedPaths", "ReadonlyPaths", "ContainerIDFile", "AutoRemove", "ConsoleSize", "Dns", "CpuShares", "Privileged", "Links", "CgroupnsMode"] {
        assert!(hc.get(gone).is_none(), "{gone}: {}", hc[gone]);
    }
    // the same networks: aliases without duplicates or the old id, the fixed address only where the owner set one
    let eps = &b["NetworkingConfig"]["EndpointsConfig"];
    assert_eq!(eps.as_object().unwrap().len(), 2);
    assert_eq!(eps["encore_default"], json!({ "Aliases": ["encore"] }));
    assert_eq!(eps["proxy"], json!({ "Aliases": ["music"], "IPAMConfig": { "IPv4Address": "172.30.0.20" } }));
    assert!(!b.to_string().contains("172.19.0.2"), "the address Docker gave is not kept");
}

#[test]
fn a_hostname_the_owner_set_is_kept() {
    let mut ins = fixture();
    ins["Config"]["Hostname"] = json!("music");
    ins["Config"]["Domainname"] = json!("home.lan");
    let b = plan_create_body(&ins, NEW);
    assert_eq!((b["Hostname"].as_str(), b["Domainname"].as_str()), (Some("music"), Some("home.lan")));
}

/// The default bridge takes neither a fixed address nor aliases (Docker refuses both there).
#[test]
fn on_the_default_bridge_no_address_and_no_aliases() {
    let mut ins = fixture();
    ins["HostConfig"]["NetworkMode"] = json!("bridge");
    ins["NetworkSettings"]["Networks"] = json!({ "bridge": { "IPAMConfig": { "IPv4Address": "172.17.0.5" }, "Aliases": ["x"], "IPAddress": "172.17.0.5" } });
    let b = plan_create_body(&ins, NEW);
    assert_eq!(b["HostConfig"]["NetworkMode"], json!("bridge"));
    assert_eq!(b["NetworkingConfig"], json!({ "EndpointsConfig": { "bridge": {} } }));
}

/// /config without a mount on it lives in an anonymous volume (the image's VOLUME): the new container mounts
/// that same volume by name, or the database would be left behind.
#[test]
fn anonymous_volumes_come_along() {
    let mut ins = fixture();
    ins["HostConfig"]["Binds"] = json!(["/srv/music:/music:rw", "/var/run/docker.sock:/var/run/docker.sock:rw"]);
    let anon = "0d4a3f0c1b6e8d2a7f9c5e3b1a8d6f4c2e0b9a7d5c3f1e8b6a4d2c0f9e7b5a3c";
    ins["Mounts"][2] = json!({ "Type": "volume", "Name": anon, "Source": format!("/var/lib/docker/volumes/{anon}/_data"), "Destination": "/config",
                               "Driver": "local", "Mode": "", "RW": true, "Propagation": "" });
    let b = plan_create_body(&ins, NEW);
    assert_eq!(b["HostConfig"]["Mounts"], json!([
        { "Type": "bind", "Source": "/srv/downloads", "Target": "/downloads" },
        { "Type": "volume", "Source": anon, "Target": "/config", "ReadOnly": false },
    ]));
}

/// Behind another container's network (a VPN container): no networks, ports, hostname, DNS or hosts of its own.
#[test]
fn a_container_on_another_containers_network_keeps_only_that() {
    let mut ins = fixture();
    ins["HostConfig"]["NetworkMode"] = json!("container:gluetun");
    ins["Config"]["Hostname"] = json!("gluetun-host");
    ins["HostConfig"]["Dns"] = json!(["1.1.1.1"]);
    let b = plan_create_body(&ins, NEW);
    assert_eq!(b["HostConfig"]["NetworkMode"], json!("container:gluetun"));
    for k in ["PortBindings", "ExtraHosts", "Dns"] {
        assert!(b["HostConfig"].get(k).is_none(), "{k}");
    }
    assert!(b.get("NetworkingConfig").is_none() && b.get("Hostname").is_none() && b.get("ExposedPorts").is_none(), "{b}");
    // host networking: no endpoints either, and no links
    ins["HostConfig"]["NetworkMode"] = json!("host");
    ins["HostConfig"]["Links"] = json!(["/db:/encore/db"]);
    let b = plan_create_body(&ins, NEW);
    assert!(b.get("NetworkingConfig").is_none() && b["HostConfig"].get("Links").is_none(), "{b}");
}

#[test]
fn links_are_written_the_way_create_takes_them() {
    let mut ins = fixture();
    ins["HostConfig"]["Links"] = json!(["/postgres:/encore/db"]);
    assert_eq!(plan_create_body(&ins, NEW)["HostConfig"]["Links"], json!(["postgres:db"]));
}

/// What only came from the old image goes, so the new image's values apply (its build id, its version
/// label, a changed entrypoint); what the owner set stays.
#[test]
fn the_old_images_own_values_give_way_to_the_new_ones() {
    let b = without_image_defaults(plan_create_body(&fixture(), NEW), &old_image());
    assert_eq!(b["Env"], json!(["TZ=Europe/Berlin", "PGID=988", "ENCORE_BASE_URL=https://music.example.org"]));
    assert!(!b.to_string().contains("ENCORE_BUILD"), "the old build id would outlive the update");
    let labels = b["Labels"].as_object().unwrap();
    assert!(labels.keys().all(|k| k.starts_with("com.docker.compose.")), "{labels:?}");
    assert_eq!(labels.len(), 10);
    for k in ["Entrypoint", "Cmd", "Healthcheck", "ExposedPorts", "WorkingDir"] {
        assert!(b.get(k).is_none(), "{k}: {}", b[k]);
    }
    assert_eq!(b["StopSignal"], json!("SIGTERM"), "set by the owner (the image has none)");
    assert_eq!(b["Image"], json!(NEW));
    assert_eq!(b["HostConfig"], plan_create_body(&fixture(), NEW)["HostConfig"]);

    // a command of the owner's own stays, with the image's entrypoint
    let mut ins = fixture();
    ins["Config"]["Cmd"] = json!(["encore", "--verbose"]);
    let b = without_image_defaults(plan_create_body(&ins, NEW), &old_image());
    assert!(b.get("Entrypoint").is_none());
    assert_eq!(b["Cmd"], json!(["encore", "--verbose"]));
    // an entrypoint of the owner's own: both stay
    ins["Config"]["Entrypoint"] = json!(["/bin/sh", "-c"]);
    ins["Config"]["Cmd"] = json!(["encore"]);
    let b = without_image_defaults(plan_create_body(&ins, NEW), &old_image());
    assert_eq!((b["Entrypoint"].clone(), b["Cmd"].clone()), (json!(["/bin/sh", "-c"]), json!(["encore"])));
}

#[test]
fn the_helper_runs_the_new_image_with_only_the_socket() {
    let ins = fixture();
    let id = ins["Id"].as_str().unwrap();
    let b = plan_helper_body(&ins, NEW, 42).unwrap();
    assert_eq!(b["Image"], json!(NEW));
    assert_eq!(b["Entrypoint"], json!(["/usr/local/bin/encore"]));
    assert_eq!(b["Cmd"], json!(["self-update-helper", "--target", id, "--image", NEW, "--schema", "42"]));
    // not removed by Docker when it ends: its log is the update's record until the server reads it
    assert_eq!(b["HostConfig"], json!({ "Binds": ["/var/run/docker.sock:/var/run/docker.sock"], "NetworkMode": "none" }));
    assert!(b.get("ExposedPorts").is_none() && b["HostConfig"].get("PortBindings").is_none());
    assert_eq!(socket_source(&ins).as_deref(), Some("/var/run/docker.sock"));
    // the socket at another host path, mounted at /run/docker.sock
    let mut other = ins.clone();
    other["Mounts"][1] = json!({ "Type": "bind", "Source": "/run/user/1000/docker.sock", "Destination": "/run/docker.sock", "RW": true });
    assert_eq!(plan_helper_body(&other, NEW, 42).unwrap()["HostConfig"]["Binds"], json!(["/run/user/1000/docker.sock:/var/run/docker.sock"]));
    // no socket bind: no helper
    let mut none = ins;
    none["Mounts"].as_array_mut().unwrap().remove(1);
    assert!(plan_helper_body(&none, NEW, 42).is_none());
}

#[test]
fn pull_errors_are_dockers_own_words() {
    use crate::docker::pull_error;
    let ok = b"{\"status\":\"Pulling from krifmaz/encore\",\"id\":\"0.28.0\"}\r\n{\"status\":\"Digest: sha256:abc\"}\r\n{\"status\":\"Status: Downloaded newer image for ghcr.io/krifmaz/encore:0.28.0\"}\r\n";
    assert_eq!(pull_error(200, ok), None);
    let bad = b"{\"status\":\"Pulling from krifmaz/encore\",\"id\":\"9.9.9\"}\r\n{\"errorDetail\":{\"message\":\"manifest unknown\"},\"error\":\"manifest unknown\"}\r\n";
    assert_eq!(pull_error(200, bad).as_deref(), Some("manifest unknown"));
    let denied = br#"{"message":"pull access denied for ghcr.io/krifmaz/encore, repository does not exist or may require 'docker login'"}"#;
    assert!(pull_error(404, denied).unwrap().starts_with("pull access denied"));
    assert_eq!(pull_error(500, b"").as_deref(), Some("Docker answered 500"));
}

// ---- a Docker on a unix socket ------------------------------------------------------------------------

/// A Docker Engine stand-in: the fixture container, an old and a new image, and the calls it was sent.
#[derive(Clone, Default)]
pub(crate) struct Mock {
    pub calls: Arc<Mutex<Vec<String>>>,
    pub bodies: Arc<Mutex<Vec<(String, Value)>>>,
    /// "create", "start", "exits": what goes wrong; "rm": the container was started with --rm; "migrates": the
    /// new container's log says it began migrating the database; "recovers": the new container runs once it
    /// was started a second time
    pub fail: Arc<Mutex<HashSet<&'static str>>>,
    /// how often the new container was started
    pub new_starts: Arc<Mutex<usize>>,
}

pub(crate) const NEW_ID: &str = "9b1e8f2a7c6d5e4f3a2b1c0d9e8f7a6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d1e0f";
pub(crate) const HELPER_ID: &str = "5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e";

impl Mock {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn body(&self, path_start: &str) -> Option<Value> {
        self.bodies.lock().unwrap().iter().find(|(p, _)| p.starts_with(path_start)).map(|(_, b)| b.clone())
    }
    fn failing(&self, what: &str) -> bool {
        self.fail.lock().unwrap().contains(what)
    }
}

fn reply(code: u16, v: Value) -> Response {
    (StatusCode::from_u16(code).unwrap(), axum::Json(v)).into_response()
}

async fn mock_handler(axum::extract::State(m): axum::extract::State<Mock>, method: Method, uri: Uri, body: Bytes) -> Response {
    let path = uri.path().to_string();
    let full = format!("{method} {}", uri.path_and_query().map(|p| p.as_str()).unwrap_or(""));
    m.calls.lock().unwrap().push(full.clone());
    if !body.is_empty() {
        m.bodies.lock().unwrap().push((full.split_once(' ').map(|x| x.1).unwrap_or("").to_string(), serde_json::from_slice(&body).unwrap_or(Value::Null)));
    }
    let mut ins = fixture();
    if m.failing("rm") {
        ins["HostConfig"]["AutoRemove"] = json!(true);
    }
    let id = ins["Id"].as_str().unwrap().to_string();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match (method.as_str(), parts.as_slice()) {
        ("GET", ["_ping"]) => "OK".into_response(),
        // the update helpers Docker still has: one that ended (exit code 1) when asked for
        ("GET", ["containers", "json"]) => reply(200, if m.failing("helper-left") { json!([{ "Id": HELPER_ID, "State": "exited" }]) } else { json!([]) }),
        ("GET", ["containers", c, "json"]) if *c == HELPER_ID => reply(200, json!({ "Id": HELPER_ID, "State": { "Status": "exited", "Running": false, "ExitCode": 1 } })),
        ("GET", ["containers", c, "logs"]) if *c == HELPER_ID => {
            let text = "encore self-update: stop: stopping encore (up to 30 s)\nencore self-update: failed: verify: the new container stopped (status exited, exit code 3): the new Encore refused the database\n";
            (StatusCode::OK, text).into_response()
        }
        ("GET", ["containers", c, "json"]) => {
            if id.starts_with(c) || *c == "encore" {
                reply(200, ins)
            } else if *c == NEW_ID {
                let running = !m.failing("exits") || (m.failing("recovers") && *m.new_starts.lock().unwrap() >= 2);
                reply(200, json!({ "Id": NEW_ID, "State": { "Status": if running { "running" } else { "exited" }, "Running": running, "Restarting": false,
                                   "ExitCode": if running { 0 } else { 3 } } }))
            } else {
                reply(404, json!({ "message": format!("No such container: {c}") }))
            }
        }
        ("GET", ["images", "json"]) => reply(200, json!([
            { "Id": "sha256:left", "RepoTags": ["<none>:<none>"], "RepoDigests": ["ghcr.io/krifmaz/encore@sha256:abc"] },
            { "Id": "sha256:other", "RepoTags": ["<none>:<none>"], "RepoDigests": ["postgres@sha256:def"] },
        ])),
        ("DELETE", ["images", ..]) => reply(200, json!([{ "Deleted": "x" }])),
        ("GET", ["images", ..]) => {
            let name = path.trim_start_matches("/images/").trim_end_matches("/json");
            if name == ins["Image"].as_str().unwrap() {
                reply(200, old_image())
            } else if name.starts_with("ghcr.io/krifmaz/encore:") {
                reply(200, json!({ "Id": "sha256:new", "Config": { "Env": ["ENCORE_BUILD=new"] } }))
            } else {
                reply(404, json!({ "message": format!("No such image: {name}") }))
            }
        }
        ("POST", ["images", "create"]) => {
            let q = uri.query().unwrap_or("");
            if q.contains("tag=9.9.9") {
                (StatusCode::OK, "{\"status\":\"Pulling from krifmaz/encore\"}\r\n{\"errorDetail\":{\"message\":\"manifest unknown\"},\"error\":\"manifest unknown\"}\r\n").into_response()
            } else if q.contains("tag=denied") {
                reply(404, json!({ "message": "pull access denied for ghcr.io/krifmaz/encore, repository does not exist or may require 'docker login'" }))
            } else {
                (StatusCode::OK, "{\"status\":\"Pulling from krifmaz/encore\"}\r\n{\"status\":\"Status: Downloaded newer image\"}\r\n").into_response()
            }
        }
        ("POST", ["containers", "create"]) => {
            if m.failing("create") {
                reply(409, json!({ "message": "Conflict. The container name \"/encore\" is already in use" }))
            } else if uri.query().unwrap_or("").contains("-update-") {
                reply(201, json!({ "Id": HELPER_ID, "Warnings": [] }))
            } else {
                reply(201, json!({ "Id": NEW_ID, "Warnings": [] }))
            }
        }
        ("POST", ["containers", c, "start"]) if *c == NEW_ID && m.failing("start") => {
            reply(500, json!({ "message": "driver failed programming external connectivity: port 8080 is already allocated" }))
        }
        ("POST", ["containers", c, "start"]) if *c == NEW_ID => {
            *m.new_starts.lock().unwrap() += 1;
            StatusCode::NO_CONTENT.into_response()
        }
        // the new server's own words: a frame of stream 2 (stderr) per line, as Docker sends a log without a terminal
        ("GET", ["containers", c, "logs"]) if *c == NEW_ID => {
            let text = if m.failing("migrates") {
                format!("2026-11-05T00:00:01Z  INFO encore_db: {}\n2026-11-05T00:00:03Z ERROR encore: database migration 0043_x failed: disk I/O error\n", encore_db::migrating_line(42, 43))
            } else {
                "\nThis database was written by a newer Encore (schema 42, this server knows 41). Start the newer image again, or restore a backup from /config/backups. Nothing was changed.\n".to_string()
            };
            let mut out = vec![];
            for l in text.split_inclusive('\n') {
                out.extend_from_slice(&[2, 0, 0, 0]);
                out.extend_from_slice(&(l.len() as u32).to_be_bytes());
                out.extend_from_slice(l.as_bytes());
            }
            (StatusCode::OK, out).into_response()
        }
        ("POST", ["containers", _, "start" | "stop" | "rename"]) => StatusCode::NO_CONTENT.into_response(),
        ("DELETE", ["containers", _]) => StatusCode::NO_CONTENT.into_response(),
        _ => reply(404, json!({ "message": "page not found" })),
    }
}

/// A short folder for the socket (a unix socket path must stay under 108 bytes).
pub(crate) fn sock_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > 40 { PathBuf::from("/tmp") } else { base };
    let d = base.join(format!("enc-dk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The mock Docker, listening on `<dir>/docker.sock`.
pub(crate) async fn serve_mock(dir: &std::path::Path) -> (Docker, Mock) {
    let m = Mock::default();
    let sock = dir.join("docker.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let app = axum::Router::new().fallback(mock_handler).with_state(m.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (Docker::new(&sock), m)
}

#[tokio::test]
async fn the_socket_client_speaks_http_over_the_unix_socket() {
    let dir = sock_dir("client");
    let (d, m) = serve_mock(&dir).await;
    assert_eq!(d.get("/_ping").await.unwrap(), (200, Value::Null), "a plain-text answer is no JSON");
    let ins = d.inspect("encore").await.unwrap().unwrap();
    assert_eq!(ins["Name"], json!("/encore"));
    assert_eq!(d.inspect("nothing").await.unwrap(), None);
    let (code, v) = d.post("/containers/create?name=x", Some(&json!({ "Image": NEW }))).await.unwrap();
    assert_eq!((code, v["Id"].as_str()), (201, Some(NEW_ID)));
    assert_eq!(m.body("/containers/create?name=x"), Some(json!({ "Image": NEW })));
    assert_eq!(d.delete("/containers/x").await.unwrap().0, 204);
    assert_eq!(d.pull(IMAGE, "0.28.0", Duration::from_secs(5)).await, Ok(()));
    assert_eq!(d.pull(IMAGE, "9.9.9", Duration::from_secs(5)).await, Err("manifest unknown".to_string()));
    assert!(m.calls().contains(&"POST /images/create?fromImage=ghcr.io%2Fkrifmaz%2Fencore&tag=0.28.0".to_string()), "{:?}", m.calls());
    // nobody on the other end
    let gone = Docker::new(dir.join("missing.sock"));
    assert!(gone.get("/_ping").await.is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

fn fast() -> Timing {
    Timing { settle: Duration::from_millis(60), poll: Duration::from_millis(20) }
}

fn short(calls: &[String]) -> Vec<String> {
    let id = fixture()["Id"].as_str().unwrap().to_string();
    calls.iter().map(|c| c.replace(&id, "OLD").replace(NEW_ID, "NEW").split("?name=encore-old-").next().unwrap().to_string()).collect()
}

/// stop → rename → create under the old name → start → keeps running → the old container goes (volumes stay).
#[tokio::test]
async fn the_helper_swaps_the_container() {
    let dir = sock_dir("swap");
    let (d, m) = serve_mock(&dir).await;
    let id = fixture()["Id"].as_str().unwrap().to_string();
    let mut lines = vec![];
    let new = run_helper(&d, &id[..12], NEW, &Preflight::default(), fast(), &mut |l| lines.push(l.to_string())).await.unwrap();
    assert_eq!(new, NEW_ID);
    let calls = short(&m.calls());
    let expect = ["GET /containers/784f60a52647/json", "GET /images/sha256:d4a92e09a51355b13a6055a235cf06c2ec9e0d079cd1ade6135629bdb98fc349/json",
                  &format!("GET /images/{NEW}/json"), "POST /containers/OLD/stop?t=30", "POST /containers/OLD/rename", "POST /containers/create?name=encore",
                  "POST /containers/NEW/start", "GET /containers/NEW/json"];
    assert_eq!(&calls[..expect.len()], &expect.map(String::from)[..], "{calls:#?}");
    let old_image = fixture()["Image"].as_str().unwrap().to_string();
    let i = calls.iter().position(|c| c == "DELETE /containers/OLD?v=false&force=true").expect("the old container goes");
    // then the old image and Encore's untagged leftovers (never another repository's)
    assert_eq!(&calls[i + 1..], &[format!("DELETE /images/{old_image}"), "GET /images/json?filters=%7B%22dangling%22%3A%5B%22true%22%5D%7D".into(),
                                   "DELETE /images/sha256:left".into()], "{calls:#?}");
    let body = m.body("/containers/create?name=encore").unwrap();
    assert_eq!(body["Image"], json!(NEW));
    assert!(!body.to_string().contains("ENCORE_BUILD"), "the old image's own values gave way: {body}");
    assert_eq!(body["HostConfig"]["Binds"][2], json!("encore_config:/config:rw"));
    assert!(lines.iter().any(|l| l.starts_with("rename: renaming encore to encore-old-")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("cleanup: done"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_old_image_and_its_encore_tags_go() {
    let old = json!({ "Id": "sha256:old", "RepoTags": ["ghcr.io/krifmaz/encore:0.35.2", "ghcr.io/krifmaz/encore:latest", "myfork/encore:keep"] });
    assert_eq!(plan_image_cleanup(&old, "sha256:new"), ["ghcr.io/krifmaz/encore:0.35.2", "ghcr.io/krifmaz/encore:latest", "sha256:old"]);
    assert!(plan_image_cleanup(&old, "sha256:old").is_empty(), "the same image: nothing goes");
    assert!(plan_image_cleanup(&Value::Null, "sha256:new").is_empty(), "an unknown old image: nothing goes");
    let list = json!([
        { "Id": "sha256:a", "RepoTags": ["<none>:<none>"], "RepoDigests": ["ghcr.io/krifmaz/encore@sha256:1"] },
        { "Id": "sha256:b", "RepoTags": null, "RepoDigests": ["ghcr.io/krifmaz/encore@sha256:2"] },
        { "Id": "sha256:c", "RepoTags": ["ghcr.io/krifmaz/encore:0.30.0"], "RepoDigests": ["ghcr.io/krifmaz/encore@sha256:3"] },
        { "Id": "sha256:d", "RepoTags": ["<none>:<none>"], "RepoDigests": ["redis@sha256:4"] },
    ]);
    assert_eq!(dangling_encore_images(&list), ["sha256:a", "sha256:b"]);
}

/// Creating the new container fails: the old one gets its name back and starts again.
#[tokio::test]
async fn a_failed_create_brings_the_old_container_back() {
    let dir = sock_dir("create");
    let (d, m) = serve_mock(&dir).await;
    m.fail.lock().unwrap().insert("create");
    let mut lines = vec![];
    let e = run_helper(&d, "encore", NEW, &Preflight::default(), fast(), &mut |l| lines.push(l.to_string())).await.unwrap_err().to_string();
    assert!(e.starts_with("create: Docker answered 409"), "{e}");
    let calls = short(&m.calls());
    assert_eq!(&calls[calls.len() - 2..], ["POST /containers/OLD/rename?name=encore", "POST /containers/OLD/start"], "{calls:#?}");
    assert!(!calls.iter().any(|c| c.starts_with("DELETE")), "nothing is removed: {calls:#?}");
    assert!(lines.iter().any(|l| l == "rollback: encore runs again on the old image"), "{lines:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The new container doesn't start, or stops again at once: it is removed and the old one comes back.
#[tokio::test]
async fn a_new_container_that_does_not_run_is_rolled_back() {
    for what in ["start", "exits"] {
        let dir = sock_dir(what);
        let (d, m) = serve_mock(&dir).await;
        m.fail.lock().unwrap().insert(what);
        let mut lines = vec![];
        let e = run_helper(&d, "encore", NEW, &Preflight::default(), fast(), &mut |l| lines.push(l.to_string())).await.unwrap_err().to_string();
        if what == "start" {
            assert!(e.starts_with("start: Docker answered 500") && e.contains("port 8080 is already allocated"), "{e}");
        } else {
            // the exit code is explained, and the new server's own words are in the log
            assert!(e.starts_with("verify: the new container stopped (status exited, exit code 3): the new Encore refused the database"), "{e}");
            assert!(lines.iter().any(|l| l.starts_with("verify: new container: This database was written by a newer Encore (schema 42, this server knows 41)")), "{lines:?}");
        }
        let calls = short(&m.calls());
        let n = calls.len();
        assert_eq!(&calls[n - 3..], ["DELETE /containers/NEW?force=true", "POST /containers/OLD/rename?name=encore", "POST /containers/OLD/start"], "{what}: {calls:#?}");
        assert!(!calls.iter().any(|c| c.starts_with("DELETE /containers/OLD")), "{what}: the old container stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// An image that is not on the host stops the helper before anything is changed.
#[tokio::test]
async fn a_missing_image_changes_nothing() {
    let dir = sock_dir("noimg");
    let (d, m) = serve_mock(&dir).await;
    let e = run_helper(&d, "encore", "example.org/other:1", &Preflight::default(), fast(), &mut |_| {}).await.unwrap_err().to_string();
    assert!(e.starts_with("inspect: ") && e.contains("is not on this host"), "{e}");
    assert!(m.calls().iter().all(|c| c.starts_with("GET ")), "{:?}", m.calls());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A container started with --rm, or a Swarm task, is never swapped: nothing is stopped.
#[tokio::test]
async fn a_container_docker_removes_on_stop_is_left_alone() {
    let dir = sock_dir("rm");
    let (d, m) = serve_mock(&dir).await;
    m.fail.lock().unwrap().insert("rm");
    let e = run_helper(&d, "encore", NEW, &Preflight::default(), fast(), &mut |_| {}).await.unwrap_err().to_string();
    assert!(e.contains("started with --rm") && e.ends_with("nothing was changed"), "{e}");
    assert!(m.calls().iter().all(|c| c.starts_with("GET ")), "{:?}", m.calls());
    let mut swarm = fixture();
    swarm["Config"]["Labels"]["com.docker.swarm.service.id"] = json!("x1y2z3");
    assert!(cannot_replace(&swarm).unwrap().contains("Swarm service"));
    assert_eq!(cannot_replace(&fixture()), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The image under the tag is older code than the running server (0.42.18 built from a stale branch, schema
/// 41, over a schema-42 database): the helper refuses before anything is stopped.
#[tokio::test]
async fn an_image_that_would_refuse_the_database_changes_nothing() {
    let dir = sock_dir("schema");
    let (d, m) = serve_mock(&dir).await;
    let pre = Preflight { schema: Some(encore_db::SCHEMA_VERSION + 1), version: None };
    let e = run_helper(&d, "encore", NEW, &pre, fast(), &mut |_| {}).await.unwrap_err().to_string();
    assert!(e.starts_with("schema: ") && e.contains("it was written by a newer Encore") && e.contains("Nothing was changed"), "{e}");
    assert!(m.calls().is_empty(), "nothing was even asked: {:?}", m.calls());
    // the same schema, or an older one (the new server migrates it), goes ahead
    assert_eq!(schema_refusal(Some(42), 42, NEW), None);
    assert_eq!(schema_refusal(Some(41), 42, NEW), None);
    assert_eq!(schema_refusal(None, 42, NEW), None, "a server before --schema");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tag names another version than the image holds (a wrong image pushed under it): nothing is changed.
#[tokio::test]
async fn an_image_with_another_version_than_its_tag_changes_nothing() {
    let dir = sock_dir("version");
    let (d, m) = serve_mock(&dir).await;
    let pre = Preflight { schema: None, version: Some("0.27.0".into()) };
    let e = run_helper(&d, "encore", NEW, &pre, fast(), &mut |_| {}).await.unwrap_err().to_string();
    assert!(e.starts_with("inspect: ghcr.io/krifmaz/encore:0.28.0 holds Encore 0.27.0, not 0.28.0"), "{e}");
    assert!(m.calls().is_empty(), "{:?}", m.calls());
    assert_eq!(version_refusal(NEW, Some("0.28.0")), None);
    assert_eq!(version_refusal("ghcr.io/krifmaz/encore:latest", Some("0.28.0")), None, "latest is no version");
    assert_eq!(version_refusal(NEW, None), None, "a build without a release number");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The new server began migrating the database and then stopped: the old Encore is never started on it. The
/// new container is started again; when it keeps running the update goes through, otherwise it stays and
/// the old one is kept stopped for a manual way back.
#[tokio::test]
async fn after_a_migration_the_update_goes_forward_not_back() {
    for recovers in [false, true] {
        let dir = sock_dir(if recovers { "mig-ok" } else { "mig-bad" });
        let (d, m) = serve_mock(&dir).await;
        m.fail.lock().unwrap().extend(["exits", "migrates"]);
        if recovers {
            m.fail.lock().unwrap().insert("recovers");
        }
        let mut lines = vec![];
        let res = run_helper(&d, "encore", NEW, &Preflight::default(), fast(), &mut |l| lines.push(l.to_string())).await;
        let calls = short(&m.calls());
        assert!(!calls.iter().any(|c| c == "POST /containers/OLD/start"), "the old Encore never runs on the migrated database: {calls:#?}");
        assert!(!calls.iter().any(|c| c.starts_with("POST /containers/OLD/rename?name=encore")), "the old one keeps its -old- name: {calls:#?}");
        assert!(!calls.iter().any(|c| c.starts_with("DELETE /containers/NEW")), "the new container stays: {calls:#?}");
        assert!(lines.iter().any(|l| l.contains("had begun migrating the database from schema 42 to 43")), "{lines:?}");
        if recovers {
            assert_eq!(res.unwrap(), NEW_ID);
            assert!(lines.iter().any(|l| l == "verify: the new container keeps running now"), "{lines:?}");
            assert_eq!(lines.last().map(String::as_str), Some("cleanup: done"));
        } else {
            let e = res.unwrap_err().to_string();
            assert!(e.starts_with("verify: the new container stopped (status exited, exit code 3)"), "{e}");
            assert!(e.contains("the old Encore was not started again") && e.contains("restore the backup taken before the migration (/config/backups)"), "{e}");
            assert_eq!(*m.new_starts.lock().unwrap(), 1 + RESTARTS_AFTER_MIGRATION, "started again {RESTARTS_AFTER_MIGRATION} times");
            assert!(!calls.iter().any(|c| c.starts_with("DELETE /containers/OLD")), "the old container is kept: {calls:#?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn exit_codes_say_what_they_mean() {
    assert!(exit_meaning(i64::from(encore_db::EXIT_NEWER_DB)).unwrap().contains("refused the database"));
    assert!(exit_meaning(137).unwrap().contains("killed"));
    assert_eq!(exit_meaning(1), None);
}
