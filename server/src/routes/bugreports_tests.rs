//! Bug reports through the real router: a user files one with a screenshot, the admin sees it as unread, reads
//! it, answers (status → answered, unread for the reporter), changes the status, exports a zip with reports.md,
//! reports.json and the file; another user gets 404 on it; a .exe, a 20 MB file and a PNG that would decode to half
//! a gigabyte are refused; a user is held to 20 reports an hour and 500 MB of files; delete removes the rows and the
//! folder.
use crate::p;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use serde_json::{json, Value};
use std::io::Read;
use tower::ServiceExt;

const PW: &str = "bugs-pass-12";
const BOUNDARY: &str = "----encore-bugreport-test";

struct Env {
    app: Router,
    st: crate::state::AppState,
    dir: std::path::PathBuf,
    admin: String,
    user: String,
    other: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn send(app: &Router, method: &str, uri: &str, cookie: &str, content_type: &str, body: Vec<u8>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri).header("x-requested-with", "encore");
    if !cookie.is_empty() {
        b = b.header("cookie", cookie);
    }
    if !content_type.is_empty() {
        b = b.header("content-type", content_type);
    }
    let resp = app.clone().oneshot(b.body(Body::from(body)).unwrap()).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap();
    (status, headers, bytes.to_vec())
}

async fn call(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> (StatusCode, Value, HeaderMap) {
    let (ct, bytes) = match body {
        Some(j) => ("application/json", j.to_string().into_bytes()),
        None => ("", vec![]),
    };
    let (status, headers, bytes) = send(app, method, uri, cookie, ct, bytes).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), headers)
}

