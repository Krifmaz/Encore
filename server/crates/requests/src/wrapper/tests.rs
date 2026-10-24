//! The wrapper plugin's tests: the app's libraries taken out of an APK and out of an APKM bundle (and a wrong
//! version refused before any build), the Android files staged, the Dockerfile without its cache mounts, the
//! build context as a tar that unpacks again, the sign-in with its two-factor code against a stand-in
//! wrapper-v2 (and gamdl pointed at it), and the container made, started, stopped and left alone through a
//! stand-in Docker on a unix socket.
use super::stage::{self, Arch, Pins};
use super::*;
use crate::test_ctx::TestCtx;
use axum::extract::State as AxState;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Json;
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// A fresh empty folder for one test.
fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("encore-wrapper-{name}-{}-{}", std::process::id(), encore_core::ids::random_hex(4)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A zip of (name, bytes) members, in memory.
fn zip_of(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, bytes) in members {
            z.start_file(*name, opts).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}

/// The two Apple libraries of the stand-in app, and pins that match them.
fn libs() -> Vec<(&'static str, Vec<u8>)> {
    vec![("libandroidappmusic.so", b"apple music lib".to_vec()), ("libstoreservicescore.so", b"store services".to_vec())]
}

fn pins_for(libs: &[(&str, Vec<u8>)]) -> Pins {
    Pins {
        libs: libs.iter().map(|(n, b)| (n.to_string(), stage::sha256_hex(b))).collect(),
        system: [("bin/linker64".to_string(), stage::sha256_hex(b"linker"))].into(),
        apple_version: "3.6.0-beta".into(),
        apple_build: "1109".into(),
    }
}

/// An APK with the libraries under `dir`.
fn apk_with(dir: &str, libs: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut members: Vec<(String, Vec<u8>)> = libs.iter().map(|(n, b)| (format!("{dir}/{n}"), b.clone())).collect();
    members.push(("AndroidManifest.xml".into(), b"<manifest/>".to_vec()));
    let refs: Vec<(&str, Vec<u8>)> = members.iter().map(|(n, b)| (n.as_str(), b.clone())).collect();
    zip_of(&refs)
}

#[test]
fn the_libraries_come_out_of_a_plain_apk_and_an_apkm_bundle() {
    let d = tmp("extract");
    let pins = pins_for(&libs());
    // a plain APK
    std::fs::write(d.join("app.apk"), apk_with("lib/x86_64", &libs())).unwrap();
    assert_eq!(stage::bundle_kind(&d.join("app.apk")), Some("APK"));
    let out = d.join("out1");
    let x = stage::extract_libs(&d.join("app.apk"), Arch::X86_64, &pins, &out).unwrap();
    assert_eq!((x.libs, x.from.as_str(), x.dir.as_str()), (2, "the APK", "lib/x86_64"));
    assert_eq!(std::fs::read(out.join("libandroidappmusic.so")).unwrap(), b"apple music lib");
    // an APKM: base.apk without libraries, the arm split, the x86_64 split with them
    let bundle = zip_of(&[
        ("base.apk", zip_of(&[("classes.dex", b"dex".to_vec())])),
        ("split_config.arm64_v8a.apk", apk_with("lib/arm64-v8a", &libs())),
        ("split_config.x86_64.apk", apk_with("lib/x86_64", &libs())),
        ("info.json", b"{}".to_vec()),
    ]);
    std::fs::write(d.join("app.apkm"), bundle).unwrap();
    assert_eq!(stage::bundle_kind(&d.join("app.apkm")), Some("APKM bundle"));
    let x = stage::extract_libs(&d.join("app.apkm"), Arch::X86_64, &pins, &d.join("out2")).unwrap();
    assert_eq!((x.from.as_str(), x.dir.as_str()), ("split_config.x86_64.apk", "lib/x86_64"));
    let x = stage::extract_libs(&d.join("app.apkm"), Arch::Arm64, &pins, &d.join("out3")).unwrap();
    assert_eq!((x.from.as_str(), x.dir.as_str()), ("split_config.arm64_v8a.apk", "lib/arm64-v8a"));
    // not an app at all
    std::fs::write(d.join("notes.zip"), zip_of(&[("readme.txt", b"hi".to_vec())])).unwrap();
    assert_eq!(stage::bundle_kind(&d.join("notes.zip")), None);
    std::fs::write(d.join("plain.txt"), b"hello").unwrap();
    assert!(stage::extract_libs(&d.join("plain.txt"), Arch::X86_64, &pins, &d.join("o")).unwrap_err().to_string().contains("not an APK"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn another_apple_music_version_is_refused_before_anything_is_written() {
    let d = tmp("version");
    let pins = pins_for(&libs());
    let other = vec![("libandroidappmusic.so", b"a newer build".to_vec()), ("libstoreservicescore.so", b"store services".to_vec())];
    std::fs::write(d.join("app.apk"), apk_with("lib/x86_64", &other)).unwrap();
    let out = d.join("out");
    let e = stage::extract_libs(&d.join("app.apk"), Arch::X86_64, &pins, &out).unwrap_err().to_string();
    assert!(e.contains("libandroidappmusic.so differs"), "{e}");
    assert!(e.contains("Apple Music for Android 3.6.0-beta (build 1109)"), "says which version to bring: {e}");
    assert!(!out.join("libstoreservicescore.so").exists(), "nothing written");
    // a library missing
    std::fs::write(d.join("short.apk"), apk_with("lib/x86_64", &libs()[..1])).unwrap();
    let e = stage::extract_libs(&d.join("short.apk"), Arch::X86_64, &pins, &out).unwrap_err().to_string();
    assert!(e.contains("libstoreservicescore.so is missing"), "{e}");
    // the libraries of the other architecture only
    std::fs::write(d.join("arm.apk"), apk_with("lib/arm64-v8a", &libs())).unwrap();
    // the sentinel's folder is taken whatever its name, then its libraries are checked
    let x = stage::extract_libs(&d.join("arm.apk"), Arch::X86_64, &pins, &d.join("out-arm")).unwrap();
    assert_eq!(x.dir, "lib/arm64-v8a");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn splits_are_scored_as_the_script_scores_them() {
    assert_eq!(stage::split_score("split_config.x86_64.apk", Arch::X86_64), 160);
    assert_eq!(stage::split_score("split_config.arm64_v8a.apk", Arch::Arm64), 150);
    assert_eq!(stage::split_score("base.apk", Arch::X86_64), -20);
    assert_eq!(stage::split_score("split_config.en.apk", Arch::X86_64), 0);
    assert_eq!(stage::split_score("x/split_x86.apk", Arch::X86_64), 10);
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(stage::lib_dir(&names(&["lib/x86/libandroidappmusic.so"]), Arch::X86_64).as_deref(), Some("lib/x86"));
    assert_eq!(stage::lib_dir(&names(&["lib/x86_64/other.so"]), Arch::X86_64), None);
    assert_eq!(stage::lib_dir(&names(&["assets/lib/x/libandroidappmusic.so"]), Arch::X86_64), None, "only lib/<abi>/");
}

#[test]
fn pins_read_from_libs_version_json() {
    let text = r#"{ "$comment": "x", "apple_music": { "version": "3.6.0-beta", "build": "1109" },
        "android_system": { "$comment": "y", "x86_64": { "bin/linker64": "AB12" } },
        "libs": { "x86_64": { "libandroidappmusic.so": "cd34" }, "arm64-v8a": {} } }"#;
    let p = Pins::parse(text, Arch::X86_64).unwrap();
    assert_eq!(p.libs.get("libandroidappmusic.so").map(String::as_str), Some("cd34"));
    assert_eq!(p.system.get("bin/linker64").map(String::as_str), Some("ab12"), "hashes compare in lower case");
    assert_eq!(p.wanted(), "Apple Music for Android 3.6.0-beta (build 1109)");
    assert!(Pins::parse(text, Arch::Arm64).is_err(), "nothing pinned for arm64 here");
    assert_eq!(Arch::of_machine("aarch64"), Some(Arch::Arm64));
    assert_eq!(Arch::of_machine("riscv64"), None);
}

/// The Dockerfile's apt step as wrapper-v2 writes it.
const DOCKERFILE: &str = "FROM --platform=linux/amd64 debian:13.2 AS build\n\
RUN --mount=type=cache,target=/var/lib/apt,sharing=locked \\\n    --mount=type=cache,target=/var/cache/apt,sharing=locked \\\n    apt-get update && \\\n    apt-get install -y cmake\n\
COPY . /app\n";

#[test]
fn the_dockerfile_loses_only_its_cache_mounts() {
    let out = stage::patch_dockerfile(DOCKERFILE);
    assert_eq!(out, "FROM --platform=linux/amd64 debian:13.2 AS build\nRUN \\\n    apt-get update && \\\n    apt-get install -y cmake\nCOPY . /app\n");
    assert_eq!(stage::patch_dockerfile(&out), out, "nothing more to take out");
}

/// A stand-in wrapper-v2 source: its pins, its Android files, a Dockerfile, the repository's own folders.
fn fake_source(dir: &Path, libs: &[(&str, Vec<u8>)]) {
    let pins = pins_for(libs);
    let libs_json: serde_json::Map<String, Value> = pins.libs.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let text = json!({
        "apple_music": { "version": "3.6.0-beta", "build": "1109" },
        "android_system": { "x86_64": { "bin/linker64": stage::sha256_hex(b"linker") }, "arm64-v8a": { "bin/linker64": stage::sha256_hex(b"linker") } },
        "libs": { "x86_64": libs_json.clone(), "arm64-v8a": libs_json },
    });
    std::fs::write(dir.join("LIBS_VERSION.json"), text.to_string()).unwrap();
    for arch in ["x86_64", "arm64-v8a"] {
        std::fs::create_dir_all(dir.join("vendor/android-system").join(arch).join("bin")).unwrap();
        std::fs::write(dir.join("vendor/android-system").join(arch).join("bin/linker64"), b"linker").unwrap();
    }
    std::fs::write(dir.join("Dockerfile"), DOCKERFILE).unwrap();
    std::fs::create_dir_all(dir.join(".github/workflows")).unwrap();
    std::fs::write(dir.join(".github/workflows/build.yml"), "on: push").unwrap();
    std::fs::create_dir_all(dir.join("src/rust")).unwrap();
    std::fs::write(dir.join("src/rust/main.rs"), "fn main() {}").unwrap();
    std::fs::create_dir_all(dir.join("rootfs/system/lib64")).unwrap();
    std::fs::write(dir.join("rootfs/system/lib64/.gitkeep"), "").unwrap();
    // a long name, past the 100 bytes of a plain tar name
    let deep = dir.join("src/daemon/apple/a-rather-long-folder-name-for-the-tar-prefix-field/and-another-one-to-pass-a-hundred");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("thunks.hpp"), "// x").unwrap();
}

#[test]
fn prepare_stages_the_source_and_the_context_unpacks_again() {
    let d = tmp("prepare");
    let src = d.join("wrapper-v2-100e0a8");
    std::fs::create_dir_all(&src).unwrap();
    fake_source(&src, &libs());
    assert_eq!(stage::top_folder(&d), src, "a GitHub tarball's one top folder");
    std::fs::write(d.join("app.apk"), apk_with("lib/x86_64", &libs())).unwrap();
    let p = stage::prepare(&src, &d.join("app.apk"), Arch::X86_64).unwrap();
    assert_eq!((p.libs.libs, p.system), (2, 1));
    assert!(src.join("rootfs/system/lib64/libandroidappmusic.so").is_file());
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(src.join("rootfs/system/bin/linker64")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "the linker runs");
    }
    assert!(!std::fs::read_to_string(src.join("Dockerfile")).unwrap().contains("--mount="));
    // the context: a tar Encore's own unpacker reads back (gzipped here, as it takes .tar.gz)
    let tar = d.join("context.tar");
    let n = stage::write_tar(&src, &tar).unwrap();
    assert_eq!(n, std::fs::metadata(&tar).unwrap().len());
    assert_eq!(n % 512, 0);
    let gz = d.join("context.tar.gz");
    {
        let mut e = flate2::write::GzEncoder::new(std::fs::File::create(&gz).unwrap(), flate2::Compression::fast());
        std::io::copy(&mut std::fs::File::open(&tar).unwrap(), &mut e).unwrap();
        e.finish().unwrap();
    }
    let back = d.join("back");
    encore_plugins::archive::unpack(&gz, encore_plugins::archive::Kind::TarGz, &back, Default::default()).unwrap();
    assert_eq!(std::fs::read(back.join("rootfs/system/lib64/libstoreservicescore.so")).unwrap(), b"store services");
    assert_eq!(std::fs::read(back.join("src/rust/main.rs")).unwrap(), b"fn main() {}");
    assert!(back.join("src/daemon/apple/a-rather-long-folder-name-for-the-tar-prefix-field/and-another-one-to-pass-a-hundred/thunks.hpp").is_file(), "a long name survives");
    assert!(!back.join("vendor").exists() && !back.join(".github").exists(), "the vendor files are in the rootfs; the repository's own folders stay out");
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(back.join("rootfs/system/bin/linker64")).unwrap().permissions().mode() & 0o111, 0o111);
    }
    let _ = std::fs::remove_dir_all(&d);
}

