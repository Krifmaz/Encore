//! streamrip's Tidal sign-in, split out of the streamrip plugin's code (sources/streamrip.rs, which re-exports
//! it): Encore runs Tidal's device flow itself and writes the tokens into streamrip's config the way streamrip
//! reads them, one "lane" per OAuth client.
use super::*;
use serde_json::Value;

// ---- Tidal account sign-in (device code) -------------------------------------------------------
// Tidal has no API key for downloads: streamrip signs in with your own account once, through a link you
// open in a browser. Encore runs that device flow itself (streamrip's own copy dies when Tidal answers the
// server with an HTML block page) and writes the tokens into streamrip's config the way streamrip expects:
// one "lane" per OAuth client – the default client (16/44.1 FLAC) in the plain fields and, when
// `hires_client` is on, the hi-res client in the `hires_*` fields.

/// Tidal's OAuth endpoint.
pub(super) const TIDAL_AUTH: &str = "https://auth.tidal.com/v1/oauth2";
/// The scopes the sign-in asks for.
pub(super) const TIDAL_SCOPE: &str = "r_usr+w_usr+w_sub";
/// The default client (16/44.1 FLAC): (id, secret).
pub(super) const TIDAL_DEFAULT_CLIENT: (&str, &str) = ("4N3n6Q1x95LL5K7p", "oKOXfJW371cX6xaZ0PyhgGNBdNLlBZd4AKKYougMjik=");
/// The hi-res client: (id, secret).
pub(super) const TIDAL_HIRES_CLIENT: (&str, &str) = ("fX2JxdmntZWK0ixT", "1Nm5AfDAjxrgJFJbKNWLeAyKGVGmINuXPPLHVXAvxAg=");
/// The browser Tidal's sign-in calls present.
pub(super) const TIDAL_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
/// The [tidal] keys a pasted config may carry, in the order they are written.
pub(super) const TIDAL_TOKEN_KEYS: [&str; 10] = ["user_id", "country_code", "access_token", "refresh_token", "token_expiry", "token_client_id",
    "hires_access_token", "hires_refresh_token", "hires_token_expiry", "hires_token_client_id"];
/// The keys of the hi-res lane.
pub(super) const TIDAL_HIRES_KEYS: [&str; 4] = ["hires_access_token", "hires_refresh_token", "hires_token_expiry", "hires_token_client_id"];
/// The message while the first lane waits for approval.
pub(super) const TIDAL_STEP1_MSG: &str = "Step 1 of 2: open the link, approve in your Tidal account, then come back here.";
/// The message while the second (hi-res) lane waits for approval.
pub(super) const TIDAL_STEP2_MSG: &str = "Step 2 of 2: open the new link, approve in your Tidal account, then come back here.";
/// The message when only one lane is signed in.
pub(super) const TIDAL_ONE_STEP_MSG: &str = "Open the link, approve in your Tidal account, then come back here.";
/// The message once signed in.
pub(super) const TIDAL_DONE_MSG: &str = "Signed in to Tidal – downloads from Tidal use this account now.";
/// The message when the hi-res lane wasn't finished.
pub(super) const TIDAL_PARTIAL_MSG: &str = "Signed in for 16/44.1 FLAC only – sign in again for hi-res (step 2 was not finished).";

/// Where the Tidal sign-in is.
#[derive(Clone, Debug, Default)]
pub struct TidalLogin {
    /// "starting" | "waiting" | "done" | "failed"
    pub status: String,
    /// the link to approve
    pub url: Option<String>,
    /// what the settings page shows
    pub message: String,
    /// 1-based lane being approved, and how many lanes there are (1 or 2).
    pub step: u8,
    /// how many lanes there are
    pub steps: u8,
}

/// The Tidal sign-in in progress (Settings → streamrip → Sign in to Tidal), kept in the server's state.
/// Clones share it.
#[derive(Clone, Default)]
pub struct TidalLogins(std::sync::Arc<std::sync::Mutex<Option<TidalLogin>>>);

/// The sign-in's state now.
pub(super) fn tidal_state(st: &impl Ctx) -> TidalLogin {
    st.requests().tidal.0.lock().map(|g| g.clone().unwrap_or_default()).unwrap_or_default()
}

