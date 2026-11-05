//! Encore's server state handed to the Krifmaz kit's server crates (`kit-server-*`): Encore's sign-in check
//! (cookie session with its CSRF header, app token or API key), so a route served by a kit crate answers exactly
//! as Encore's own did, and the stores the crates keep their data in ([`KitStores`]).
//!
//! Where the data is:
//! - bug reports, their replies and files, and the apps' crash reports: the kit's own tables (`kit_bug_reports`,
//!   `kit_bug_report_messages`, `kit_bug_report_files`, `kit_client_crashes`) in Encore's database, through
//!   Encore's own connections ([`kit_server_bugreports::SqliteReports`] on [`Db`]). The rows Encore kept in its
//!   tables of the same shape before (`bug_reports`, …, `client_crashes`) are copied over once, ids and all
//!   ([`import_legacy`]); those tables are left as they were.
//! - each account's What's new marks (`whats_new_seen`, `whats_new_popup`): in the account's own preferences
//!   (`users.prefs`), next to the rest of them ([`AccountPrefs`]).
//! - the settings: Encore's `settings` table (`encore_config`).
use crate::auth::{Admin, User};
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use encore_db::{p, Db};
use kit_server_bugreports::SqliteReports;
use kit_server_core::{async_trait, Account, Error, Host, Migrations, PrefsStore, Result};
use serde_json::{json, Value};

impl Host for AppState {
    async fn signed_in(&self, parts: &mut Parts) -> Result<Account> {
        let User(u) = User::from_request_parts(parts, self).await?;
        Ok(Account::from_row(&u))
    }

    async fn admin(&self, parts: &mut Parts) -> Result<Account> {
        let Admin(u) = Admin::from_request_parts(parts, self).await?;
        Ok(Account::from_row(&u))
    }
}

/// The stores the kit crates keep their data in. Clones share them.
#[derive(Clone)]
pub struct KitStores {
    /// bug reports and the apps' crash reports, in the kit's tables of Encore's database
    pub reports: SqliteReports,
    /// each account's preferences, where Encore keeps them (`users.prefs`)
    pub prefs: AccountPrefs,
}

impl KitStores {
    /// The stores on `db`: the kit's tables made or brought up to date, and what Encore kept before copied
    /// into them once.
    pub fn open(db: &Db) -> Result<KitStores> {
        let reports = SqliteReports::new(db.clone())?;
        import_legacy(db)?;
        Ok(KitStores {
            reports,
            prefs: AccountPrefs(db.clone()),
        })
    }
}

/// Copies the bug reports (with their replies and file records) from Encore's `bug_reports`,
/// `bug_report_messages` and `bug_report_files`. Same columns, same ids, so the files on disk
/// (`reports/<report id>/<file id>.<ext>`) and every link to a report stay as they were.
const IMPORT_BUG_REPORTS: Migrations = Migrations {
    component: "encore-import-bug-reports",
    steps: &["INSERT INTO kit_bug_reports (id, user_id, username, kind, title, body, page, app_version, device, status,
                created_at, updated_at, last_reply_by, user_seen_at, admin_seen_at)
              SELECT id, COALESCE(user_id, 0), COALESCE(username, ''), COALESCE(kind, ''), COALESCE(title, ''),
                COALESCE(body, ''), COALESCE(page, ''), COALESCE(app_version, ''), COALESCE(device, ''),
                COALESCE(status, 'new'), COALESCE(created_at, 0), COALESCE(updated_at, 0), COALESCE(last_reply_by, ''),
                COALESCE(user_seen_at, 0), COALESCE(admin_seen_at, 0)
              FROM bug_reports;
              INSERT INTO kit_bug_report_messages (id, report_id, user_id, username, is_admin, body, created_at)
              SELECT id, COALESCE(report_id, 0), COALESCE(user_id, 0), COALESCE(username, ''), COALESCE(is_admin, 0),
                COALESCE(body, ''), COALESCE(created_at, 0)
              FROM bug_report_messages;
              INSERT INTO kit_bug_report_files (id, report_id, message_id, name, mime, size, path, created_at)
              SELECT id, COALESCE(report_id, 0), message_id, COALESCE(name, ''), COALESCE(mime, ''), COALESCE(size, 0),
                COALESCE(path, ''), COALESCE(created_at, 0)
              FROM bug_report_files"],
};