// ---- the sign-in against a stand-in wrapper ----------------------------------------------------------------

/// What the stand-in wrapper saw and holds.
#[derive(Clone, Default)]
struct Fake {
    state: Arc<Mutex<String>>,
    bodies: Arc<Mutex<Vec<Value>>>,
}

async fn me(AxState(f): AxState<Fake>) -> Json<Value> {
    let state = f.state.lock().unwrap().clone();
    Json(json!({ "version": "0.0.2", "runtime": { "loader_ok": true }, "auth": { "state": state, "apple_id": "me@example.com", "storefront": "us", "music_user_token": "SECRET" } }))
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "version": "0.0.2" }))
}

async fn login(AxState(f): AxState<Fake>, Json(b): Json<Value>) -> axum::response::Response {
    f.bodies.lock().unwrap().push(b.clone());
    if b["password"] == "wrong" {
        *f.state.lock().unwrap() = "failed".into();
        return (StatusCode::UNAUTHORIZED, Json(json!({ "state": "failed", "error": "invalid credentials" }))).into_response();
    }
    *f.state.lock().unwrap() = "awaiting_2fa".into();
    (StatusCode::ACCEPTED, Json(json!({ "state": "awaiting_2fa", "apple_id": b["username"] }))).into_response()
}

async fn two_fa(AxState(f): AxState<Fake>, Json(b): Json<Value>) -> axum::response::Response {
    f.bodies.lock().unwrap().push(b.clone());
    if b["code"] != "123456" {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "state": "failed", "error": "wrong code" }))).into_response();
    }
    *f.state.lock().unwrap() = "authenticated".into();
    Json(json!({ "state": "authenticated", "apple_id": "me@example.com", "storefront": "us", "music_user_token": "SECRET" })).into_response()
}

