//! "Update now", the container part: `encore self-update-helper --target <id> --image <image>` runs in a
//! short-lived container of the NEW image that the server starts through the Docker socket (update.rs). It
//! replaces the server's container the way Watchtower does: inspect it, stop it, rename it out of the way,
//! create a container of the new image with the same settings under the old name, start it, and remove the
//! old one once the new one keeps running (its volumes stay). When creating or starting the new one fails,
//! or it stops again within the settle time, the old one gets its name back and is started again.
//!
//! Every step is printed as `encore self-update: <step>: <text>`, and a failure as the last line
//! `encore self-update: failed: <step>: <error>` (update_report.rs reads them back). The helper's container is
//! not removed by Docker when it ends: the server that comes back reads its log and removes it. When the new
//! container stops, its own last lines and what its exit code means are printed too, so the reason (such as an
//! older Encore refusing a newer database) is in the record.
//!
//! The pure parts – which settings carry over (`plan_create_body`), how an image reference splits
//! (`parse_image`) and the helper's own container (`plan_helper_body`) – are tested on a captured inspect
//! answer (fixtures/docker-inspect.json, Docker 27).
use crate::docker::{self, Docker};
use crate::update_report::{FAILED, LINE_PREFIX};
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
/// socket), with the same socket bind, no network, no ports. `schema` is the database schema the running
/// server keeps (`encore_db::SCHEMA_VERSION`): a helper whose own Encore knows less refuses before anything is
/// stopped. Docker does not remove it when it ends: its log is the update's record until the server that comes
/// back has read it (update_report.rs `collect`, which removes it).
pub fn plan_helper_body(self_inspect: &Value, image: &str, schema: i64) -> Option<Value> {
    let id = self_inspect["Id"].as_str().filter(|i| !i.is_empty())?;
    let sock = socket_source(self_inspect)?;
    Some(json!({
        "Image": image,
        "Entrypoint": [BINARY],
        "Cmd": ["self-update-helper", "--target", id, "--image", image, "--schema", schema.to_string()],
        "User": "0:0",
        "Labels": { HELPER_LABEL: "self-update-helper" },
        "HostConfig": { "Binds": [format!("{sock}:{}", docker::SOCKET)], "NetworkMode": "none" },
    }))
}

/// Whether this helper's Encore can open a database at `schema` (the running server's). The helper runs the
/// new image, so its `SCHEMA_VERSION` is the new server's: a lower one means the new server would refuse the
/// database and exit at once (`encore_db::EXIT_NEWER_DB`), so the swap is not even started.
pub fn schema_refusal(schema: Option<i64>, known: i64, image: &str) -> Option<String> {
    let found = schema?;
    (found > known).then(|| format!("{image} knows database schema {known}, but this server's database is at schema {found} (it was written by a newer Encore): \
        this image is older code than the one running, so it would refuse the database. Nothing was changed; pick a newer release"))
}

/// Whether the image holds the Encore its tag names: `own` is this helper's version (it runs the new image;
/// None in a build without a release number, which is not checked). `latest` and other names are not
/// versions and are not checked either.
pub fn version_refusal(image: &str, own: Option<&str>) -> Option<String> {
    let own = own?;
    let tag = parse_image(image).1;
    (tag.starts_with(|c: char| c.is_ascii_digit()) && tag != own)
        .then(|| format!("{image} holds Encore {own}, not {tag}: the registry has the wrong image under that tag. Nothing was changed"))
}

/// What the helper checks before it touches anything: the running server's database schema (`--schema`) and
/// the helper's own version (the new image's).
#[derive(Clone, Debug, Default)]
pub struct Preflight {
    /// the running server's database schema; None from a server before `--schema`
    pub schema: Option<i64>,
    /// this helper's version (`ENCORE_VERSION`); None in a build without one
    pub version: Option<String>,
}

/// What a container's exit code says, when Encore knows it.
pub fn exit_meaning(code: i64) -> Option<&'static str> {
    match i32::try_from(code).ok()? {
        encore_db::EXIT_NEWER_DB => Some("the new Encore refused the database because a newer Encore wrote it (the image is older code than the one that was running)"),
        137 => Some("it was killed (out of memory, or stopped from outside)"),
        126 | 127 => Some("its program could not be started (missing or not executable)"),
        _ => None,
    }
}

/// How many of the new container's own last lines a failure prints.
const NEW_LOG_LINES: usize = 20;

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
    log(&format!("rollback: {name} gets its name back and starts again"));
    if let Err(e) = step(d, "rollback: rename back", "POST", &format!("/containers/{id}/rename?name={}", urlencoding::encode(name)), None).await {
        log(&e.to_string());
    }
    match step(d, "rollback: start the old container", "POST", &format!("/containers/{id}/start"), None).await {
        Ok(_) => log(&format!("rollback: {name} runs again on the old image")),
        Err(e) => log(&format!("{e} – start it by hand: docker start {id}")),
    }
}