/// Change the sign-in's state.
pub(super) fn tidal_set(st: &impl Ctx, f: impl FnOnce(&mut TidalLogin)) {
    if let Ok(mut g) = st.requests().tidal.0.lock() {
        let mut s = g.take().unwrap_or_default();
        f(&mut s);
        *g = Some(s);
    }
}

/// The `[tidal]` section of a TOML text (without the header), or "" when there is none.
pub(super) fn tidal_section(text: &str) -> &str {
    section_body(text, "tidal")
}

/// One TOML scalar as text: quotes stripped, inline comments dropped.
pub(super) fn toml_scalar(raw: &str) -> String {
    let v = raw.trim();
    if let Some(q) = v.strip_prefix('"').or_else(|| v.strip_prefix('\'')) {
        let close = if v.starts_with('"') { '"' } else { '\'' };
        return q.split(close).next().unwrap_or("").replace("\\\"", "\"").replace("\\\\", "\\");
    }
    v.split('#').next().unwrap_or("").trim().to_string()
}

/// `key = value` pairs of a `[tidal]` section. Accepts a whole config.toml or just the section, with or
/// without its `[tidal]` header, quoted or bare values.
pub fn parse_tidal_toml(text: &str) -> std::collections::BTreeMap<String, String> {
    // a whole file (any section header present) is narrowed to [tidal]; a bare key list is taken as is
    let body = if Regex::new(r"(?m)^\s*\[").unwrap().is_match(text) { tidal_section(text).to_string() } else { text.to_string() };
    let line = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$").unwrap();
    let mut out = std::collections::BTreeMap::new();
    for c in line.captures_iter(&body) {
        let key = c[1].to_string();
        if TIDAL_TOKEN_KEYS.contains(&key.as_str()) || matches!(key.as_str(), "hires_client" | "client_id" | "client_secret" | "quality") {
            out.insert(key, toml_scalar(&c[2]));
        }
    }
    out
}

/// What streamrip's config currently says about Tidal.
pub(super) fn tidal_config(st: &impl Ctx) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(config_path(st)).map(|t| parse_tidal_toml(&t)).unwrap_or_default()
}

/// The hi-res client is used (`hires_client`, on unless "false").
pub(super) fn tidal_hires_client(cfg: &std::collections::BTreeMap<String, String>) -> bool {
    cfg.get("hires_client").map(|v| v.trim() != "false").unwrap_or(true)
}

/// The OAuth clients to sign in: the one in the config if set, else streamrip's default and (with
/// `hires_client`) hi-res clients. Each lane is (client_id, client_secret, key prefix).
pub(super) fn tidal_lanes(cfg: &std::collections::BTreeMap<String, String>) -> Vec<(String, String, &'static str)> {
    let (cid, sec) = (cfg.get("client_id").cloned().unwrap_or_default(), cfg.get("client_secret").cloned().unwrap_or_default());
    if !cid.is_empty() && !sec.is_empty() {
        return vec![(cid, sec, "")];
    }
    let mut lanes = vec![(TIDAL_DEFAULT_CLIENT.0.to_string(), TIDAL_DEFAULT_CLIENT.1.to_string(), "")];
    if tidal_hires_client(cfg) {
        lanes.push((TIDAL_HIRES_CLIENT.0.to_string(), TIDAL_HIRES_CLIENT.1.to_string(), "hires_"));
    }
    lanes
}

/// A TOML float with one decimal, the way streamrip stores `token_expiry`.
pub(super) fn toml_float(v: f64) -> String {
    format!("{v:.1}")
}

/// Write `[tidal]` values into streamrip's config (made sound through ensure_config first).
pub(super) async fn tidal_write(st: &impl Ctx, values: &[(String, String)]) -> anyhow::Result<()> {
    let path = ensure_config(st).await?;
    let mut text = read_config(&path)?;
    for (k, v) in values {
        text = set_toml(&text, "tidal", k, v);
    }
    write_config(&path, &text)?;
    Ok(())
}