/// A multipart/form-data body: text fields and files (field name "file", with a file name and a type).
fn multipart(fields: &[(&str, &str)], files: &[(&str, &str, &[u8])]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (k, v) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n").as_bytes());
    }
    for (name, mime, data) in files {
        body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

async fn post_form(app: &Router, uri: &str, cookie: &str, fields: &[(&str, &str)], files: &[(&str, &str, &[u8])]) -> (StatusCode, Value) {
    let (ct, body) = multipart(fields, files);
    let (status, _, bytes) = send(app, "POST", uri, cookie, &ct, body).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn login(app: &Router, name: &str) -> String {
    let (status, headers, bytes) = send(app, "POST", "/api/auth/login", "", "application/json", json!({ "username": name, "password": PW }).to_string().into_bytes()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    headers.get("set-cookie").unwrap().to_str().unwrap().split(';').next().unwrap().to_string()
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-bugs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = {
        let _env = crate::TEST_ENV.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ENCORE_CONFIG_DIR", &dir);
        crate::build_state().unwrap()
    };
    crate::auth::create_user(&st, "boss", PW, "admin", None, None).unwrap();
    crate::auth::create_user(&st, "phone", PW, "user", Some("phone@example.com"), None).unwrap();
    crate::auth::create_user(&st, "other", PW, "user", None, None).unwrap();
    let app = crate::app(st.clone());
    let admin = login(&app, "boss").await;
    let user = login(&app, "phone").await;
    let other = login(&app, "other").await;
    Env { app, st, dir, admin, user, other }
}

/// A small real PNG (the image crate has to be able to read it).
fn png() -> Vec<u8> {
    let mut img = image::RgbaImage::new(6, 4);
    for (x, y, p) in img.enumerate_pixels_mut() {
        *p = image::Rgba([x as u8 * 40, y as u8 * 60, 128, 255]);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// A `w`×`h` all-black RGBA8 PNG built by hand, so the test neither holds nor compresses the raw pixels (11000×11000
/// is 484 MB): the IDAT is one fixed-Huffman deflate block – a literal zero, then "copy the last byte" runs of 258.
fn huge_png(w: u32, h: u32) -> Vec<u8> {
    let n: u64 = h as u64 * (1 + w as u64 * 4); // per row a filter byte and w RGBA8 pixels, all zero
    let mut z = vec![0x78u8, 0x01]; // zlib: deflate, 32 KB window, no dictionary
    let (mut acc, mut bits) = (0u64, 0u32);
    let mut put = |z: &mut Vec<u8>, v: u64, k: u32| {
        acc |= v << bits;
        bits += k;
        while bits >= 8 {
            z.push(acc as u8);
            acc >>= 8;
            bits -= 8;
        }
    };
    // a Huffman code goes in from its first bit while the stream fills bytes from the low bit: reversed
    let code = |c: u32, k: u32| (c.reverse_bits() >> (32 - k)) as u64;
    put(&mut z, 1, 1); // the last block
    put(&mut z, 1, 2); // fixed Huffman codes
    put(&mut z, code(0x30, 8), 8); // literal 0
    let mut left = n - 1;
    while left >= 258 {
        put(&mut z, code(0b1100_0101, 8), 8); // length 258 (symbol 285)
        put(&mut z, 0, 5); // distance 1 (code 0)
        left -= 258;
    }
    for _ in 0..left {
        put(&mut z, code(0x30, 8), 8);
    }
    put(&mut z, 0, 7); // end of block (symbol 256)
    if bits > 0 {
        z.push(acc as u8);
    }
    let adler = ((n % 65521) << 16) | 1; // over zero bytes a stays 1 and b counts them
    z.extend_from_slice(&(adler as u32).to_be_bytes());
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]| {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&crc.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bits a sample, RGBA, deflate, adaptive filters, no interlace
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    png
}

async fn unread_count(e: &Env, cookie: &str) -> i64 {
    let r = call(&e.app, "GET", "/api/bug-reports/unread", cookie, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    r.1["count"].as_i64().unwrap()
}

#[tokio::test]
async fn report_reply_status_export_and_delete() {
    let e = setup("flow").await;
    let shot = png();
    // the user files a report with a screenshot
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user,
        &[("kind", "bug"), ("title", "Play button does nothing"), ("body", "Tapped play on an album, nothing happened."), ("page", "#/album/12"),
          ("app_version", "0.26.4"), ("device", "Mozilla/5.0 test 1280x720")],
        &[("../shot one.png", "image/png", &shot)]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(body["ok"], json!(true));
    let id = body["id"].as_i64().unwrap();
    let folder = e.dir.join("reports").join(id.to_string());
    let on_disk: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().map(|f| f.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(on_disk.len(), 1, "{on_disk:?}");
    assert!(on_disk[0].ends_with(".png"), "{on_disk:?}");

    // the list: the admin sees it unread, the reporter has nothing unread yet
    let r = call(&e.app, "GET", "/api/bug-reports", &e.admin, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let list = r.1["reports"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    let row = &list[0];
    assert_eq!(row["id"], json!(id));
    assert_eq!(row["kind"], json!("bug"));
    assert_eq!(row["title"], json!("Play button does nothing"));
    assert_eq!(row["status"], json!("new"));
    assert_eq!(row["username"], json!("phone"));
    assert_eq!(row["message_count"], json!(0));
    assert_eq!(row["file_count"], json!(1));
    assert_eq!(row["unread"], json!(true));
    assert_eq!(unread_count(&e, &e.admin).await, 1);
    let r = call(&e.app, "GET", "/api/bug-reports", &e.user, None).await;
    assert_eq!(r.1["reports"][0]["unread"], json!(false));
    assert_eq!(unread_count(&e, &e.user).await, 0);
    // someone else's list is empty, and mine=1 for the admin shows only the admin's own (none)
    let r = call(&e.app, "GET", "/api/bug-reports", &e.other, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 0);
    let r = call(&e.app, "GET", "/api/bug-reports?mine=1", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 0);
    // filters
    let r = call(&e.app, "GET", "/api/bug-reports?status=fixed", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 0);
    let r = call(&e.app, "GET", "/api/bug-reports?kind=bug&q=play", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 1);

    // the admin reads it: the detail has the file with a url, and the report is no longer unread
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let rep = &r.1["report"];
    assert_eq!(rep["body"], json!("Tapped play on an album, nothing happened."));
    assert_eq!(rep["page"], json!("#/album/12"));
    assert_eq!(rep["app_version"], json!("0.26.4"));
    assert_eq!(rep["user_id"].as_i64().is_some(), true);
    let files = rep["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["name"], json!("shot one.png"), "no path parts in the stored name");
    assert_eq!(files[0]["mime"], json!("image/png"));
    assert_eq!(files[0]["size"], json!(shot.len()));
    let fid = files[0]["id"].as_i64().unwrap();
    assert_eq!(files[0]["url"], json!(format!("/api/bug-reports/{id}/files/{fid}")));
    assert_eq!(r.1["messages"].as_array().unwrap().len(), 0);
    assert_eq!(unread_count(&e, &e.admin).await, 0);
    let r = call(&e.app, "GET", "/api/bug-reports", &e.admin, None).await;
    assert_eq!(r.1["reports"][0]["unread"], json!(false));

    // the file comes back as an inline image, privately cached
    let (status, headers, bytes) = send(&e.app, "GET", &format!("/api/bug-reports/{id}/files/{fid}"), &e.user, "", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, shot);
    assert_eq!(headers.get("content-type").unwrap(), "image/png");
    assert!(headers.get("content-disposition").unwrap().to_str().unwrap().starts_with("inline;"));
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("cache-control").unwrap(), "private, max-age=3600");
    let (status, _, _) = send(&e.app, "GET", &format!("/api/bug-reports/{id}/files/{fid}"), &e.other, "", vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = send(&e.app, "GET", &format!("/api/bug-reports/{id}/files/{}", fid + 100), &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // an empty reply is refused; the admin's reply answers the report and is unread for the reporter
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.admin, &[("body", "   ")], &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert!(body["detail"].as_str().unwrap().ends_with("attach a file"), "{body:?}");
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.admin, &[("body", "Thanks, I can see it. Looking.")], &[]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let r = call(&e.app, "GET", "/api/bug-reports", &e.user, None).await;
    assert_eq!(r.1["reports"][0]["status"], json!("answered"));
    assert_eq!(r.1["reports"][0]["message_count"], json!(1));
    assert_eq!(r.1["reports"][0]["unread"], json!(true));
    assert_eq!(unread_count(&e, &e.user).await, 1);
    assert_eq!(unread_count(&e, &e.admin).await, 0);
    // the reporter reads it: seen
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.user, None).await;
    assert_eq!(r.0, StatusCode::OK);
    let msgs = r.1["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["username"], json!("boss"));
    assert_eq!(msgs[0]["is_admin"], json!(true));
    assert_eq!(msgs[0]["body"], json!("Thanks, I can see it. Looking."));
    assert_eq!(unread_count(&e, &e.user).await, 0);
    // the reporter answers with a log file only: the report is new again and unread for the admin
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.user, &[], &[("console.log", "text/plain", b"TypeError: x is undefined\n")]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.1["report"]["status"], json!("new"));
    let msgs = r.1["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[1]["is_admin"], json!(false));
    assert_eq!(msgs[1]["files"][0]["mime"], json!("text/plain"));
    assert_eq!(msgs[1]["files"][0]["name"], json!("console.log"));
    let log_fid = msgs[1]["files"][0]["id"].as_i64().unwrap();
    let (status, headers, bytes) = send(&e.app, "GET", &format!("/api/bug-reports/{id}/files/{log_fid}"), &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, b"TypeError: x is undefined\n");
    assert!(headers.get("content-disposition").unwrap().to_str().unwrap().starts_with("attachment;"));
    // the unread count for the admin was cleared by that read; a fresh user reply makes it one again
    assert_eq!(unread_count(&e, &e.admin).await, 0);
    let (status, _) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.user, &[("body", "Still happening")], &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unread_count(&e, &e.admin).await, 1);

    // status: admin only, one of the five
    let r = call(&e.app, "POST", &format!("/api/bug-reports/{id}/status"), &e.user, Some(json!({ "status": "fixed" }))).await;
    assert_eq!(r.0, StatusCode::FORBIDDEN);
    let r = call(&e.app, "POST", &format!("/api/bug-reports/{id}/status"), &e.admin, Some(json!({ "status": "done" }))).await;
    assert_eq!(r.0, StatusCode::BAD_REQUEST);
    let r = call(&e.app, "POST", &format!("/api/bug-reports/{id}/status"), &e.admin, Some(json!({ "status": "looking" }))).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.1["report"]["status"], json!("looking"));
    // an admin reply keeps "looking"
    let (status, _) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.admin, &[("body", "Found it")], &[]).await;
    assert_eq!(status, StatusCode::OK);
    let r = call(&e.app, "GET", "/api/bug-reports?status=looking", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 1);

    // export: a zip with reports.md, reports.json and the files
    let (status, _, _) = send(&e.app, "GET", "/api/bug-reports/export", &e.user, "", vec![]).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, headers, bytes) = send(&e.app, "GET", &format!("/api/bug-reports/export?ids={id}"), &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "application/zip");
    let cd = headers.get("content-disposition").unwrap().to_str().unwrap();
    assert!(cd.starts_with("attachment; filename=\"encore-bug-reports-") && cd.ends_with(".zip\""), "{cd}");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: Vec<String> = zip.file_names().map(|s| s.unwrap().into_owned()).collect();
    assert!(names.contains(&"reports.md".to_string()), "{names:?}");
    assert!(names.contains(&"reports.json".to_string()), "{names:?}");
    assert!(names.contains(&format!("files/{id}/{fid}-shot one.png")), "{names:?}");
    assert!(names.contains(&format!("files/{id}/{log_fid}-console.log")), "{names:?}");
    let mut md = String::new();
    zip.by_name("reports.md").unwrap().read_to_string(&mut md).unwrap();
    assert!(md.starts_with(&format!("# Encore bug reports ({}, exported ", crate::VERSION)), "{md}");
    assert!(md.contains(&format!("## #{id} [Bug] Play button does nothing — Looking into it")), "{md}");
    assert!(md.contains("by phone,") && md.contains("page #/album/12") && md.contains("app 0.26.4"), "{md}");
    assert!(md.contains(&format!("![shot one.png](<files/{id}/{fid}-shot one.png>)")), "{md}");
    assert!(md.contains("**boss (admin) — ") && md.contains("**phone — "), "{md}");
    assert!(md.contains(&format!("[console.log](<files/{id}/{log_fid}-console.log>)")), "{md}");
    let mut js = String::new();
    zip.by_name("reports.json").unwrap().read_to_string(&mut js).unwrap();
    let js: Value = serde_json::from_str(&js).unwrap();
    assert_eq!(js["version"], json!(crate::VERSION));
    assert_eq!(js["reports"][0]["id"], json!(id));
    assert_eq!(js["reports"][0]["status"], json!("looking"));
    assert_eq!(js["reports"][0]["files"][0]["path"], json!(format!("files/{id}/{fid}-shot one.png")));
    assert_eq!(js["reports"][0]["messages"].as_array().unwrap().len(), 4);
    let mut img = Vec::new();
    zip.by_name(&format!("files/{id}/{fid}-shot one.png")).unwrap().read_to_end(&mut img).unwrap();
    assert_eq!(img, shot);
    // a filter that matches nothing still gives a zip, with an empty reports.md body
    let (status, _, bytes) = send(&e.app, "GET", "/api/bug-reports/export?status=fixed", &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(zip.len(), 2);
    let mut md = String::new();
    zip.by_name("reports.md").unwrap().read_to_string(&mut md).unwrap();
    assert!(!md.contains("## #"));

    // another user: 404 on reading, replying and deleting is admin only
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.other, None).await;
    assert_eq!(r.0, StatusCode::NOT_FOUND);
    let (status, _) = post_form(&e.app, &format!("/api/bug-reports/{id}/messages"), &e.other, &[("body", "me too")], &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let r = call(&e.app, "DELETE", &format!("/api/bug-reports/{id}"), &e.user, None).await;
    assert_eq!(r.0, StatusCode::FORBIDDEN);
    // signed out: 401
    let r = call(&e.app, "GET", "/api/bug-reports", "", None).await;
    assert_eq!(r.0, StatusCode::UNAUTHORIZED);

    // delete removes the rows and the folder
    assert!(folder.is_dir());
    let r = call(&e.app, "DELETE", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    assert!(!folder.exists());
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.0, StatusCode::NOT_FOUND);
    let r = call(&e.app, "GET", "/api/bug-reports", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 0);
    let (status, _, _) = send(&e.app, "GET", &format!("/api/bug-reports/{id}/files/{fid}"), &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let r = call(&e.app, "DELETE", &format!("/api/bug-reports/{id}"), &e.admin, None).await;
    assert_eq!(r.0, StatusCode::NOT_FOUND);

    // an admin's own report is not news to them
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.admin, &[("kind", "not_bug"), ("title", "Note to self"), ("body", "The volume knob is a feature.")], &[]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    assert_eq!(unread_count(&e, &e.admin).await, 0);
    let r = call(&e.app, "GET", "/api/bug-reports", &e.admin, None).await;
    assert_eq!(r.1["reports"][0]["unread"], json!(false));
}

#[tokio::test]
async fn bad_uploads_are_refused_with_a_sentence() {
    let e = setup("refuse").await;
    let fields = [("kind", "howto"), ("title", "How do I add a playlist?"), ("body", "Can't find the button.")];
    // an .exe, whatever it calls itself
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("setup.exe", "application/octet-stream", b"MZ\x90\x00binary")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert!(body["detail"].as_str().unwrap().starts_with("Only PNG, JPEG, WebP or GIF images"), "{body:?}");
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("shot.png", "image/png", b"MZ\x90\x00not a png")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    // a PNG header with garbage behind it can't be read
    let mut broken = b"\x89PNG\r\n\x1a\n".to_vec();
    broken.extend_from_slice(&[7u8; 64]);
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("shot.png", "image/png", &broken)]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("That image can't be read"));
    // a 3 MB PNG that would decode to 484 MB is refused on its header, with the same sentence
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("huge.png", "image/png", &huge_png(11000, 11000))]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("That image can't be read"));
    // 20 MB is over the 8 MB per file
    let big = vec![0u8; 20 * 1024 * 1024];
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("big.txt", "text/plain", &big)]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("Each file can be at most 8 MB"));
    // seven files are one too many
    let shot = png();
    let seven: Vec<(&str, &str, &[u8])> = (0..7).map(|_| ("a.png", "image/png", shot.as_slice())).collect();
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &seven).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("At most 6 files per report"));
    // an empty title or body
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &[("kind", "bug"), ("title", "  "), ("body", "x")], &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], json!("Give the report a title"));
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &[("kind", "bug"), ("title", "Title"), ("body", "")], &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], json!("Describe what happened"));
    let long = "t".repeat(201);
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &[("kind", "bug"), ("title", &long), ("body", "x")], &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["detail"], json!("The title can have at most 200 characters"));
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &[("kind", "feature"), ("title", "Title"), ("body", "x")], &[]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    // nothing was filed, no folder was made
    let r = call(&e.app, "GET", "/api/bug-reports", &e.admin, None).await;
    assert_eq!(r.1["reports"].as_array().unwrap().len(), 0);
    assert!(!e.dir.join("reports").exists());
    // a sound one with a text file and no kind (bug by default), a real 1×1 GIF, and a PNG that calls itself .html;
    // the title's line breaks (legal in a form field) are folded, so it can't add lines to the export
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &[("title", "Notes\r\n## #999 [Bug] fake — Fixed\r\n"), ("body", "see the log")],
        &[("notes.txt", "text/plain", "héllo\n".as_bytes()), ("anim.gif", "image/gif", gif), ("readme.html", "text/html", &shot)]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let id = body["id"].as_i64().unwrap();
    let r = call(&e.app, "GET", &format!("/api/bug-reports/{id}"), &e.user, None).await;
    assert_eq!(r.1["report"]["kind"], json!("bug"));
    assert_eq!(r.1["report"]["title"], json!("Notes ## #999 [Bug] fake — Fixed"));
    let files = r.1["report"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!((files[0]["mime"].as_str().unwrap(), files[1]["mime"].as_str().unwrap()), ("text/plain", "image/gif"));
    assert_eq!((files[2]["mime"].as_str().unwrap(), files[2]["name"].as_str().unwrap()), ("image/png", "readme.png"));
    // a GIF is decoded like the others: a header with garbage behind it is refused
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[("anim.gif", "image/gif", b"GIF89a\x01\x00\x01\x00garbage")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("That image can't be read"));
}

