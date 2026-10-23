//! "Update now", the container part: `encore self-update-helper --target <id> --image <image>` runs in a
//! short-lived container of the NEW image that the server starts through the Docker socket (update.rs). It
//! replaces the server's container the way Watchtower does: inspect it, stop it, rename it out of the way,
//! create a container of the new image with the same settings under the old name, start it, and remove the
//! old one once the new one keeps running (its volumes stay). When creating or starting the new one fails,
//! the old one gets its name back and is started again. Every step is printed (`docker logs` of the helper
//! until Docker removes it).
//!
//! The pure parts – which settings carry over (`plan_create_body`), how an image reference splits
//! (`parse_image`) and the helper's own container (`plan_helper_body`) – are tested on a captured inspect
//! answer (fixtures/docker-inspect.json, Docker 27).
use crate::docker::{self, Docker};
use serde_json::{json, Map, Value};
use std::time::Duration;

/// The only image Encore updates itself to.
pub const IMAGE: &str = "ghcr.io/krifmaz/encore";
/// The program inside the image (Dockerfile: COPY … /usr/local/bin/encore).
const BINARY: &str = "/usr/local/bin/encore";
/// The label on the helper's container, so it can be told apart in `docker ps -a`.
pub const HELPER_LABEL: &str = "org.encore.role";

/// `ghcr.io/krifmaz/encore:0.28.0` → (`ghcr.io/krifmaz/encore`, `0.28.0`); no tag → `latest`; a registry port
/// is not a tag (`localhost:5000/encore` → tag `latest`); `repo@sha256:…` → (repo, `sha256:…`).
pub fn parse_image(reference: &str) -> (String, String) {
    let r = reference.trim();
    if let Some((repo, digest)) = r.split_once('@') {
        return (repo.to_string(), digest.to_string());
    }
    match r.rfind(':') {
        Some(i) if !r[i..].contains('/') => (r[..i].to_string(), r[i + 1..].to_string()),
        _ => (r.to_string(), "latest".to_string()),
    }
}

