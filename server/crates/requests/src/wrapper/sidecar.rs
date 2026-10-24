//! The wrapper's container: built, started, stopped and removed through the Docker Engine API on the socket
//! the owner mounts into Encore's container (the one "Update now" uses), never by Encore's own process.
//!
//! wrapper-v2 sets up a chroot of Android's libraries and mounts `/proc` and `/dev/urandom` inside it, so
//! its container gets what its compose file gives it: the capabilities `SYS_ADMIN`, `SYS_CHROOT` and
//! `SYS_PTRACE`, and no AppArmor profile. Encore's own container keeps none of that.
//!
//! Where it runs, so Encore and gamdl reach it ([`Placement`]):
//! - Encore on a Docker network of its own (compose's `music`): the wrapper joins it, named
//!   `encore-wrapper`; nothing is published on the host;
//! - Encore on Docker's default bridge: the wrapper joins the bridge and is reached at its address there;
//! - Encore on the host's network, or not in a container: its two ports are published on 127.0.0.1 only.
//!
//! The Apple session lives in the volume `encore-wrapper-data`, so a new container (after an update of the
//! image) is still signed in.
use super::stage::Arch;
use encore_system::docker::{self, Docker};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// The container's name.
pub const CONTAINER: &str = "encore-wrapper";
/// The volume with the Apple session (wrapper-v2's `WRAPPER_BASE_DIR`).
pub const VOLUME: &str = "encore-wrapper-data";
/// Where the session lives inside the container: the Android app's files folder inside the chroot.
pub const SESSION_DIR: &str = "/app/rootfs/data/data/com.apple.android.music/files";
/// The label Encore puts on what it makes, so it never touches a container of the same name it did not make.
pub const LABEL: &str = "app.encore.managed";
/// The control API's port inside the container.
pub const HTTP_PORT: u16 = 80;
/// The decrypt socket's port, inside the container and published.
pub const DECRYPT_PORT: u16 = 10020;
/// The control API's port on the host, when the ports are published (127.0.0.1 only).
pub const HOST_HTTP_PORT: u16 = 30080;
/// The longest an image build may take: the Android NDK is a gigabyte, and the daemon compiles from scratch.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(3 * 3600);

/// Where Encore reaches the wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    /// the control API (gamdl's "Wrapper URL")
    pub url: String,
    /// the decrypt socket, `host:port` (gamdl's "Wrapper decrypt address")
    pub decrypt: String,
}

/// Where the wrapper's container runs, from Encore's own (see the module's notes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Placement {
    /// on this Docker network of Encore's, by name
    Network(String),
    /// on Docker's default bridge, by address
    Bridge,
    /// ports published on the host's 127.0.0.1
    Host,
}

/// Where the wrapper goes, given Encore's container as Docker inspects it (None: not in a container).
pub fn placement_of(this: Option<&Value>) -> Placement {
    let Some(c) = this else { return Placement::Host };
    let mode = c["HostConfig"]["NetworkMode"].as_str().unwrap_or("");
    if mode == "host" || mode.starts_with("container:") {
        return Placement::Host;
    }
    let mut nets: Vec<&String> = c["NetworkSettings"]["Networks"].as_object().map(|o| o.keys().collect()).unwrap_or_default();
    nets.sort();
    // a network of the owner's (a name Docker resolves) beats the default bridge
    if let Some(n) = nets.iter().find(|n| !matches!(n.as_str(), "bridge" | "host" | "none")) {
        return Placement::Network((*n).clone());
    }
    if nets.iter().any(|n| n.as_str() == "bridge") { Placement::Bridge } else { Placement::Host }
}

/// The body of `POST /containers/create` for `image` at `placement`.
pub fn create_body(image: &str, placement: &Placement) -> Value {
    let mut host = json!({
        "CapAdd": ["SYS_ADMIN", "SYS_CHROOT", "SYS_PTRACE"],
        "SecurityOpt": ["apparmor=unconfined"],
        "RestartPolicy": { "Name": "unless-stopped" },
        "Mounts": [{ "Type": "volume", "Source": VOLUME, "Target": SESSION_DIR }],
    });
    let mut body = json!({
        "Image": image,
        "Env": [
            "WRAPPER_HOST=0.0.0.0", format!("WRAPPER_PORT={HTTP_PORT}"),
            "WRAPPER_DECRYPT_HOST=0.0.0.0", format!("WRAPPER_DECRYPT_PORT={DECRYPT_PORT}"),
        ],
        "Labels": { LABEL: "wrapper" },
        "ExposedPorts": { format!("{HTTP_PORT}/tcp"): {}, format!("{DECRYPT_PORT}/tcp"): {} },
    });
    match placement {
        Placement::Network(n) => {
            host["NetworkMode"] = json!(n);
            body["NetworkingConfig"] = json!({ "EndpointsConfig": { n.as_str(): { "Aliases": [CONTAINER] } } });
        }
        Placement::Bridge => host["NetworkMode"] = json!("bridge"),
        Placement::Host => {
            // published on the loopback only: the wrapper decrypts for whoever reaches it
            host["PortBindings"] = json!({
                format!("{HTTP_PORT}/tcp"): [{ "HostIp": "127.0.0.1", "HostPort": HOST_HTTP_PORT.to_string() }],
                format!("{DECRYPT_PORT}/tcp"): [{ "HostIp": "127.0.0.1", "HostPort": DECRYPT_PORT.to_string() }],
            });
        }
    }
    body["HostConfig"] = host;
    body
}