/// The TOML value for a `[tidal]` key: bare for the float/bool keys, quoted for everything else.
pub(super) fn tidal_toml_value(key: &str, val: &str) -> String {
    if key == "hires_client" {
        return if val == "true" { "true".into() } else { "false".into() };
    }
    if key.ends_with("token_expiry") {
        return match val.trim().parse::<f64>() { Ok(f) => toml_float(f), Err(_) => tstr(val) };
    }
    tstr(val)
}

/// streamrip's config has a Tidal access token (the account is signed in).
pub fn tidal_signed_in(st: &impl Ctx) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && (!tidal_hires_client(&cfg) || has("hires_access_token"))
}

/// Signed in on the default client only: hi-res is wanted but step 2 was never done.
pub(super) fn tidal_partial(st: &impl Ctx) -> bool {
    let cfg = tidal_config(st);
    let has = |k: &str| cfg.get(k).map(|v| v.len() > 10).unwrap_or(false);
    has("access_token") && tidal_hires_client(&cfg) && !has("hires_access_token")
}

/// Current sign-in state for the settings page.
pub fn tidal_login_status(st: &impl Ctx) -> Value {
    let s = tidal_state(st);
    let mut out = serde_json::json!({ "status": s.status, "url": s.url, "message": s.message, "signed_in": tidal_signed_in(st) });
    if s.steps > 0 {
        out["step"] = serde_json::json!(s.step);
        out["steps"] = serde_json::json!(s.steps);
    }
    if tidal_partial(st) && s.status != "waiting" && s.status != "starting" {
        out["partial"] = serde_json::json!(true);
        if s.status != "failed" {
            out["message"] = serde_json::json!(TIDAL_PARTIAL_MSG);
        }
    }
    out
}

/// The message for a Tidal answer that is not JSON (an HTML block page) or a 403/429/5xx.
pub fn tidal_blocked_message(code: u16, body: &str) -> String {
    let mut m = format!("Tidal refused this server's address (HTTP {code}, an HTML page instead of an answer). Tidal blocks many hosting and VPN addresses. Sign in on your own computer instead and paste the tokens below, or give the container a different route to the internet.");
    if let Some(c) = Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(body) {
        let title: String = c[1].split_whitespace().collect::<Vec<_>>().join(" ");
        let title = title.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
        if !title.is_empty() {
            m.push_str(" – ");
            m.extend(title.chars().take(120));
        }
    }
    m
}

/// POST a form to auth.tidal.com with browser-like headers; Ok((status, json)) or Err(plain message).
pub(super) async fn tidal_auth_post(st: &impl Ctx, path: &str, form: &[(&str, &str)], basic: Option<(&str, &str)>) -> Result<(u16, Value), String> {
    let mut req = st.http().post(format!("{TIDAL_AUTH}/{path}"))
        .header("User-Agent", TIDAL_UA).header("Accept", "application/json").header("Accept-Language", "en-US,en;q=0.9").form(form);
    if let Some((u, p)) = basic {
        req = req.basic_auth(u, Some(p));
    }
    let r = req.send().await.map_err(|e| format!("Could not reach Tidal: {e}"))?;
    let code = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    // a JSON object of any status is an answer (pending/invalid-code JSON is handled by the caller);
    // anything else (an HTML block page, an empty body) means the address is refused
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if v.is_object() => Ok((code, v)),
        _ => Err(tidal_blocked_message(code, &text)),
    }
}

