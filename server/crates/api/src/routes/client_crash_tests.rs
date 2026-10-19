//! App crash reports through the real router: any signed-in app uploads, the same key is kept once, only admins
//! list and clear, the newest 50 are kept, and the list line is the summary or the first line of the text.
use axum::http::StatusCode;
use axum::Router;
use serde_json::{json, Value};
use crate::test_support::call;

const PW: &str = "crash-pass-12";

struct Env {
    app: Router,
    dir: std::path::PathBuf,
    admin: String,
    user: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}


async fn login(app: &Router, name: &str) -> String {
    let r = call(app, "POST", "/api/auth/login", "", Some(json!({ "username": name, "password": PW }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    r.2.split(';').next().unwrap().to_string()
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-crash-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "boss", PW, "admin", None, None).unwrap();
    crate::auth::create_user(&st, "phone", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let admin = login(&app, "boss").await;
    let user = login(&app, "phone").await;
    Env { app, dir, admin, user }
}

fn report(key: &str, text: &str) -> Value {
    json!({ "kind": "crash", "device": "Pixel 8", "device_id": "dev-1", "app_version": "0.24.0 (240)", "os": "Android 15 (API 35)",
            "time": 1_700_000_000.0, "summary": "", "text": text, "key": key })
}

#[tokio::test]
async fn upload_dedupe_list_and_clear() {
    let e = setup("flow").await;
    let text = "\n  java.lang.IllegalStateException: boom\n\tat app.encore.X.run(X.kt:1)\n";
    let r = call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(report("crash-1", text))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    assert_eq!(r.1["duplicate"], json!(false));
    let first = r.1["id"].clone();
    // the same report again (its answer got lost): kept once
    let r = call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(report("crash-1", text))).await;
    assert_eq!(r.0, StatusCode::OK);
    assert_eq!(r.1["duplicate"], json!(true));
    assert_eq!(r.1["id"], first);
    // an empty report is refused
    let r = call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(json!({ "text": "  " }))).await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
    // a native crash with its own summary line
    let mut native = report("exit-1699999999000", "*** *** *** tombstone");
    native["kind"] = json!("native");
    native["summary"] = json!("Native crash (SIGSEGV)");
    native["time"] = json!(1_700_000_100.0);
    call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(native)).await;

    // users can't read them, admins can
    let r = call(&e.app, "GET", "/api/diagnostics/client-crash", &e.user, None).await;
    assert_eq!(r.0, StatusCode::FORBIDDEN);
    let r = call(&e.app, "GET", "/api/diagnostics/client-crash", &e.admin, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let list = r.1["crashes"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    // newest first
    assert_eq!(list[0]["kind"], json!("native"));
    assert_eq!(list[0]["first_line"], json!("Native crash (SIGSEGV)"));
    assert_eq!(list[1]["first_line"], json!("java.lang.IllegalStateException: boom"));
    assert_eq!(list[1]["user"], json!("phone"));
    assert_eq!(list[1]["device"], json!("Pixel 8"));
    assert_eq!(list[1]["app_version"], json!("0.24.0 (240)"));
    assert!(list[1]["text"].as_str().unwrap().contains("X.kt:1"));

    let r = call(&e.app, "DELETE", "/api/diagnostics/client-crash", &e.user, None).await;
    assert_eq!(r.0, StatusCode::FORBIDDEN);
    let r = call(&e.app, "DELETE", "/api/diagnostics/client-crash", &e.admin, None).await;
    assert_eq!(r.0, StatusCode::OK);
    let r = call(&e.app, "GET", "/api/diagnostics/client-crash", &e.admin, None).await;
    assert_eq!(r.1["crashes"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn keeps_the_newest_fifty_and_cuts_long_text() {
    let e = setup("keep").await;
    for i in 0..55 {
        let mut b = report(&format!("k{i}"), &format!("Error number {i}"));
        b["time"] = json!(1_700_000_000.0 + i as f64);
        let r = call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(b)).await;
        assert_eq!(r.0, StatusCode::OK);
    }
    let r = call(&e.app, "GET", "/api/diagnostics/client-crash", &e.admin, None).await;
    let list = r.1["crashes"].as_array().unwrap();
    assert_eq!(list.len(), super::KEEP as usize);
    assert_eq!(list[0]["first_line"], json!("Error number 54"));
    assert_eq!(list.last().unwrap()["first_line"], json!("Error number 5"));

    let long = "é".repeat(200_000); // 400 000 bytes
    let mut b = report("long", &long);
    b["time"] = json!(1_800_000_000.0);
    let r = call(&e.app, "POST", "/api/diagnostics/client-crash", &e.user, Some(b)).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let r = call(&e.app, "GET", "/api/diagnostics/client-crash", &e.admin, None).await;
    let text = r.1["crashes"][0]["text"].as_str().unwrap().to_string();
    assert!(text.len() < 270 * 1024, "{}", text.len());
    assert!(text.ends_with("(cut at 256 KB)"));
}

#[test]
fn first_line_rules() {
    assert_eq!(super::first_line("", "\n\n  Boom: x \n at y"), "Boom: x");
    assert_eq!(super::first_line(" ANR ", "trace"), "ANR");
    assert_eq!(super::first_line("", "").len(), 0);
}