async fn logout(AxState(f): AxState<Fake>) -> Json<Value> {
    *f.state.lock().unwrap() = "logged_out".into();
    Json(json!({ "state": "logged_out" }))
}

/// The stand-in wrapper on a free port: its address and what it saw.
async fn fake_wrapper() -> (String, Fake) {
    let f = Fake::default();
    *f.state.lock().unwrap() = "logged_out".into();
    let app = axum::Router::new()
        .route("/me", get(me))
        .route("/health", get(health))
        .route("/login", post(login).merge(delete(logout)))
        .route("/login/2fa", post(two_fa))
        .with_state(f.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, f)
}

#[tokio::test]
async fn the_sign_in_takes_the_code_and_points_gamdl_at_the_wrapper() {
    let (url, f) = fake_wrapper().await;
    let st = TestCtx::new("wrapper-signin");
    // no address yet
    st.cfg.set_many(&json!({ "wrapper_mode": "external" }));
    assert_eq!(sign_in(&st, "me@example.com", "pw").await.unwrap_err().code, Code::Auth);
    st.cfg.set_many(&json!({ "wrapper_url": format!("{url}/") }));
    assert_eq!(super::url(&st).as_deref(), Some(url.as_str()), "the address without its slash");
    // a refused password
    let v = sign_in(&st, "me@example.com", "wrong").await.unwrap();
    assert_eq!((v["state"].as_str(), v["signed_in"].as_bool()), (Some("failed"), Some(false)));
    assert_eq!(v["message"], "Apple refused the sign-in: invalid credentials");
    // Apple wants the code
    let v = sign_in(&st, " me@example.com ", "s3cret").await.unwrap();
    assert_eq!(v["state"], "awaiting_2fa");
    assert_eq!(f.bodies.lock().unwrap().last().unwrap()["username"], "me@example.com", "the Apple ID trimmed");
    // a code that is not six digits never reaches the wrapper
    let sent = f.bodies.lock().unwrap().len();
    let v = sign_in_code(&st, "12345").await.unwrap();
    assert_eq!(v["state"], "failed");
    assert_eq!(f.bodies.lock().unwrap().len(), sent);
    // "123 456" as typed from the phone
    assert_eq!(st.cfg.str("gamdl_wrapper_url"), "");
    let v = sign_in_code(&st, "123 456").await.unwrap();
    assert_eq!(f.bodies.lock().unwrap().last().unwrap()["code"], "123456");
    assert_eq!((v["state"].as_str(), v["signed_in"].as_bool()), (Some("authenticated"), Some(true)));
    assert!(v["message"].as_str().unwrap().contains("gamdl uses the wrapper at"), "{v}");
    assert!(!v.to_string().contains("SECRET"), "no token reaches the page: {v}");
    assert_eq!(st.cfg.str("gamdl_wrapper_url"), url, "gamdl's Wrapper URL filled in");
    assert_eq!(st.cfg.str("gamdl_wrapper_decrypt"), "", "an external wrapper's decrypt socket is on its host, port 10020");
    assert!(gamdl_points_here(&st));
    // Test's verdict on it
    let a = account::me(encore_http::Ctx::http(&st), &url).await.unwrap();
    assert_eq!(a.state, State::Authenticated);
    let line = test_line(&a, "0.0.2", true).unwrap();
    assert!(line.starts_with("Signed in as me@example.com · storefront us · wrapper-v2 0.0.2"), "{line}");
    assert_eq!(account::health(encore_http::Ctx::http(&st), &url).await.unwrap(), "0.0.2");
    // sign out
    sign_out(&st).await.unwrap();
    let a = account::me(encore_http::Ctx::http(&st), &url).await.unwrap();
    assert_eq!(test_line(&a, "0.0.2", true).unwrap_err().code, Code::Auth);
}

