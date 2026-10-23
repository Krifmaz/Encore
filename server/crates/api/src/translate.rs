//! Translated and romanized titles in the API: the routes of Settings → Language and the edit dialog, and
//! the Translator's row on the old Download tools card. Answers gain a listener's translations through the
//! enrichment pass (crate::enrich); the translations themselves (sources, cache, the model, the background
//! worker) live in encore_library::translate.
use encore_library::translate::*;

use crate::auth::{self, User};
use encore_db::{Row, RowExt};
use encore_core::error::{err, ApiResult};
use crate::state::AppState;
use encore_system::tools::ToolInfo;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use prompt::Kind;
use serde::Deserialize;
use serde_json::{json, Value};

/// The Translator's row on the old Download tools card (`/api/system/tools`, answered from the plugin).
pub fn tool_info(st: &AppState, last: &Value) -> ToolInfo {
    let inst = llama::installed(st).map(|(_, t)| t);
    let m = llama::model(st);
    let has_model = llama::model_path(st, &m).is_file();
    let latest = last["latest"].as_str().map(String::from);
    let newer = inst.as_deref().zip(latest.as_deref()).is_some_and(|(i, l)| llama::newer_build(l, i));
    let note = match (&inst, has_model) {
        (Some(_), true) => format!("Translates titles on this server with the {} model – starts when needed, stops after 2 minutes idle", m.id),
        (Some(_), false) => format!("The {} model is not downloaded yet – Update downloads it (about {:.1} GB)", m.id, m.gb),
        (None, _) => format!("Not installed – install the Translator plugin under Settings → Plugins (llama.cpp and the {} model, about {:.1} GB)", m.id, m.gb),
    };
    ToolInfo {
        id: "translator".into(), name: encore_system::tools::label("translator").into(),
        source: if inst.is_some() { "updated" } else { "missing" }.into(), resettable: st.plugins.installed("translator"),
        update_available: inst.is_some() && (newer || !has_model),
        // a missing model counts as an update: the button downloads it
        latest: if inst.is_some() && !has_model && !newer { inst.clone() } else { latest },
        installed: inst, checked_at: last["checked_at"].as_f64(), error: last["error"].as_str().map(String::from), note, updatable: true,
    }
}


// ---- routes -------------------------------------------------------------------------------------

/// The translation routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/translate/languages", get(languages))
        .route("/api/translate/status", get(status))
        .route("/api/translate/entry", get(get_entry).put(put_entry).delete(delete_entry))
        .route("/api/translate/run", post(run_now))
}

/// The choices Settings → Language offers.
async fn languages(User(_): User) -> Json<Value> {
    Json(json!({
        "languages": script::LANGUAGES.iter().map(|(c, n, _)| json!({ "code": c, "name": n })).collect::<Vec<_>>(),
        "modes": MODES.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
        "default": script::DEFAULT_TARGET,
    }))
}

/// Whether the local model can run, which sources this server uses (`summary`, one line for everyone), and for
/// admins how far the cache is and each source with its switch and state (`tiers`, strongest first).
async fn status(State(st): State<AppState>, User(u): User) -> ApiResult<Json<Value>> {
    let admin = encore_config::perms::is_admin(&u);
    let st2 = st.clone();
    let (ready, counts, sources, summary) = tokio::task::spawn_blocking(move || {
        (llama::ready(&st2), store::counts(&st2.db), admin.then(|| tiers::states(&st2)), tiers::summary(&st2))
    }).await.map_err(|e| err(500, e.to_string()))?;
    let m = llama::model(&st);
    let mut out = json!({
        "translator": ready, "model": m.id, "running": llama::running(&st),
        "can_edit": auth::can(&st, &u, "translate"), "background": st.cfg.bool("translate_background"),
        "summary": summary,
    });
    if admin {
        out["rows"] = json!(counts.0);
        out["shown"] = json!(counts.1);
        out["waiting"] = json!(counts.2);
        out["queued"] = json!(st.library.translator.queued());
        out["tiers"] = json!(sources);
    }
    Ok(Json(out))
}

/// `GET /api/translate/entry` query.
#[derive(Deserialize)]
struct EntryQ {
    /// the text looked up
    text: String,
    /// the language ("" = the listener's own)
    #[serde(default)]
    target: String,
}

/// The language to use: the one asked for, else the listener's preference.
fn target_for(u: &Row, asked: &str) -> &'static str {
    if asked.trim().is_empty() { prefs_of(u).1 } else { script::target(asked) }
}

/// One text's translation for the edit dialog.
async fn get_entry(State(st): State<AppState>, User(u): User, Query(q): Query<EntryQ>) -> ApiResult<Json<Value>> {
    if q.text.trim().is_empty() {
        return Err(err(400, "Nothing to look up"));
    }
    let target = target_for(&u, &q.target);
    Ok(Json(tokio::task::spawn_blocking(move || store::entry(&st.db, &q.text, target)).await.map_err(|e| err(500, e.to_string()))?))
}

/// `PUT /api/translate/entry` body: a manual correction.
#[derive(Deserialize)]
struct EntryBody {
    /// the text corrected
    text: String,
    /// what the text is (a title, an artist …)
    #[serde(default)]
    kind: String,
    /// the language ("" = the listener's own)
    #[serde(default)]
    target: String,
    /// the translation to show
    translation: Option<String>,
    /// the romanization to show
    romanization: Option<String>,
}

/// A manual correction: it wins over every source and stays (empty text = show nothing).
async fn put_entry(State(st): State<AppState>, User(u): User, Json(b): Json<EntryBody>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "translate") {
        return Err(err(403, "You can't edit translations"));
    }
    if b.text.trim().is_empty() || b.text.len() > 1000 {
        return Err(err(400, "Nothing to translate"));
    }
    let long = |s: &Option<String>| s.as_ref().is_some_and(|x| x.len() > 500);
    if long(&b.translation) || long(&b.romanization) {
        return Err(err(400, "That is too long for a title"));
    }
    let target = target_for(&u, &b.target);
    let kind = Kind::parse(&b.kind).as_str();
    let who = u.s("username");
    let out = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        store::set_manual(&st.db, &b.text, target, kind, Some(b.translation.as_deref().unwrap_or("")), Some(b.romanization.as_deref().unwrap_or("")))?;
        st.db.log("info", &format!("{who} corrected the {} translation of “{}”", script::language_name(target), script::normalize(&b.text)));
        Ok(store::entry(&st.db, &b.text, target))
    }).await.map_err(|e| err(500, e.to_string()))??;
    Ok(Json(out))
}

/// Remove a manual correction: what the sources found shows again.
async fn delete_entry(State(st): State<AppState>, User(u): User, Query(q): Query<EntryQ>) -> ApiResult<Json<Value>> {
    if !auth::can(&st, &u, "translate") {
        return Err(err(403, "You can't edit translations"));
    }
    let target = target_for(&u, &q.target);
    let out = tokio::task::spawn_blocking(move || -> rusqlite::Result<Value> {
        store::set_manual(&st.db, &q.text, target, "title", None, None)?;
        Ok(store::entry(&st.db, &q.text, target))
    }).await.map_err(|e| err(500, e.to_string()))??;
    Ok(Json(out))
}

/// Start a pass now (Settings → Language after a change or a source switched on; the Translator plugin wakes the
/// worker itself after an install).
async fn run_now(State(st): State<AppState>, User(_): User) -> Json<Value> {
    st.library.translator.wake();
    Json(json!({ "ok": true }))
}