/// Whether a JSON value holds something (not null, not an empty string, array or object).
fn nonempty(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

/// HostConfig fields that carry over to the new container (the rest is Docker's bookkeeping or its defaults).
const HOST_FIELDS: &[&str] = &[
    "Binds", "Mounts", "PortBindings", "PublishAllPorts", "RestartPolicy", "NetworkMode", "Devices", "DeviceRequests", "DeviceCgroupRules",
    "CapAdd", "CapDrop", "ExtraHosts", "LogConfig", "Dns", "DnsOptions", "DnsSearch", "Privileged", "SecurityOpt", "ShmSize", "Tmpfs", "Sysctls",
    "Ulimits", "GroupAdd", "Init", "ReadonlyRootfs", "VolumesFrom", "Links", "IpcMode", "PidMode", "UTSMode", "UsernsMode", "Runtime",
    "OomScoreAdj", "OomKillDisable", "PidsLimit", "Memory", "MemoryReservation", "MemorySwap", "MemorySwappiness", "NanoCpus", "CpuShares",
    "CpuPeriod", "CpuQuota", "CpusetCpus", "CpusetMems", "BlkioWeight", "CgroupParent",
];

/// Config fields that carry over as they are (Env, Cmd, Entrypoint, Labels … see below for the rest).
const CONFIG_FIELDS: &[&str] = &["User", "Env", "Cmd", "Entrypoint", "Labels", "WorkingDir", "ExposedPorts", "Healthcheck", "StopSignal", "StopTimeout",
    "Tty", "OpenStdin", "StdinOnce"];

/// The create body for the new container: the old one's settings with the new image. Hostname only when the
/// owner set one (Docker's default is the first 12 characters of the id); the same networks with their
/// aliases (not the old id's), a fixed IP only on user-defined networks; anonymous volumes (an image VOLUME
/// such as /config that nothing was mounted on) are mounted again by name so their data comes along.
pub fn plan_create_body(inspect: &Value, image: &str) -> Value {
    let cfg = &inspect["Config"];
    let host = &inspect["HostConfig"];
    let short_id: String = inspect["Id"].as_str().unwrap_or("").chars().take(12).collect();
    let mode = host["NetworkMode"].as_str().unwrap_or("");
    let shares_net = mode.starts_with("container:");
    let host_net = mode == "host";
    let mut body = Map::new();
    for k in CONFIG_FIELDS {
        if nonempty(&cfg[*k]) {
            body.insert(k.to_string(), cfg[*k].clone());
        }
    }
    body.insert("Image".into(), json!(image));
    let hostname = cfg["Hostname"].as_str().unwrap_or("");
    if !hostname.is_empty() && hostname != short_id && !shares_net && !host_net {
        body.insert("Hostname".into(), json!(hostname));
        if let Some(d) = cfg["Domainname"].as_str().filter(|d| !d.is_empty()) {
            body.insert("Domainname".into(), json!(d));
        }
    }

    let mut hc = Map::new();
    for k in HOST_FIELDS {
        if nonempty(&host[*k]) {
            hc.insert(k.to_string(), host[*k].clone());
        }
    }
    // "/db:/encore/db" (how inspect shows a link) → "db:db" (how create takes it)
    if let Some(links) = hc.get("Links").and_then(|l| l.as_array()).cloned() {
        let fixed: Vec<Value> = links.iter().filter_map(|l| l.as_str()).filter_map(|l| {
            let (name, alias) = l.split_once(':')?;
            Some(json!(format!("{}:{}", name.trim_start_matches('/'), alias.rsplit('/').next().unwrap_or(alias))))
        }).collect();
        hc.insert("Links".into(), json!(fixed));
    }
    // anonymous volumes: mounted again by their name
    let mut covered: Vec<String> = host["Binds"].as_array().into_iter().flatten().filter_map(|b| b.as_str())
        .filter_map(|b| b.split(':').nth(1).map(String::from)).collect();
    covered.extend(host["Mounts"].as_array().into_iter().flatten().filter_map(|m| m["Target"].as_str().map(String::from)));
    let mut mounts: Vec<Value> = host["Mounts"].as_array().cloned().unwrap_or_default();
    for m in inspect["Mounts"].as_array().into_iter().flatten() {
        let (Some(name), Some(dest)) = (m["Name"].as_str(), m["Destination"].as_str()) else { continue };
        if m["Type"] == "volume" && !covered.iter().any(|c| c == dest) {
            mounts.push(json!({ "Type": "volume", "Source": name, "Target": dest, "ReadOnly": m["RW"] == false }));
            covered.push(dest.to_string());
        }
    }
    if !mounts.is_empty() {
        hc.insert("Mounts".into(), json!(mounts));
    }
    if shares_net {
        // another container's network: Docker refuses these next to it
        for k in ["Links", "Dns", "DnsOptions", "DnsSearch", "ExtraHosts", "PortBindings", "PublishAllPorts"] {
            hc.remove(k);
        }
        body.remove("ExposedPorts");
    }
    if host_net {
        hc.remove("Links");
    }
    body.insert("HostConfig".into(), Value::Object(hc));

    if !(shares_net || host_net || mode == "none") {
        let mut endpoints = Map::new();
        for (net, ep) in inspect["NetworkSettings"]["Networks"].as_object().into_iter().flatten() {
            let mut e = Map::new();
            // the default bridge takes neither aliases nor a fixed address
            if net != "bridge" {
                let mut aliases: Vec<String> = vec![];
                for a in ep["Aliases"].as_array().into_iter().flatten().filter_map(|a| a.as_str()) {
                    if a != short_id && !aliases.iter().any(|x| x == a) {
                        aliases.push(a.to_string());
                    }
                }
                if !aliases.is_empty() {
                    e.insert("Aliases".into(), json!(aliases));
                }
                let ipam = &ep["IPAMConfig"];
                if ipam.as_object().is_some_and(|o| o.values().any(nonempty)) {
                    e.insert("IPAMConfig".into(), ipam.clone());
                }
                if nonempty(&ep["Links"]) {
                    e.insert("Links".into(), ep["Links"].clone());
                }
                if nonempty(&ep["DriverOpts"]) {
                    e.insert("DriverOpts".into(), ep["DriverOpts"].clone());
                }
            }
            endpoints.insert(net.clone(), Value::Object(e));
        }
        if !endpoints.is_empty() {
            body.insert("NetworkingConfig".into(), json!({ "EndpointsConfig": endpoints }));
        }
    }
    Value::Object(body)
}

/// What the old container only had from its old image goes, so the new image's own values apply: Env lines,
/// labels and exposed ports the old image set, and Entrypoint/Cmd, Healthcheck, User, WorkingDir and
/// StopSignal when they equal the old image's. (The old build id in ENCORE_BUILD or the image's version
/// label would otherwise outlive the update.) `old_image` is `GET /images/<old image id>/json`.
pub fn without_image_defaults(mut body: Value, old_image: &Value) -> Value {
    let img = &old_image["Config"];
    let Some(b) = body.as_object_mut() else { return body };
    if let (Some(env), Some(img_env)) = (b.get("Env").and_then(|e| e.as_array()).cloned(), img["Env"].as_array()) {
        let kept: Vec<Value> = env.into_iter().filter(|e| !img_env.contains(e)).collect();
        if kept.is_empty() { b.remove("Env") } else { b.insert("Env".into(), json!(kept)) };
    }
    if let (Some(labels), Some(img_labels)) = (b.get("Labels").and_then(|l| l.as_object()).cloned(), img["Labels"].as_object()) {
        let kept: Map<String, Value> = labels.into_iter().filter(|(k, v)| img_labels.get(k) != Some(v)).collect();
        if kept.is_empty() { b.remove("Labels") } else { b.insert("Labels".into(), Value::Object(kept)) };
    }
    if let (Some(ports), Some(img_ports)) = (b.get("ExposedPorts").and_then(|p| p.as_object()).cloned(), img["ExposedPorts"].as_object()) {
        let kept: Map<String, Value> = ports.into_iter().filter(|(k, _)| !img_ports.contains_key(k)).collect();
        if kept.is_empty() { b.remove("ExposedPorts") } else { b.insert("ExposedPorts".into(), Value::Object(kept)) };
    }
    let same = |b: &Map<String, Value>, k: &str| b.get(k).is_some_and(|v| *v == img[k]);
    if same(b, "Entrypoint") || (!b.contains_key("Entrypoint") && !nonempty(&img["Entrypoint"])) {
        b.remove("Entrypoint");
        if same(b, "Cmd") {
            b.remove("Cmd");
        }
    }
    for k in ["Healthcheck", "User", "WorkingDir", "StopSignal"] {
        if same(b, k) {
            b.remove(k);
        }
    }
    body
}

/// Containers this swap must not touch: one Docker removes as soon as it stops (`--rm`: the old one would be
/// gone before the new one exists, with nothing to roll back to) and a Swarm task (the service owns it).
pub fn cannot_replace(inspect: &Value) -> Option<&'static str> {
    if inspect["HostConfig"]["AutoRemove"] == true {
        return Some("This container is removed when it stops (started with --rm), so it can't be replaced");
    }
    if inspect["Config"]["Labels"]["com.docker.swarm.service.id"].is_string() {
        return Some("This container belongs to a Docker Swarm service: update the service instead");
    }
    None
}