/// Copies the apps' crash reports from Encore's `client_crashes` (same columns, same ids).
const IMPORT_CRASHES: Migrations = Migrations {
    component: "encore-import-crashes",
    steps: &["INSERT INTO kit_client_crashes (id, user_id, username, kind, device, device_id, app_version, os, time,
                received_at, first_line, text, dedupe)
              SELECT id, COALESCE(user_id, 0), COALESCE(username, ''), COALESCE(kind, ''), COALESCE(device, ''),
                COALESCE(device_id, ''), COALESCE(app_version, ''), COALESCE(os, ''), COALESCE(time, 0),
                COALESCE(received_at, 0), COALESCE(first_line, ''), COALESCE(text, ''), dedupe
              FROM client_crashes"],
};

/// The import of a component whose old tables were never made (a newer install, or nobody ever filed one):
/// recorded as done with nothing to copy.
const fn nothing_to_import(component: &'static str) -> Migrations {
    Migrations {
        component,
        steps: &["SELECT 1"],
    }
}

/// Copies what Encore kept in its own tables before the kit's tables held it, once per database (recorded in
/// `kit_migrations`, each copy in one transaction). The old tables were made on first use, so a database may
/// not have them: then there is nothing to copy.
fn import_legacy(db: &Db) -> Result<()> {
    let has = |table: &str| -> Result<bool> {
        let row = db
            .one(
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE type = 'table' AND name = ?",
                p![table],
            )
            .map_err(Error::database)?;
        Ok(row
            .and_then(|r| r.get("n").and_then(Value::as_i64))
            .unwrap_or(0)
            > 0)
    };
    let bug_reports = ["bug_reports", "bug_report_messages", "bug_report_files"];
    let mut all = true;
    for t in bug_reports {
        all &= has(t)?;
    }
    if all {
        IMPORT_BUG_REPORTS.run(db)?;
    } else {
        nothing_to_import(IMPORT_BUG_REPORTS.component).run(db)?;
    }
    if has("client_crashes")? {
        IMPORT_CRASHES.run(db)?;
    } else {
        nothing_to_import(IMPORT_CRASHES.component).run(db)?;
    }
    Ok(())
}

/// Each account's preferences where Encore keeps them: the JSON object in `users.prefs`, which the web app's
/// own settings (theme, start page …) share. The kit's What's new routes keep `whats_new_seen` and
/// `whats_new_popup` in it, as Encore always did.
#[derive(Clone)]
pub struct AccountPrefs(Db);