#[test]
fn test_refuses_a_wrapper_gamdl_cannot_use() {
    let mut a = Account { state: State::Authenticated, apple_id: None, storefront: None, error: None, version: Some("0.0.1".into()), loader_ok: Some(true) };
    assert_eq!(test_line(&a, "0.0.1", true).unwrap_err().code, Code::Unsupported);
    a.version = Some("0.0.2".into());
    a.loader_ok = Some(false);
    assert!(test_line(&a, "0.0.2", true).unwrap_err().message.contains("did not load"));
    a.loader_ok = Some(true);
    assert!(test_line(&a, "0.0.2", false).unwrap().contains("press Use with gamdl"));
}

#[test]
fn gamdl_is_pointed_at_the_managed_wrapper_with_its_decrypt_socket() {
    let st = TestCtx::new("wrapper-fill");
    assert_eq!(mode(&st), Mode::Managed, "managed by default");
    assert_eq!(fill_gamdl(&st).unwrap_err(), "Start the wrapper first");
    st.cfg.set_many(&json!({ "wrapper_address": "http://encore-wrapper", "wrapper_decrypt_address": "encore-wrapper:10020" }));
    assert!(!gamdl_points_here(&st));
    fill_gamdl(&st).unwrap();
    assert_eq!(st.cfg.str("gamdl_wrapper_url"), "http://encore-wrapper");
    assert_eq!(st.cfg.str("gamdl_wrapper_decrypt"), "encore-wrapper:10020");
    assert!(gamdl_points_here(&st));
    assert_eq!(image(&st), built_image());
    st.cfg.set_many(&json!({ "wrapper_image": " ghcr.io/me/wrapper-v2:1 " }));
    assert_eq!(image(&st), "ghcr.io/me/wrapper-v2:1");
    assert!(built_image().starts_with("encore-wrapper-v2:100e0a864e88"));
    assert!(source_url().ends_with(SOURCE_COMMIT));
}