#[tokio::test]
async fn too_many_or_too_much_is_refused() {
    let e = setup("limits").await;
    let fields = [("kind", "bug"), ("title", "Again"), ("body", "the same thing")];
    // a user may file 20 reports an hour; the 21st waits
    let mut first = 0;
    for i in 0..20 {
        let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[]).await;
        assert_eq!(status, StatusCode::OK, "report {i}: {body:?}");
        if i == 0 {
            first = body["id"].as_i64().unwrap();
        }
    }
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.user, &fields, &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body:?}");
    assert_eq!(body["detail"], json!("Too many reports in a short time – try again later"));
    // replies are counted apart from reports, and an admin is not held back
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{first}/messages"), &e.user, &[("body", "still happening")], &[]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    for i in 0..21 {
        let (status, body) = post_form(&e.app, "/api/bug-reports", &e.admin, &fields, &[]).await;
        assert_eq!(status, StatusCode::OK, "admin report {i}: {body:?}");
    }

    // a user's reports may hold 500 MB of files in all – pretend other's report already does
    let (status, body) = post_form(&e.app, "/api/bug-reports", &e.other, &fields, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let rid = body["id"].as_i64().unwrap();
    e.st.db.execute("INSERT INTO bug_report_files (report_id, message_id, name, mime, size, path, created_at) VALUES (?,NULL,'old.log','text/plain',?,'',0)",
        p![rid, 600i64 * 1024 * 1024]).unwrap();
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{rid}/messages"), &e.other, &[], &[("more.log", "text/plain", b"line\n")]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body:?}");
    assert_eq!(body["detail"], json!("Your reports already hold 600 MB of files (the most is 500 MB); the admin can delete old ones"));
    // words alone still go through, and the admin may attach what they like
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{rid}/messages"), &e.other, &[("body", "no file then")], &[]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");
    let (status, body) = post_form(&e.app, &format!("/api/bug-reports/{rid}/messages"), &e.admin, &[], &[("more.log", "text/plain", b"line\n")]).await;
    assert_eq!(status, StatusCode::OK, "{body:?}");

    // the export is built in memory: over 512 MB of files it asks for a narrower selection
    let (status, _, bytes) = send(&e.app, "GET", "/api/bug-reports/export", &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["detail"], json!("That export would be over 512 MB – narrow it with the filters or export reports one by one"));
    let (status, headers, _) = send(&e.app, "GET", &format!("/api/bug-reports/export?ids={first}"), &e.admin, "", vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "application/zip");
}