/// Where this container has the Docker socket from (the host side of the bind), for the helper's own bind.
pub fn socket_source(inspect: &Value) -> Option<String> {
    inspect["Mounts"].as_array()?.iter()
        .find(|m| matches!(m["Destination"].as_str(), Some("/var/run/docker.sock" | "/run/docker.sock")))
        .and_then(|m| m["Source"].as_str()).map(String::from)
}

/// The helper's container: the new image running `encore self-update-helper`, as root (it only talks to the
/// socket), with the same socket bind, no network, no ports, removed by Docker when it ends.
pub fn plan_helper_body(self_inspect: &Value, image: &str) -> Option<Value> {
    let id = self_inspect["Id"].as_str().filter(|i| !i.is_empty())?;
    let sock = socket_source(self_inspect)?;
    Some(json!({
        "Image": image,
        "Entrypoint": [BINARY],
        "Cmd": ["self-update-helper", "--target", id, "--image", image],
        "User": "0:0",
        "Labels": { HELPER_LABEL: "self-update-helper" },
        "HostConfig": { "Binds": [format!("{sock}:{}", docker::SOCKET)], "AutoRemove": true, "NetworkMode": "none" },
    }))
}

/// How long the helper waits on Docker.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// the new container must keep running this long
    pub settle: Duration,
    /// between looks at it
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { settle: Duration::from_secs(20), poll: Duration::from_secs(1) }
    }
}

/// Whether an HTTP status counts as success (2xx, or 304 Not Modified).
fn ok(code: u16) -> bool {
    (200..300).contains(&code) || code == 304
}