/// Where Encore reaches the running wrapper (its container as Docker inspects it) at `placement`. None on
/// the bridge before Docker gave it an address.
pub fn address_of(placement: &Placement, wrapper: &Value) -> Option<Address> {
    let at = |host: &str, http: u16| Address {
        url: if http == 80 { format!("http://{host}") } else { format!("http://{host}:{http}") },
        decrypt: format!("{host}:{DECRYPT_PORT}"),
    };
    match placement {
        Placement::Network(_) => Some(at(CONTAINER, HTTP_PORT)),
        Placement::Bridge => {
            let ip = wrapper["NetworkSettings"]["Networks"]["bridge"]["IPAddress"].as_str().or(wrapper["NetworkSettings"]["IPAddress"].as_str()).unwrap_or("");
            (!ip.is_empty()).then(|| at(ip, HTTP_PORT))
        }
        Placement::Host => Some(at("127.0.0.1", HOST_HTTP_PORT)),
    }
}

/// A container's state in one word: running, restarting, exited, created, paused, dead, or missing.
pub fn state_word(c: Option<&Value>) -> String {
    match c {
        None => "missing".into(),
        Some(c) => c["State"]["Status"].as_str().unwrap_or("unknown").to_string(),
    }
}

/// Whether Encore made this container (its label).
pub fn ours(c: &Value) -> bool {
    c["Config"]["Labels"][LABEL].as_str() == Some("wrapper")
}

/// The Docker socket, answering: else why not, in words the card shows.
pub async fn probe(sock: &Path) -> Result<Docker, String> {
    use std::os::unix::fs::FileTypeExt;
    match std::fs::metadata(sock) {
        Ok(m) if m.file_type().is_socket() => {}
        Ok(_) => return Err(format!("{} is not the Docker socket", sock.display())),
        Err(_) => return Err("The Docker socket is not mounted: add /var/run/docker.sock:/var/run/docker.sock to Encore's volumes, or run the wrapper yourself and choose \"A wrapper I run\"".into()),
    }
    let d = Docker::new(sock);
    match d.get("/_ping").await {
        Ok((200, _)) => Ok(d),
        Ok((code, v)) => Err(format!("Docker refused the request: it answered {code}: {}", docker::message(&v))),
        Err(e) if e.to_string().contains("ermission denied") => {
            // SAFETY: a plain getter without arguments
            let gid = unsafe { libc::getgid() };
            Err(format!("Encore may not use the Docker socket (it runs with group {gid}): set PGID to the socket's group, or PUID=0"))
        }
        Err(e) => Err(format!("The Docker socket does not answer: {e}")),
    }
}

/// The wrapper's container as Docker inspects it, if there is one.
pub async fn container(d: &Docker) -> Result<Option<Value>, String> {
    d.inspect(CONTAINER).await.map_err(|e| e.to_string())
}

/// `POST /build` of the context tar at `context` as `tag` for `arch`, every progress line handed to
/// `on_line`. Docker's own words when it fails.
pub async fn build(d: &Docker, context: &Path, tag: &str, arch: Arch, on_line: impl FnMut(&str) + Send) -> Result<(), String> {
    let args = json!({ "TARGET_ARCH": arch.name(), "BUILD_PLATFORM": "linux/amd64", "RUNTIME_PLATFORM": arch.platform() });
    let labels = json!({ LABEL: "wrapper" });
    // version=1: the builder the Engine API runs by itself (BuildKit would need a session of its own)
    let path = format!(
        "/build?t={}&rm=1&forcerm=1&version=1&buildargs={}&labels={}",
        urlencoding::encode(tag), urlencoding::encode(&args.to_string()), urlencoding::encode(&labels.to_string())
    );
    let mut last_error: Option<String> = None;
    let mut on_line = on_line;
    let status = d.stream_file("POST", &path, context, "application/x-tar", BUILD_TIMEOUT, |line| {
        if let Some(e) = docker::build_error(line) {
            last_error = Some(e);
        }
        on_line(line);
    }).await.map_err(|e| format!("The image build stopped: {e}"))?;
    match (status, last_error) {
        (_, Some(e)) => Err(format!("The image build failed: {e}")),
        (s, None) if s >= 400 => Err(format!("Docker refused the image build (it answered {s})")),
        _ => Ok(()),
    }
}