#[test]
fn names_dates_and_labels() {
    assert_eq!(super::safe_name("../../etc/passwd.png", "png"), "passwd.png");
    assert_eq!(super::safe_name("C:\\Users\\me\\shot:1?.png", "png"), "shot_1_.png");
    assert_eq!(super::safe_name("", "jpg"), "file.jpg");
    assert_eq!(super::safe_name("...", "jpg"), "file.jpg");
    assert_eq!(super::safe_name(&"é".repeat(200), "png").chars().count(), 120);
    // the sniffed type names the file, not the browser
    assert_eq!(super::safe_name("readme.html", "png"), "readme.png");
    assert_eq!(super::safe_name("shot", "png"), "shot.png");
    assert_eq!(super::safe_name("Screenshot 2026-10-18 (1).png", "png"), "Screenshot 2026-10-18 (1).png");
    assert_eq!(super::safe_name(".hidden", "jpg"), "hidden.jpg");
    // a title stays on one line
    assert_eq!(super::line("Play stops\r\n## #999 [Bug] fake — Fixed\r\n"), "Play stops ## #999 [Bug] fake — Fixed");
    assert_eq!(super::line("  two\t\tspaces  "), "two spaces");
    assert_eq!(super::ymd(0.0), "1970-01-01");
    assert_eq!(super::ymd(1_700_000_000.0), "2023-11-14");
    assert_eq!(super::kind_label("howto"), "How do I…");
    assert_eq!(super::status_label("wontfix"), "Won't fix");
    assert_eq!(super::status_label("anything"), "New");
    assert_eq!(super::check_file("notes.log", b"line").unwrap(), ("text/plain", "log"));
    assert!(super::check_file("notes.txt", &[0xff, 0xfe, 0x00]).is_err());
    assert!(super::check_file("notes.md", b"text").is_err());
}

#[test]
fn huge_images_are_refused_on_their_header() {
    // the hand-made PNG is a sound one
    assert_eq!(super::check_file("ok.png", &huge_png(64, 48)).unwrap(), ("image/png", "png"));
    // 11000×11000 RGBA8 would take 484 MB: refused before the buffer is allocated
    let e = super::check_file("huge.png", &huge_png(11000, 11000)).unwrap_err();
    assert_eq!((e.status.as_u16(), e.detail.as_str()), (400, "That image can't be read"));
    // and so is one side over 8192
    assert!(super::check_file("wide.png", &huge_png(8193, 2)).is_err());
    assert!(super::check_file("fine.png", &huge_png(8192, 2)).is_ok());
}