/// One call that must succeed (2xx, or 304: already in that state).
async fn step(d: &Docker, what: &str, method: &str, path: &str, body: Option<&Value>) -> anyhow::Result<Value> {
    let (code, v) = d.call(method, path, body).await.map_err(|e| anyhow::anyhow!("{what}: {e}"))?;
    if !ok(code) {
        anyhow::bail!("{what}: Docker answered {code}: {}", docker::message(&v));
    }
    Ok(v)
}

/// The old container gets its name back and runs again.
async fn roll_back(d: &Docker, log: &mut dyn FnMut(&str), id: &str, name: &str) {
    log(&format!("rolling back: {name} gets its name back and starts again"));
    if let Err(e) = step(d, "rename back", "POST", &format!("/containers/{id}/rename?name={}", urlencoding::encode(name)), None).await {
        log(&e.to_string());
    }
    match step(d, "start the old container", "POST", &format!("/containers/{id}/start"), None).await {
        Ok(_) => log(&format!("{name} runs again on the old image")),
        Err(e) => log(&format!("{e} – start it by hand: docker start {id}")),
    }
}

/// The new container goes (it never ran properly), then the old one comes back.
async fn give_up(d: &Docker, log: &mut dyn FnMut(&str), e: anyhow::Error, new_id: Option<&str>, id: &str, name: &str) -> anyhow::Error {
    log(&e.to_string());
    if let Some(n) = new_id.filter(|n| !n.is_empty()) {
        if let Err(e) = step(d, "remove the new container", "DELETE", &format!("/containers/{n}?force=true"), None).await {
            log(&e.to_string());
        }
    }
    roll_back(d, log, id, name).await;
    e
}