/// The new container goes (it never ran properly), then the old one comes back. `e` is the failure, already
/// named after its step.
async fn give_up(d: &Docker, log: &mut dyn FnMut(&str), e: anyhow::Error, new_id: Option<&str>, id: &str, name: &str) -> anyhow::Error {
    log(&e.to_string());
    if let Some(n) = new_id.filter(|n| !n.is_empty()) {
        if let Err(e) = step(d, "rollback: remove the new container", "DELETE", &format!("/containers/{n}?force=true"), None).await {
            log(&e.to_string());
        }
    }
    roll_back(d, log, id, name).await;
    e
}

/// The new container stopped within the settle time: its last lines go into the log (they hold the server's
/// own reason, e.g. the newer-database message). Gives back the error, naming the exit code and what it
/// means, and whether the new server had begun migrating the database (`encore_db::migration_in_log`).
async fn stopped_error(d: &Docker, log: &mut dyn FnMut(&str), new_id: &str, state: &Value) -> (anyhow::Error, Option<(i64, i64)>) {
    let mut migrated = None;
    match d.logs(new_id, NEW_LOG_LINES).await {
        Ok(text) => {
            for l in text.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
                log(&format!("verify: new container: {l}"));
            }
            migrated = encore_db::migration_in_log(&text);
        }
        Err(e) => log(&format!("verify: the new container's log could not be read: {e}")),
    }
    let code = state["ExitCode"].as_i64();
    let mut e = format!("verify: the new container stopped (status {}, exit code {})", state["Status"].as_str().unwrap_or("unknown"),
        code.map(|c| c.to_string()).unwrap_or_else(|| "unknown".into()));
    if let Some(m) = code.and_then(exit_meaning) {
        e.push_str(": ");
        e.push_str(m);
    }
    (anyhow::anyhow!(e), migrated)
}

/// How often a new container that stopped after migrating is started again before the helper gives up.
const RESTARTS_AFTER_MIGRATION: usize = 3;

/// The new server stopped after it began migrating the database: rolling back would hand the migrated
/// database to an Encore that may not read it, so the new container stays and is started again (up to
/// [`RESTARTS_AFTER_MIGRATION`] times). The old container stays renamed and stopped, to go back by hand
/// with the backup taken before the migration. Ok when the new one keeps running after all.
async fn keep_new(d: &Docker, log: &mut dyn FnMut(&str), e: anyhow::Error, (from, to): (i64, i64), new_id: &str, old_name: &str, t: Timing) -> anyhow::Result<()> {
    log(&e.to_string());
    log(&format!("verify: the new server had begun migrating the database from schema {from} to {to}: no roll-back, the new container is started again"));
    for n in 1..=RESTARTS_AFTER_MIGRATION {
        log(&format!("verify: starting the new container again ({n} of {RESTARTS_AFTER_MIGRATION})"));
        if let Err(e) = step(d, "verify: start the new container again", "POST", &format!("/containers/{new_id}/start"), None).await {
            log(&e.to_string());
            continue;
        }
        if settles(d, new_id, t).await.is_ok() {
            log("verify: the new container keeps running now");
            return Ok(());
        }
    }
    anyhow::bail!("{e}. The new server had begun migrating the database (schema {from} → {to}), so the old Encore was not started again: it may not read the migrated database. \
        The new container is left in place (its restart policy keeps trying) and the old one is kept, stopped, as {old_name}. To go back: restore the backup taken before the migration (/config/backups), then start {old_name}")
}