/// One lane's device flow: get the link, show it, poll until approved. Returns the token answer.
pub(super) async fn tidal_lane_flow(st: &impl Ctx, cid: &str, sec: &str, step: u8, steps: u8) -> Result<Value, String> {
    let (_, d) = tidal_auth_post(st, "device_authorization", &[("client_id", cid), ("scope", TIDAL_SCOPE)], None).await?;
    let Some(code) = d["deviceCode"].as_str().map(str::to_string) else {
        return Err(format!("Tidal did not hand out a sign-in code: {}", d["error_description"].as_str().or(d["error"].as_str()).unwrap_or("no deviceCode in the answer")));
    };
    let Some(url) = d["verificationUriComplete"].as_str().map(|u| if u.starts_with("http") { u.to_string() } else { format!("https://{u}") }) else {
        return Err("Tidal did not hand out a sign-in link".into());
    };
    let expires = d["expiresIn"].as_f64().unwrap_or(300.0).max(30.0);
    let interval = d["interval"].as_f64().unwrap_or(2.0).max(2.0);
    let message = if steps == 1 { TIDAL_ONE_STEP_MSG } else if step == 1 { TIDAL_STEP1_MSG } else { TIDAL_STEP2_MSG };
    tidal_set(st, |s| { s.status = "waiting".into(); s.url = Some(url.clone()); s.message = message.into(); s.step = step; s.steps = steps; });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs_f64(expires);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs_f64(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            return Err("Timed out – the link is only valid for 5 minutes. Try again.".into());
        }
        let form = [("client_id", cid), ("device_code", code.as_str()), ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("scope", TIDAL_SCOPE)];
        let (http, t) = tidal_auth_post(st, "token", &form, Some((cid, sec))).await?;
        if t["access_token"].is_string() {
            return Ok(t);
        }
        let pending = t["sub_status"].as_i64() == Some(1002) || t["error"].as_str() == Some("authorization_pending") || t["error"].as_str() == Some("slow_down");
        if http == 400 && pending {
            continue;
        }
        let why = t["error_description"].as_str().or(t["error"].as_str()).map(str::to_string).unwrap_or_else(|| format!("HTTP {http}"));
        return Err(format!("Tidal did not approve the sign-in: {why}"));
    }
}

/// Start the sign-in: the device flow runs in the background, lane by lane, and writes the tokens into
/// streamrip's config. Returns once the first link is known (or it failed).
pub async fn tidal_login_start(st: &impl Ctx) -> anyhow::Result<Value> {
    ensure_config(st).await?;
    let cfg = tidal_config(st);
    let lanes = tidal_lanes(&cfg);
    let steps = lanes.len() as u8;
    // the old tokens stay until lane 1 is approved: a refused link (Tidal blocks the address) must not
    // throw away a working, pasted-in sign-in
    tidal_set(st, |s| { *s = TidalLogin { status: "starting".into(), url: None, message: "Asking Tidal for a sign-in link…".into(), step: 1, steps }; });
    let st2 = st.clone();
    tokio::spawn(async move {
        let mut values: Vec<(String, String)> = vec![];
        for (i, (cid, sec, prefix)) in lanes.iter().enumerate() {
            let step = i as u8 + 1;
            match tidal_lane_flow(&st2, cid, sec, step, steps).await {
                Ok(t) => {
                    let expiry = t["expires_in"].as_f64().unwrap_or(604800.0) + encore_core::time::now();
                    if i == 0 {
                        values.push(("user_id".into(), tstr(t["user"]["userId"].to_string().trim_matches('"'))));
                        values.push(("country_code".into(), tstr(t["user"]["countryCode"].as_str().unwrap_or(""))));
                        // the new default lane replaces the whole old sign-in: blank the hi-res lane with it so a
                        // stale hi-res token from another account is never mixed in (lane 2 fills it again)
                        for k in TIDAL_HIRES_KEYS {
                            values.push((k.to_string(), "\"\"".to_string()));
                        }
                    }
                    values.push((format!("{prefix}access_token"), tstr(t["access_token"].as_str().unwrap_or(""))));
                    values.push((format!("{prefix}refresh_token"), tstr(t["refresh_token"].as_str().unwrap_or(""))));
                    values.push((format!("{prefix}token_expiry"), toml_float(expiry)));
                    values.push((format!("{prefix}token_client_id"), tstr(cid)));
                    // the first lane is saved at once, so a stopped step 2 still leaves a working 16/44.1 sign-in
                    if let Err(e) = tidal_write(&st2, &values).await {
                        tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
                        return;
                    }
                    values.clear();
                }
                Err(e) => {
                    tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = e; });
                    return;
                }
            }
        }
        let hires = lanes.iter().any(|l| l.2 == "hires_");
        if let Err(e) = tidal_write(&st2, &[("hires_client".into(), if hires { "true".into() } else { "false".into() })]).await {
            tidal_set(&st2, |s| { s.status = "failed".into(); s.url = None; s.message = format!("Could not write streamrip's config: {e}"); });
            return;
        }
        st2.db().log("info", &format!("streamrip: signed in to Tidal ({} client{})", lanes.len(), if lanes.len() == 1 { "" } else { "s" }));
        tidal_set(&st2, |s| { s.status = "done".into(); s.url = None; s.message = TIDAL_DONE_MSG.into(); s.step = steps; });
    });
    // wait for the link
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let s = tidal_state(st);
        if s.url.is_some() || s.status == "failed" || s.status == "done" {
            return Ok(tidal_login_status(st));
        }
    }
    tidal_set(st, |s| { if s.status == "starting" { s.status = "failed".into(); s.message = "Tidal did not answer within 30 seconds. Try again.".into(); } });
    Ok(tidal_login_status(st))
}