/// Replace container `target` with one of `image`. Ok(new container id).
pub async fn run_helper(d: &Docker, target: &str, image: &str, t: Timing, log: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
    log(&format!("updating container {target} to {image}"));
    let inspect = d.inspect(target).await?.ok_or_else(|| anyhow::anyhow!("no container {target}"))?;
    let id = inspect["Id"].as_str().unwrap_or(target).to_string();
    let name = inspect["Name"].as_str().unwrap_or("").trim_start_matches('/').to_string();
    if name.is_empty() {
        anyhow::bail!("container {target} has no name");
    }
    if let Some(why) = cannot_replace(&inspect) {
        anyhow::bail!("{why}: nothing was changed");
    }
    let mut body = plan_create_body(&inspect, image);
    let mut old_image = Value::Null;
    if let Some(old) = inspect["Image"].as_str().filter(|i| !i.is_empty()) {
        match d.get(&format!("/images/{old}/json")).await {
            Ok((200, img)) => {
                body = without_image_defaults(body, &img);
                old_image = img;
            }
            _ => log("the old image is gone: its settings are copied as the container had them"),
        }
    }
    // image references go into the path as they are, like the docker CLI sends them
    let (code, new_image) = d.get(&format!("/images/{image}/json")).await?;
    if code != 200 {
        anyhow::bail!("{image} is not on this host (Docker answered {code}): nothing was changed");
    }

    log(&format!("stopping {name} (up to 30 s)"));
    step(d, "stop", "POST", &format!("/containers/{id}/stop?t=30"), None).await?;
    let old_name = format!("{name}-old-{}", encore_db::now() as i64);
    log(&format!("renaming {name} to {old_name}"));
    if let Err(e) = step(d, "rename", "POST", &format!("/containers/{id}/rename?name={}", urlencoding::encode(&old_name)), None).await {
        log(&e.to_string());
        let _ = step(d, "start the old container", "POST", &format!("/containers/{id}/start"), None).await;
        return Err(e);
    }
    log(&format!("creating {name} from {image}"));
    let created = match step(d, "create", "POST", &format!("/containers/create?name={}", urlencoding::encode(&name)), Some(&body)).await {
        Ok(v) => v,
        Err(e) => return Err(give_up(d, log, e, None, &id, &name).await),
    };
    let new_id = created["Id"].as_str().unwrap_or("").to_string();
    for w in created["Warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str()) {
        log(&format!("Docker: {w}"));
    }
    log(&format!("starting {name} ({})", new_id.chars().take(12).collect::<String>()));
    if let Err(e) = step(d, "start", "POST", &format!("/containers/{new_id}/start"), None).await {
        return Err(give_up(d, log, e, Some(&new_id), &id, &name).await);
    }
    // health is the compose file's job: here the new container only has to keep running
    let deadline = tokio::time::Instant::now() + t.settle;
    loop {
        let state = d.inspect(&new_id).await.ok().flatten().map(|v| v["State"].clone()).unwrap_or(Value::Null);
        if state["Running"] != true || state["Restarting"] == true {
            let e = anyhow::anyhow!("the new container stopped (status {}, exit code {})", state["Status"].as_str().unwrap_or("unknown"), state["ExitCode"]);
            return Err(give_up(d, log, e, Some(&new_id), &id, &name).await);
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(t.poll).await;
    }
    log(&format!("{name} is running on {image}; removing {old_name} (its volumes stay)"));
    if let Err(e) = step(d, "remove the old container", "DELETE", &format!("/containers/{id}?v=false&force=true"), None).await {
        log(&format!("{e} – remove it by hand: docker rm {old_name}"));
    }
    clean_images(d, &old_image, new_image["Id"].as_str().unwrap_or(""), log).await;
    log("done");
    Ok(new_id)
}

/// What goes after a swap to free the disk: the old image's tags of Encore's image (untagging lets Docker drop
/// the image once nothing uses it), then the image itself by id. Nothing when the old image is the new one or is
/// unknown. Tags of other repositories are left alone: someone else put them there.
pub fn plan_image_cleanup(old_image: &Value, new_id: &str) -> Vec<String> {
    let Some(old_id) = old_image["Id"].as_str().filter(|i| !i.is_empty() && *i != new_id) else { return vec![] };
    let mut out: Vec<String> = old_image["RepoTags"].as_array().into_iter().flatten().filter_map(|t| t.as_str())
        .filter(|t| parse_image(t).0 == IMAGE).map(String::from).collect();
    out.push(old_id.to_string());
    out
}

/// The untagged leftovers of Encore's image (`<none>` after a newer :latest or :hotfix was pulled over them).
pub fn dangling_encore_images(list: &Value) -> Vec<String> {
    list.as_array().into_iter().flatten()
        .filter(|i| i["RepoDigests"].as_array().into_iter().flatten().filter_map(|d| d.as_str()).any(|d| parse_image(d).0 == IMAGE))
        .filter(|i| i["RepoTags"].as_array().into_iter().flatten().filter_map(|t| t.as_str()).all(|t| t == "<none>:<none>"))
        .filter_map(|i| i["Id"].as_str().map(String::from))
        .collect()
}

/// The old image and Encore's untagged leftovers go (never forced: an image a container still uses stays).
/// A failure only logs: the update itself is done.
async fn clean_images(d: &Docker, old_image: &Value, new_id: &str, log: &mut dyn FnMut(&str)) {
    let mut gone = 0;
    for r in plan_image_cleanup(old_image, new_id) {
        match d.delete(&format!("/images/{r}")).await {
            Ok((code, _)) if ok(code) || code == 404 => gone += 1,
            Ok((code, v)) => log(&format!("kept the old image {r} (Docker answered {code}: {})", docker::message(&v))),
            Err(e) => log(&format!("kept the old image {r}: {e}")),
        }
    }
    let filters = urlencoding::encode(r#"{"dangling":["true"]}"#).into_owned();
    if let Ok((200, list)) = d.get(&format!("/images/json?filters={filters}")).await {
        for id in dangling_encore_images(&list).into_iter().filter(|i| i != new_id) {
            if let Ok((code, _)) = d.delete(&format!("/images/{id}")).await {
                if ok(code) {
                    gone += 1;
                }
            }
        }
    }
    if gone > 0 {
        log("old Encore images removed");
    }
}

/// `encore self-update-helper …`: Some(exit code) when the program was started as the helper, None otherwise
/// (then it is the server).
pub async fn main_if_helper() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("self-update-helper") {
        return None;
    }
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let (Some(target), Some(image)) = (arg("--target"), arg("--image")) else {
        eprintln!("usage: encore self-update-helper --target <container id> --image {IMAGE}:<tag>");
        return Some(2);
    };
    if parse_image(&image).0 != IMAGE {
        eprintln!("encore self-update: only {IMAGE} images, not {image}");
        return Some(2);
    }
    let d = Docker::new(arg("--socket").unwrap_or_else(|| docker::SOCKET.to_string()));
    let mut log = |m: &str| println!("encore self-update: {m}");
    match run_helper(&d, &target, &image, Timing::default(), &mut log).await {
        Ok(_) => Some(0),
        Err(e) => {
            println!("encore self-update: failed: {e}");
            Some(1)
        }
    }
}

#[cfg(test)]
#[path = "selfupdate_tests.rs"]
pub(crate) mod tests;