// ---- the container through a stand-in Docker ----------------------------------------------------------------

/// What the stand-in Docker saw and holds: the calls, the created container, its running state.
#[derive(Clone, Default)]
struct Dock {
    calls: Arc<Mutex<Vec<String>>>,
    created: Arc<Mutex<Option<Value>>>,
    running: Arc<Mutex<bool>>,
    /// a container of the same name someone else made
    foreign: Arc<Mutex<bool>>,
}

async fn dock(AxState(d): AxState<Dock>, req: axum::extract::Request) -> axum::response::Response {
    let (method, uri) = (req.method().to_string(), req.uri().to_string());
    let body = axum::body::to_bytes(req.into_body(), 64 << 20).await.unwrap();
    d.calls.lock().unwrap().push(format!("{method} {}", uri.split('?').next().unwrap()));
    let path: Vec<&str> = uri.split('?').next().unwrap().trim_start_matches('/').split('/').collect();
    let reply = |code: u16, v: Value| (StatusCode::from_u16(code).unwrap(), Json(v)).into_response();
    match (method.as_str(), path.as_slice()) {
        ("GET", ["_ping"]) => "OK".into_response(),
        ("GET", ["images", ..]) => reply(200, json!({ "Id": "sha256:img" })),
        ("POST", ["build"]) => {
            // a tar came, and the build answers line by line, the last one an error
            assert!(body.len() >= 1024, "a context tar");
            assert!(uri.contains("version=1") && uri.contains("TARGET_ARCH"), "{uri}");
            "{\"stream\":\"Step 1/2 : FROM debian\\n\"}\n{\"stream\":\"Step 2/2 : RUN make\\n\"}\n{\"errorDetail\":{\"message\":\"make: no rule\"},\"error\":\"make: no rule\"}\n".into_response()
        }
        ("GET", ["containers", "encore-wrapper", "json"]) => {
            if *d.foreign.lock().unwrap() {
                return reply(200, json!({ "Id": "x", "Config": { "Image": "someone/else", "Labels": {} }, "State": { "Status": "running", "Running": true } }));
            }
            match d.created.lock().unwrap().clone() {
                None => reply(404, json!({ "message": "No such container" })),
                Some(c) => {
                    let running = *d.running.lock().unwrap();
                    reply(200, json!({ "Id": "abc", "Config": { "Image": c["Image"], "Labels": c["Labels"] }, "HostConfig": c["HostConfig"],
                        "State": { "Status": if running { "running" } else { "exited" }, "Running": running, "ExitCode": 0 } }))
                }
            }
        }
        ("GET", ["containers", "encore-wrapper", "logs"]) => {
            let mut frame = vec![1u8, 0, 0, 0, 0, 0, 0, 6];
            frame.extend_from_slice(b"ready\n");
            frame.into_response()
        }
        ("GET", ["containers", _, "json"]) => reply(404, json!({ "message": "No such container" })),
        ("POST", ["containers", "create"]) => {
            *d.created.lock().unwrap() = Some(serde_json::from_slice(&body).unwrap());
            reply(201, json!({ "Id": "abc" }))
        }
        ("POST", ["containers", "encore-wrapper", "start"]) => {
            *d.running.lock().unwrap() = true;
            StatusCode::NO_CONTENT.into_response()
        }
        ("POST", ["containers", "encore-wrapper", "stop"]) => {
            *d.running.lock().unwrap() = false;
            StatusCode::NO_CONTENT.into_response()
        }
        ("DELETE", ["containers", "encore-wrapper"]) => {
            *d.created.lock().unwrap() = None;
            StatusCode::NO_CONTENT.into_response()
        }
        _ => reply(404, json!({ "message": "page not found" })),
    }
}