/// An image name split into what `/images/create` takes: (`repo`, `tag`), the tag `latest` when none is
/// given (a registry's port is not a tag: `host:5000/x` is `host:5000/x`, `latest`).
pub fn split_image(image: &str) -> (String, String) {
    match image.rsplit_once(':') {
        Some((repo, tag)) if !tag.contains('/') && !repo.is_empty() => (repo.to_string(), tag.to_string()),
        _ => (image.to_string(), "latest".to_string()),
    }
}

/// Run `image` as the wrapper's container: a container of another image (an older build) is replaced, one
/// that runs this image is started when it is not running. Gives back where Encore reaches it.
pub async fn run(d: &Docker, image: &str) -> Result<Address, String> {
    let this = d.this_container().await;
    let placement = placement_of(this.as_ref());
    if let Some(c) = container(d).await? {
        if !ours(&c) {
            return Err(format!("Docker already has a container named {CONTAINER} that Encore did not make: rename or remove it first"));
        }
        let same = c["Config"]["Image"].as_str() == Some(image);
        if !same || placement != placement_from_wrapper(&c, &placement) {
            remove(d).await?;
        }
    }
    if container(d).await?.is_none() {
        let (code, v) = d.post(&format!("/containers/create?name={CONTAINER}"), Some(&create_body(image, &placement))).await.map_err(|e| e.to_string())?;
        if code >= 400 {
            return Err(format!("Docker could not create the wrapper's container: {}", docker::message(&v)));
        }
    }
    let (code, v) = d.post(&format!("/containers/{CONTAINER}/start"), None).await.map_err(|e| e.to_string())?;
    // 304: it already runs
    if code >= 400 {
        return Err(format!("Docker could not start the wrapper: {}", docker::message(&v)));
    }
    // a container that ends at once (no privileges, a broken image) says so in its state
    let mut last = Value::Null;
    for _ in 0..20 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if let Some(c) = container(d).await? {
            last = c;
            if last["State"]["Running"] == true {
                if let Some(a) = address_of(&placement, &last) {
                    return Ok(a);
                }
            } else if matches!(last["State"]["Status"].as_str(), Some("exited" | "dead")) {
                let tail = d.logs(CONTAINER, 5).await.unwrap_or_default();
                return Err(format!("The wrapper stopped right after it started (exit code {}): {}", last["State"]["ExitCode"], tail.trim()));
            }
        }
    }
    address_of(&placement, &last).ok_or_else(|| "The wrapper started, but Docker gave it no address yet: press Start again in a moment".to_string())
}

/// The placement an existing container of ours was made for (so a move of Encore to another network makes a
/// new one): its network, else the published ports. `wanted` when it can't be told.
fn placement_from_wrapper(c: &Value, wanted: &Placement) -> Placement {
    let mode = c["HostConfig"]["NetworkMode"].as_str().unwrap_or("");
    if c["HostConfig"]["PortBindings"].as_object().is_some_and(|o| !o.is_empty()) {
        Placement::Host
    } else if mode == "bridge" || mode == "default" {
        Placement::Bridge
    } else if !mode.is_empty() {
        Placement::Network(mode.to_string())
    } else {
        wanted.clone()
    }
}

/// Stop the wrapper's container (it stays, to start again). Fine when it is not there or not running.
pub async fn stop(d: &Docker) -> Result<(), String> {
    match container(d).await? {
        Some(c) if !ours(&c) => Err(format!("The container named {CONTAINER} is not Encore's: Encore leaves it alone")),
        Some(_) => {
            let (code, v) = d.post(&format!("/containers/{CONTAINER}/stop?t=10"), None).await.map_err(|e| e.to_string())?;
            if code >= 400 && code != 404 { Err(format!("Docker could not stop the wrapper: {}", docker::message(&v))) } else { Ok(()) }
        }
        None => Ok(()),
    }
}

/// Remove the wrapper's container (the session's volume and the image stay). Fine when it is not there.
pub async fn remove(d: &Docker) -> Result<(), String> {
    match container(d).await? {
        Some(c) if !ours(&c) => Err(format!("The container named {CONTAINER} is not Encore's: Encore leaves it alone")),
        Some(_) => {
            let (code, v) = d.delete(&format!("/containers/{CONTAINER}?force=1")).await.map_err(|e| e.to_string())?;
            if code >= 400 && code != 404 { Err(format!("Docker could not remove the wrapper's container: {}", docker::message(&v))) } else { Ok(()) }
        }
        None => Ok(()),
    }
}