/// The error when a paste has no access_token.
pub const TIDAL_NO_TOKEN_MSG: &str = "No access_token found in the pasted text. Paste the [tidal] section of streamrip's config.toml, including its access_token line.";

/// Does a pasted `[tidal]` section (or whole config.toml) carry a default-lane access_token? Anything
/// else about the paste that goes wrong is a server-side problem, not a bad paste.
pub fn tidal_toml_has_token(toml: &str) -> bool {
    parse_tidal_toml(toml).get("access_token").map(|v| v.len() > 10).unwrap_or(false)
}

/// "Paste tokens instead": a `[tidal]` section (or whole config.toml) from a streamrip run on another
/// computer. Writes the token fields and reports the sign-in state like the status route.
pub async fn tidal_tokens_save(st: &impl Ctx, toml: &str) -> anyhow::Result<Value> {
    let got = parse_tidal_toml(toml);
    if !tidal_toml_has_token(toml) {
        anyhow::bail!("{TIDAL_NO_TOKEN_MSG}");
    }
    let mut values: Vec<(String, String)> = TIDAL_TOKEN_KEYS.iter().filter_map(|k| got.get(*k).map(|v| (k.to_string(), tidal_toml_value(k, v)))).collect();
    // a pasted default-lane token with no client id came from streamrip's default client
    if !got.contains_key("token_client_id") {
        values.push(("token_client_id".into(), tstr(TIDAL_DEFAULT_CLIENT.0)));
    }
    let hires = got.get("hires_access_token").map(|v| v.len() > 10).unwrap_or(false);
    if hires && !got.contains_key("hires_token_client_id") {
        values.push(("hires_token_client_id".into(), tstr(TIDAL_HIRES_CLIENT.0)));
    }
    if let Some(h) = got.get("hires_client") {
        values.push(("hires_client".into(), tidal_toml_value("hires_client", h)));
    } else {
        values.push(("hires_client".into(), if hires { "true".into() } else { "false".into() }));
    }
    tidal_write(st, &values).await?;
    tidal_set(st, |s| { *s = TidalLogin { status: "done".into(), url: None, message: "Tokens saved – downloads from Tidal use this account now.".into(), step: 0, steps: 0 }; });
    st.db().log("info", "streamrip: Tidal tokens pasted in from another computer");
    Ok(tidal_login_status(st))
}

/// "Sign out of Tidal": blank every token field.
pub async fn tidal_sign_out(st: &impl Ctx) -> anyhow::Result<Value> {
    let values: Vec<(String, String)> = TIDAL_TOKEN_KEYS.iter().map(|k| (k.to_string(), "\"\"".to_string())).collect();
    tidal_write(st, &values).await?;
    tidal_set(st, |s| { *s = TidalLogin::default(); });
    Ok(tidal_login_status(st))
}

/// Diagnostics: can this server talk to auth.tidal.com at all? A JSON answer (even an error) means yes.
pub async fn tidal_probe(st: &impl Ctx) -> String {
    match tidal_auth_post(st, "device_authorization", &[("client_id", "encore-probe"), ("scope", TIDAL_SCOPE)], None).await {
        Ok(_) => "Tidal reachable".into(),
        Err(e) if e.starts_with("Could not reach") => e,
        Err(_) => "Tidal blocks this server's address – see Sign in to Tidal".into(),
    }
}