/// Whether the container keeps running for the settle time: Err(its state) as soon as it stops.
async fn settles(d: &Docker, id: &str, t: Timing) -> Result<(), Value> {
    let deadline = tokio::time::Instant::now() + t.settle;
    loop {
        let state = d.inspect(id).await.ok().flatten().map(|v| v["State"].clone()).unwrap_or(Value::Null);
        if state["Running"] != true || state["Restarting"] == true {
            return Err(state);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(t.poll).await;
    }
}

/// Replace container `target` with one of `image`, after the checks of `pre`. Ok(new container id); every
/// error starts with the step it failed at.
pub async fn run_helper(d: &Docker, target: &str, image: &str, pre: &Preflight, t: Timing, log: &mut dyn FnMut(&str)) -> anyhow::Result<String> {
    log(&format!("inspect: updating container {target} to {image}"));
    // the helper runs the new image: what it is, is what the new server will be
    if let Some(why) = version_refusal(image, pre.version.as_deref()) {
        anyhow::bail!("inspect: {why}");
    }
    if let Some(why) = schema_refusal(pre.schema, encore_db::SCHEMA_VERSION, image) {
        anyhow::bail!("schema: {why}");
    }
    if let Some(s) = pre.schema {
        log(&format!("schema: the new Encore reads this database (schema {s}, it knows {})", encore_db::SCHEMA_VERSION));
    }
    let inspect = d.inspect(target).await.map_err(|e| anyhow::anyhow!("inspect: {e}"))?.ok_or_else(|| anyhow::anyhow!("inspect: no container {target}"))?;
    let id = inspect["Id"].as_str().unwrap_or(target).to_string();
    let name = inspect["Name"].as_str().unwrap_or("").trim_start_matches('/').to_string();
    if name.is_empty() {
        anyhow::bail!("inspect: container {target} has no name");
    }
    if let Some(why) = cannot_replace(&inspect) {
        anyhow::bail!("inspect: {why}: nothing was changed");
    }
    let mut body = plan_create_body(&inspect, image);
    let mut old_image = Value::Null;
    if let Some(old) = inspect["Image"].as_str().filter(|i| !i.is_empty()) {
        match d.get(&format!("/images/{old}/json")).await {
            Ok((200, img)) => {
                body = without_image_defaults(body, &img);
                old_image = img;
            }
            _ => log("inspect: the old image is gone: its settings are copied as the container had them"),
        }
    }
    // image references go into the path as they are, like the docker CLI sends them
    let (code, new_image) = d.get(&format!("/images/{image}/json")).await.map_err(|e| anyhow::anyhow!("inspect: {e}"))?;
    if code != 200 {
        anyhow::bail!("inspect: {image} is not on this host (Docker answered {code}): nothing was changed");
    }

    log(&format!("stop: stopping {name} (up to 30 s)"));
    step(d, "stop", "POST", &format!("/containers/{id}/stop?t=30"), None).await?;
    let old_name = format!("{name}-old-{}", encore_db::now() as i64);
    log(&format!("rename: renaming {name} to {old_name}"));
    if let Err(e) = step(d, "rename", "POST", &format!("/containers/{id}/rename?name={}", urlencoding::encode(&old_name)), None).await {
        log(&e.to_string());
        match step(d, "rollback: start the old container", "POST", &format!("/containers/{id}/start"), None).await {
            Ok(_) => log(&format!("rollback: {name} runs again on the old image")),
            Err(e2) => log(&format!("{e2} – start it by hand: docker start {id}")),
        }
        return Err(e);
    }
    log(&format!("create: creating {name} from {image}"));
    let created = match step(d, "create", "POST", &format!("/containers/create?name={}", urlencoding::encode(&name)), Some(&body)).await {
        Ok(v) => v,
        Err(e) => return Err(give_up(d, log, e, None, &id, &name).await),
    };
    let new_id = created["Id"].as_str().unwrap_or("").to_string();
    for w in created["Warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str()) {
        log(&format!("create: Docker: {w}"));
    }
    log(&format!("start: starting {name} ({})", new_id.chars().take(12).collect::<String>()));
    if let Err(e) = step(d, "start", "POST", &format!("/containers/{new_id}/start"), None).await {
        return Err(give_up(d, log, e, Some(&new_id), &id, &name).await);
    }
    // health is the compose file's job: here the new container only has to keep running
    log(&format!("verify: the new container must keep running for {} s", t.settle.as_secs()));
    if let Err(state) = settles(d, &new_id, t).await {
        let (e, migrated) = stopped_error(d, log, &new_id, &state).await;
        match migrated {
            // the database may have moved on: forward only
            Some(m) => keep_new(d, log, e, m, &new_id, &old_name, t).await?,
            None => return Err(give_up(d, log, e, Some(&new_id), &id, &name).await),
        }
    }
    log(&format!("cleanup: {name} is running on {image}; removing {old_name} (its volumes stay)"));
    if let Err(e) = step(d, "cleanup: remove the old container", "DELETE", &format!("/containers/{id}?v=false&force=true"), None).await {
        log(&format!("{e} – remove it by hand: docker rm {old_name}"));
    }
    clean_images(d, &old_image, new_image["Id"].as_str().unwrap_or(""), log).await;
    log("cleanup: done");
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
            Ok((code, v)) => log(&format!("cleanup: kept the old image {r} (Docker answered {code}: {})", docker::message(&v))),
            Err(e) => log(&format!("cleanup: kept the old image {r}: {e}")),
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
        log("cleanup: old Encore images removed");
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
        eprintln!("usage: encore self-update-helper --target <container id> --image {IMAGE}:<tag> [--schema <n>]");
        return Some(2);
    };
    if parse_image(&image).0 != IMAGE {
        println!("{LINE_PREFIX}{FAILED}inspect: only {IMAGE} images, not {image}");
        return Some(2);
    }
    // a server before --schema passes none: the check is skipped
    let pre = Preflight { schema: arg("--schema").and_then(|s| s.parse::<i64>().ok()), version: option_env!("ENCORE_VERSION").map(String::from) };
    let d = Docker::new(arg("--socket").unwrap_or_else(|| docker::SOCKET.to_string()));
    let mut log = |m: &str| println!("{LINE_PREFIX}{m}");
    match run_helper(&d, &target, &image, &pre, Timing::default(), &mut log).await {
        Ok(_) => Some(0),
        Err(e) => {
            println!("{LINE_PREFIX}{FAILED}{e}");
            Some(1)
        }
    }
}

#[cfg(test)]
#[path = "selfupdate_tests.rs"]
pub(crate) mod tests;
