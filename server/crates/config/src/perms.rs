//! Roles and permissions: what an account may do. The defaults of some permissions follow the settings
//! (who may pick versions), so the checks take the [`Config`].
use crate::Config;
use encore_db::{Row, RowExt};
use serde_json::{json, Map, Value};

/// Whether account `u` is an admin (the owner counts as one).
pub fn is_admin(u: &Row) -> bool {
    matches!(u.s("role").as_str(), "admin" | "owner")
}

/// Whether account `u` is the owner.
pub fn is_owner(u: &Row) -> bool {
    u.s("role") == "owner"
}

/// Every permission an account can be given, with its description in the UI.
pub const PERMS: &[(&str, &str)] = &[
    ("auto_approve", "Requests skip the approval queue"),
    ("auto_approve_playlists", "Playlist downloads skip the approval queue"),
    ("pick", "Pick the exact version to download (their own requests)"),
    ("fix", "Fix their own downloads: retry, re-pick, delete a wrong song they requested"),
    ("storage", "See server storage"),
    ("server_dsp", "Use server-side Cadenza processing for their streams (costs server CPU)"),
    ("translate", "Correct translated titles (the fix shows for everyone reading that language)"),
    ("own_files", "Replace songs with their own files (⋯ → Replace with my own file; the new file plays for everyone)"),
];

/// Every permission of `u`: the saved ones over the defaults.
pub fn perms_of(cfg: &Config, u: &Row) -> Map<String, Value> {
    let saved: Map<String, Value> = serde_json::from_str(&u.s("perms")).unwrap_or_default();
    let mut out = Map::new();
    out.insert("auto_approve_playlists".into(), json!(false));
    out.insert("pick".into(), json!(cfg.str("manual_search") != "admins"));
    out.insert("fix".into(), json!(true));
    out.insert("storage".into(), json!(true));
    out.insert("server_dsp".into(), json!(false));
    out.insert("translate".into(), json!(true));
    // a replaced file plays for everyone: only admins, unless an admin gives it to an account
    out.insert("own_files".into(), json!(false));
    for (k, v) in saved {
        if PERMS.iter().any(|(p, _)| *p == k) {
            out.insert(k, json!(v.as_bool().unwrap_or(false)));
        }
    }
    out.insert("auto_approve".into(), json!(u.b("auto_approve")));
    out
}

/// `u` may do `perm` (admins may do everything).
pub fn can(cfg: &Config, u: &Row, perm: &str) -> bool {
    is_admin(u) || perms_of(cfg, u).get(perm).and_then(|v| v.as_bool()).unwrap_or(false)
}