#[async_trait]
impl PrefsStore for AccountPrefs {
    async fn prefs(&self, account: i64) -> Result<Value> {
        let row = self
            .0
            .one("SELECT prefs FROM users WHERE id = ?", p![account])
            .map_err(Error::database)?;
        let text = row
            .and_then(|r| r.get("prefs").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        Ok(match serde_json::from_str::<Value>(&text) {
            Ok(v @ Value::Object(_)) => v,
            _ => json!({}),
        })
    }

    async fn save_prefs(&self, account: i64, prefs: &Value) -> Result<()> {
        self.0
            .execute(
                "UPDATE users SET prefs = ? WHERE id = ?",
                p![prefs.to_string(), account],
            )
            .map_err(Error::database)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database from before the kit's tables: its bug reports, replies, files and crash reports are copied
    /// over once, ids and all, the old tables are left as they were, and a second start copies nothing again.
    #[test]
    fn the_old_reports_are_copied_once() {
        let dir = encore_core::tmp::dir("kit-import");
        let db = Db::open(&dir).unwrap();
        db.execute_batch(
            "CREATE TABLE bug_reports (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER, username TEXT, kind TEXT, title TEXT,
                body TEXT, page TEXT, app_version TEXT, device TEXT, status TEXT DEFAULT 'new', created_at REAL, updated_at REAL,
                last_reply_by TEXT DEFAULT '', user_seen_at REAL DEFAULT 0, admin_seen_at REAL DEFAULT 0);
             CREATE TABLE bug_report_messages (id INTEGER PRIMARY KEY AUTOINCREMENT, report_id INTEGER, user_id INTEGER, username TEXT,
                is_admin INTEGER, body TEXT, created_at REAL);
             CREATE TABLE bug_report_files (id INTEGER PRIMARY KEY AUTOINCREMENT, report_id INTEGER, message_id INTEGER, name TEXT,
                mime TEXT, size INTEGER, path TEXT, created_at REAL);
             CREATE TABLE client_crashes (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER, username TEXT, kind TEXT, device TEXT,
                device_id TEXT, app_version TEXT, os TEXT, time REAL, received_at REAL, first_line TEXT, text TEXT, dedupe TEXT);
             INSERT INTO bug_reports (id, user_id, username, kind, title, body, status, created_at, updated_at, page)
                VALUES (7, 2, 'ann', 'bug', 'Play stops', 'x', 'looking', 1, 2, NULL);
             INSERT INTO bug_report_messages (id, report_id, user_id, username, is_admin, body, created_at) VALUES (3, 7, 1, 'admin', 1, 'On it', 2);
             INSERT INTO bug_report_files (id, report_id, message_id, name, mime, size, path, created_at)
                VALUES (5, 7, NULL, 'log.txt', 'text/plain', 4, 'reports/7/5.txt', 1);
             INSERT INTO client_crashes (id, user_id, username, kind, time, received_at, first_line, text, dedupe)
                VALUES (9, 2, 'ann', 'crash', 1, 1, 'Boom', 'Boom\n at x', '2|d|k');",
        )
        .unwrap();
        KitStores::open(&db).unwrap();
        KitStores::open(&db).unwrap();
        let one = |sql: &str| db.one(sql, p![]).unwrap().unwrap();
        let r = one("SELECT * FROM kit_bug_reports");
        assert_eq!(
            (
                r["id"].clone(),
                r["title"].clone(),
                r["status"].clone(),
                r["page"].clone()
            ),
            (json!(7), json!("Play stops"), json!("looking"), json!(""))
        );
        assert_eq!(
            one("SELECT COUNT(*) n FROM kit_bug_reports")["n"],
            json!(1),
            "copied once"
        );
        assert_eq!(
            one("SELECT id, report_id, is_admin FROM kit_bug_report_messages"),
            one("SELECT 3 id, 7 report_id, 1 is_admin")
        );
        let f = one("SELECT * FROM kit_bug_report_files");
        assert_eq!(
            (f["id"].clone(), f["message_id"].clone(), f["path"].clone()),
            (json!(5), Value::Null, json!("reports/7/5.txt"))
        );
        let c = one("SELECT * FROM kit_client_crashes");
        assert_eq!(
            (c["id"].clone(), c["dedupe"].clone(), c["device"].clone()),
            (json!(9), json!("2|d|k"), json!(""))
        );
        assert_eq!(
            one("SELECT COUNT(*) n FROM bug_reports")["n"],
            json!(1),
            "the old table is left as it was"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A newer database never had the old tables: nothing to copy, and the stores start all the same.
    #[test]
    fn a_new_database_has_nothing_to_copy() {
        let dir = encore_core::tmp::dir("kit-import-new");
        let db = Db::open(&dir).unwrap();
        KitStores::open(&db).unwrap();
        assert_eq!(
            db.one("SELECT COUNT(*) n FROM kit_bug_reports", p![])
                .unwrap()
                .unwrap()["n"],
            json!(0)
        );
        assert_eq!(
            db.one(
                "SELECT COUNT(*) n FROM kit_migrations WHERE component LIKE 'encore-import-%'",
                p![]
            )
            .unwrap()
            .unwrap()["n"],
            json!(2)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The What's new marks live in the account's own preferences, next to the rest of them.
    #[tokio::test]
    async fn prefs_are_the_accounts_own() {
        let dir = encore_core::tmp::dir("kit-prefs");
        let db = Db::open(&dir).unwrap();
        db.execute("INSERT INTO users (id, username, password_hash, role, prefs, created_at) VALUES (4, 'bo', 'x', 'user', ?, 0)", p![r#"{"theme":"dark"}"#])
            .unwrap();
        let prefs = AccountPrefs(db.clone());
        assert_eq!(prefs.prefs(4).await.unwrap(), json!({ "theme": "dark" }));
        prefs
            .save_prefs(4, &json!({ "theme": "dark", "whats_new_seen": "0.43.0" }))
            .await
            .unwrap();
        assert_eq!(
            db.one("SELECT prefs FROM users WHERE id = 4", p![])
                .unwrap()
                .unwrap()["prefs"],
            json!(r#"{"theme":"dark","whats_new_seen":"0.43.0"}"#)
        );
        assert_eq!(
            prefs.prefs(99).await.unwrap(),
            json!({}),
            "no such account: nothing saved"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