/// The stand-in Docker on `<short dir>/docker.sock`.
async fn fake_docker(name: &str) -> (PathBuf, Dock) {
    // a unix socket's path must stay under 108 bytes
    let base = if std::env::temp_dir().as_os_str().len() > 40 { PathBuf::from("/tmp") } else { std::env::temp_dir() };
    let dir = base.join(format!("enc-wr-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("docker.sock");
    let listener = tokio::net::UnixListener::bind(&sock).unwrap();
    let d = Dock::default();
    let app = axum::Router::new().fallback(dock).with_state(d.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (sock, d)
}

#[tokio::test]
async fn the_container_is_made_started_stopped_and_removed() {
    let (sock, dk) = fake_docker("run").await;
    let d = sidecar::probe(&sock).await.unwrap();
    // not in a container Docker knows: the ports on the loopback
    let a = sidecar::run(&d, "encore-wrapper-v2:test").await.unwrap();
    assert_eq!(a, sidecar::Address { url: "http://127.0.0.1:30080".into(), decrypt: "127.0.0.1:10020".into() });
    let made = dk.created.lock().unwrap().clone().unwrap();
    assert_eq!(made["HostConfig"]["CapAdd"], json!(["SYS_ADMIN", "SYS_CHROOT", "SYS_PTRACE"]));
    assert_eq!(made["Labels"][sidecar::LABEL], "wrapper");
    assert!(dk.calls.lock().unwrap().contains(&"POST /containers/encore-wrapper/start".to_string()));
    // started again: the same container, no second create
    sidecar::run(&d, "encore-wrapper-v2:test").await.unwrap();
    assert_eq!(dk.calls.lock().unwrap().iter().filter(|c| c.as_str() == "POST /containers/create").count(), 1);
    // a new image: the old container goes, a new one comes
    sidecar::run(&d, "encore-wrapper-v2:newer").await.unwrap();
    assert!(dk.calls.lock().unwrap().contains(&"DELETE /containers/encore-wrapper".to_string()));
    assert_eq!(dk.created.lock().unwrap().as_ref().unwrap()["Image"], "encore-wrapper-v2:newer");
    assert_eq!(d.logs(sidecar::CONTAINER, 10).await.unwrap(), "ready\n", "the log without Docker's frame headers");
    sidecar::stop(&d).await.unwrap();
    assert!(!*dk.running.lock().unwrap());
    sidecar::remove(&d).await.unwrap();
    assert!(dk.created.lock().unwrap().is_none());
    // someone else's container of that name is left alone
    *dk.foreign.lock().unwrap() = true;
    assert!(sidecar::run(&d, "encore-wrapper-v2:test").await.unwrap_err().contains("did not make"));
    assert!(sidecar::remove(&d).await.unwrap_err().contains("leaves it alone"));
    let _ = std::fs::remove_dir_all(sock.parent().unwrap());
}

#[tokio::test]
async fn a_build_streams_its_lines_and_reports_dockers_error() {
    let (sock, _dk) = fake_docker("build").await;
    let d = sidecar::probe(&sock).await.unwrap();
    let dir = tmp("build");
    std::fs::write(dir.join("Dockerfile"), "FROM scratch\n").unwrap();
    let tar = dir.join("ctx.tar");
    stage::write_tar(&dir, &tar).unwrap();
    let mut lines = Vec::new();
    let e = sidecar::build(&d, &tar, "encore-wrapper-v2:t", Arch::X86_64, |l| lines.push(l.to_string())).await.unwrap_err();
    assert_eq!(e, "The image build failed: make: no rule");
    let texts: Vec<String> = lines.iter().filter_map(|l| encore_system::docker::build_text(l)).collect();
    assert_eq!(texts, ["Step 1/2 : FROM debian", "Step 2/2 : RUN make"]);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(sock.parent().unwrap());
}

#[tokio::test]
async fn no_socket_says_how_to_mount_it() {
    let e = sidecar::probe(Path::new("/nonexistent/docker.sock")).await.unwrap_err();
    assert!(e.contains("/var/run/docker.sock:/var/run/docker.sock") && e.contains("A wrapper I run"), "{e}");
}