/// Pull `image` from its registry (an image the owner named instead of a build).
pub async fn pull(d: &Docker, image: &str) -> Result<(), String> {
    let (repo, tag) = split_image(image);
    d.pull(&repo, &tag, Duration::from_secs(1800)).await.map_err(|e| format!("Docker could not pull {image}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encore_on(nets: &[&str], mode: &str) -> Value {
        let mut n = serde_json::Map::new();
        for x in nets {
            n.insert(x.to_string(), json!({ "IPAddress": "172.18.0.2" }));
        }
        json!({ "HostConfig": { "NetworkMode": mode }, "NetworkSettings": { "Networks": n } })
    }

    #[test]
    fn the_wrapper_goes_where_encore_reaches_it() {
        assert_eq!(placement_of(None), Placement::Host);
        assert_eq!(placement_of(Some(&encore_on(&["encore_music"], "encore_music"))), Placement::Network("encore_music".into()));
        // the owner's network beats the bridge
        assert_eq!(placement_of(Some(&encore_on(&["bridge", "music"], "bridge"))), Placement::Network("music".into()));
        assert_eq!(placement_of(Some(&encore_on(&["bridge"], "default"))), Placement::Bridge);
        assert_eq!(placement_of(Some(&encore_on(&[], "host"))), Placement::Host);
        assert_eq!(placement_of(Some(&encore_on(&[], "container:vpn"))), Placement::Host);
    }

    #[test]
    fn the_container_has_the_wrappers_privileges_and_publishes_only_on_loopback() {
        let b = create_body("encore-wrapper-v2:abc", &Placement::Network("music".into()));
        assert_eq!(b["Image"], "encore-wrapper-v2:abc");
        assert_eq!(b["HostConfig"]["CapAdd"], json!(["SYS_ADMIN", "SYS_CHROOT", "SYS_PTRACE"]));
        assert_eq!(b["HostConfig"]["SecurityOpt"], json!(["apparmor=unconfined"]));
        assert_eq!(b["HostConfig"]["NetworkMode"], "music");
        assert_eq!(b["NetworkingConfig"]["EndpointsConfig"]["music"]["Aliases"], json!([CONTAINER]));
        assert_eq!(b["HostConfig"]["Mounts"][0]["Source"], VOLUME);
        assert_eq!(b["HostConfig"]["Mounts"][0]["Target"], SESSION_DIR);
        assert_eq!(b["Labels"][LABEL], "wrapper");
        assert!(b["HostConfig"]["PortBindings"].is_null(), "nothing published on a network");
        let h = create_body("x", &Placement::Host);
        assert_eq!(h["HostConfig"]["PortBindings"]["80/tcp"][0]["HostIp"], "127.0.0.1");
        assert_eq!(h["HostConfig"]["PortBindings"]["80/tcp"][0]["HostPort"], "30080");
        assert_eq!(h["HostConfig"]["PortBindings"]["10020/tcp"][0]["HostPort"], "10020");
    }

    #[test]
    fn addresses_follow_the_placement() {
        let on_bridge = json!({ "NetworkSettings": { "Networks": { "bridge": { "IPAddress": "172.17.0.5" } } } });
        assert_eq!(address_of(&Placement::Network("m".into()), &Value::Null), Some(Address { url: "http://encore-wrapper".into(), decrypt: "encore-wrapper:10020".into() }));
        assert_eq!(address_of(&Placement::Bridge, &on_bridge), Some(Address { url: "http://172.17.0.5".into(), decrypt: "172.17.0.5:10020".into() }));
        assert_eq!(address_of(&Placement::Bridge, &Value::Null), None);
        assert_eq!(address_of(&Placement::Host, &Value::Null), Some(Address { url: "http://127.0.0.1:30080".into(), decrypt: "127.0.0.1:10020".into() }));
    }

    #[test]
    fn image_names_split_at_the_tag() {
        assert_eq!(split_image("ghcr.io/me/wrapper-v2:1.0"), ("ghcr.io/me/wrapper-v2".into(), "1.0".into()));
        assert_eq!(split_image("wrapper-v2"), ("wrapper-v2".into(), "latest".into()));
        assert_eq!(split_image("registry:5000/wrapper"), ("registry:5000/wrapper".into(), "latest".into()));
        assert_eq!(split_image("registry:5000/wrapper:v2"), ("registry:5000/wrapper".into(), "v2".into()));
    }
}
