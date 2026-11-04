//! Covers through the real router: cache headers with and without `v`, ETag 304s, size buckets,
//! WebP vs JPEG by Accept, `cover_v` in the JSON, and a new `cover_v` after an artwork override.
use encore_db::p;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::json;
use std::path::PathBuf;
use tower::ServiceExt;
use crate::test_support::{send, Resp};

const PW: &str = "cover-pass-1";

struct Env {
    st: AppState,
    app: Router,
    dir: PathBuf,
    cookie: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(name: &str) -> Env {
    let dir = std::env::temp_dir().join(format!("encore-covers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    crate::auth::create_user(&st, "coverer", PW, "admin", None, None).unwrap();
    let app = crate::app(st.clone());
    let req = Request::builder().method("POST").uri("/api/auth/login").header("x-requested-with", "encore").header("content-type", "application/json")
        .body(Body::from(json!({ "username": "coverer", "password": PW }).to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let cookie = resp.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap().to_string();
    Env { st, app, dir, cookie }
}

impl Env {
    async fn get(&self, uri: &str, headers: &[(&str, &str)]) -> Resp {
        send(&self.app, "GET", uri, &self.cookie, None, headers).await
    }
    /// An album of one song with a real 1500×1500 cover.jpg next to it.
    fn album(&self, title: &str) -> String {
        let aid = encore_library::album_key("Artist", title, "");
        let folder = self.dir.join("music").join(&aid);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("1.mp3");
        std::fs::write(&path, b"ID3 not really audio").unwrap();
        self.st.db.execute(
            "INSERT INTO tracks (path, title, artist, album, albumartist, album_id, format, bitrate, duration, size, year, disc, track, added_at)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            p![path.to_string_lossy(), "Song", "Artist", title, "Artist", aid, "mp3", 320, 180.0, 20, "2020", 1, 1, 1.0]).unwrap();
        self.st.library.artists.invalidate();
        let img = image::RgbImage::from_fn(1500, 1500, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
        img.save(folder.join("cover.jpg")).unwrap();
        aid
    }
}

#[tokio::test]
async fn cover_headers_formats_and_versions() {
    let e = setup("headers").await;
    let aid = e.album("Blue");

    // the album JSON carries cover_v (album page: looked up on the spot), and so do its songs
    let a = e.get(&format!("/api/library/albums/{aid}"), &[]).await.json();
    let v = a["cover_v"].as_u64().expect("album has cover_v");
    assert!(v > 0 && v < (1u64 << 53));
    assert_eq!(a["tracks"][0]["cover_v"].as_u64(), Some(v));
    // lists: the albums list too
    let l = e.get("/api/library/albums?limit=10", &[]).await.json();
    let row = l["items"].as_array().cloned().unwrap_or_default();
    assert!(row.iter().any(|r| r["cover_v"].as_u64() == Some(v)), "{l}");

    // without v: cached a day, revalidated
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200"), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.h("cache-control"), "private, max-age=86400, stale-while-revalidate=604800");
    assert_eq!(r.h("content-type"), "image/jpeg");
    assert_eq!(r.h("vary"), "Accept");
    let etag = r.h("etag");
    assert!(!etag.is_empty());
    assert!(!r.h("last-modified").is_empty());
    let img = image::load_from_memory(&r.body).unwrap();
    assert_eq!((img.width(), img.height()), (200, 200));

    // with the current v: immutable
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200&v={v}"), &[]).await;
    assert_eq!(r.h("cache-control"), "private, max-age=31536000, immutable");
    // an old v: not immutable (the image under it is the current one)
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200&v=12345"), &[]).await;
    assert_eq!(r.h("cache-control"), "private, max-age=86400, stale-while-revalidate=604800");

    // ETag → 304
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200"), &[("if-none-match", &etag)]).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert!(r.body.is_empty());
    assert_eq!(r.h("etag"), etag);
    // Last-Modified → 304 too
    let lm = e.get(&format!("/api/library/albums/{aid}/cover?s=200"), &[]).await.h("last-modified");
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200"), &[("if-modified-since", &lm)]).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);

    // WebP when the client takes it, JPEG otherwise; separate ETags
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=400"), &[("accept", "image/avif,image/webp,*/*")]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.h("content-type"), "image/webp");
    assert_eq!(&r.body[..4], b"RIFF");
    assert_eq!(&r.body[8..12], b"WEBP");
    let img = image::load_from_memory(&r.body).unwrap();
    assert_eq!((img.width(), img.height()), (400, 400));
    let wtag = r.h("etag");
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=400"), &[("accept", "image/jpeg")]).await;
    assert_eq!(r.h("content-type"), "image/jpeg");
    assert_ne!(r.h("etag"), wtag);

    // sizes snap to buckets: 150 → 200, 5000 → 1000; the bucket file is reused
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=150"), &[]).await;
    assert_eq!(image::load_from_memory(&r.body).unwrap().width(), 200);
    assert_eq!(r.h("etag"), etag);
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=5000"), &[]).await;
    assert_eq!(image::load_from_memory(&r.body).unwrap().width(), 1000);
    let thumbs: Vec<String> = std::fs::read_dir(e.st.data_dir().join("covers").join("thumbs")).unwrap().flatten()
        .map(|f| f.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with(&aid)).collect();
    assert!(thumbs.iter().all(|n| n.ends_with(".jpg") || n.ends_with(".webp")), "{thumbs:?}");
    assert!(!thumbs.iter().any(|n| n.contains("_150_") || n.contains("_5000_")), "{thumbs:?}");

    // the original: no Vary, still versioned caching
    let r = e.get(&format!("/api/library/albums/{aid}/cover?v={v}"), &[]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.h("cache-control"), "private, max-age=31536000, immutable");
    assert_eq!(image::load_from_memory(&r.body).unwrap().width(), 1500);
}

#[tokio::test]
async fn artwork_override_changes_cover_v() {
    let e = setup("override").await;
    let aid = e.album("Red");
    let v1 = e.get(&format!("/api/library/albums/{aid}"), &[]).await.json()["cover_v"].as_u64().unwrap();
    // warm the thumbnail under v1
    assert_eq!(e.get(&format!("/api/library/albums/{aid}/cover?s=200&v={v1}"), &[]).await.status, StatusCode::OK);

    // the folder picture replaced by a new one (what Save cover / an override does), then forgotten
    std::thread::sleep(std::time::Duration::from_millis(20));
    let folder = e.dir.join("music").join(&aid);
    image::RgbImage::from_pixel(800, 800, image::Rgb([200, 10, 10])).save(folder.join("cover.jpg")).unwrap();
    e.st.library.forget_cover(Some(&aid));

    let v2 = e.get(&format!("/api/library/albums/{aid}"), &[]).await.json()["cover_v"].as_u64().unwrap();
    assert_ne!(v1, v2);
    let r = e.get(&format!("/api/library/albums/{aid}/cover?s=200&v={v2}"), &[]).await;
    assert_eq!(r.h("cache-control"), "private, max-age=31536000, immutable");
    let px = image::load_from_memory(&r.body).unwrap().to_rgb8();
    let c = px.get_pixel(100, 100);
    assert!(c[0] > 150 && c[1] < 60, "new picture served: {c:?}");
    // the old version's thumbnails are gone
    assert!(!encore_media::covers::thumb_path(&e.st.db, &aid, 200, v1, false).exists());

    // a scan/import marks covers stale; lists keep the known version meanwhile
    e.st.library.forget_cover(None);
    let l = e.get("/api/library/albums?limit=10", &[]).await.json();
    assert!(l.to_string().contains(&format!("\"cover_v\":{v2}")), "{l}");
}

#[tokio::test]
async fn warm_pass_makes_grid_thumbnails() {
    let e = setup("warm").await;
    let aid = e.album("Green");
    e.st.library.warm_covers(e.st.db.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let v = loop {
        if let Some(Some(v)) = e.st.library.covers.known(&aid) {
            if [200, 400].iter().all(|s| [true, false].iter().all(|w| encore_media::covers::thumb_path(&e.st.db, &aid, *s, v, *w).is_file())) {
                break v;
            }
        }
        assert!(std::time::Instant::now() < deadline, "warm pass did not finish");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(v > 0);
}

#[tokio::test]
async fn warm_pass_finds_cover_colours_and_json_carries_them() {
    let e = setup("color").await;
    let aid = e.album("Orange");
    // a flat orange cover
    image::RgbImage::from_pixel(600, 600, image::Rgb([240, 140, 20])).save(e.dir.join("music").join(&aid).join("cover.jpg")).unwrap();
    e.st.library.forget_cover(Some(&aid));
    e.st.library.warm_covers(e.st.db.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let v = loop {
        if let Some(Some(v)) = e.st.library.covers.known(&aid) {
            if e.st.library.covers.color(&e.st.db, &aid, v).is_some() {
                break v;
            }
        }
        assert!(std::time::Instant::now() < deadline, "warm pass found no colour");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let c = e.st.library.covers.color(&e.st.db, &aid, v).unwrap();
    let rgb = u32::from_str_radix(&c[1..], 16).unwrap();
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    assert!(r > 200 && (100..180).contains(&g) && b < 70, "orange: {c}");
    // stored, for the next start
    let row = e.st.db.one("SELECT v, color FROM cover_color WHERE album_id=?", p![aid]).unwrap().unwrap();
    use encore_db::RowExt;
    assert_eq!((row.i("v") as u64, row.s("color")), (v, c.clone()));
    // the album, its songs and the albums list carry it next to cover_v
    let a = e.get(&format!("/api/library/albums/{aid}"), &[]).await.json();
    assert_eq!(a["cover_color"].as_str(), Some(c.as_str()), "{a}");
    assert_eq!(a["tracks"][0]["cover_color"].as_str(), Some(c.as_str()));
    let l = e.get("/api/library/albums?limit=10", &[]).await.json();
    assert!(l.to_string().contains(&format!("\"cover_color\":\"{c}\"")), "{l}");
    // the pass is done and says so (the album is counted after its colour is stored: give it a moment)
    let (done, total) = loop {
        let (done, total, _) = e.st.library.covers.progress();
        if done >= 1 {
            break (done, total);
        }
        assert!(std::time::Instant::now() < deadline, "the pass stored the colour but never counted the album: {done}/{total}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(total >= 1 && done >= 1, "{done}/{total}");
    // a new cover: the old colour is not used for it
    std::thread::sleep(std::time::Duration::from_millis(20));
    image::RgbImage::from_pixel(600, 600, image::Rgb([20, 40, 200])).save(e.dir.join("music").join(&aid).join("cover.jpg")).unwrap();
    e.st.library.forget_cover(Some(&aid));
    let a = e.get(&format!("/api/library/albums/{aid}"), &[]).await.json();
    assert_ne!(a["cover_v"].as_u64(), Some(v));
    assert_ne!(a["cover_color"].as_str(), Some(c.as_str()), "{a}");
}

/// After a scan or an import only the albums whose songs changed are looked at again: their new cover gets
/// its thumbnails and colour, the other albums' covers are not touched, and an album left without songs loses
/// its thumbnails and colour. Diagnostics' numbers stay those of the whole pass.
#[tokio::test]
async fn a_refresh_looks_only_at_the_changed_albums() {
    let e = setup("refresh").await;
    let (a, b) = (e.album("Changed"), e.album("Untouched"));
    let thumbs = |aid: &str, v: u64| [200, 400].iter().all(|s| [true, false].iter().all(|w| encore_media::covers::thumb_path(&e.st.db, aid, *s, v, *w).is_file()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let wait = |what: &str, ok: &dyn Fn() -> bool| {
        while !ok() {
            assert!(std::time::Instant::now() < deadline, "{what}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    };
    e.st.library.warm_covers(e.st.db.clone());
    let known = |aid: &str| e.st.library.covers.known(aid).flatten();
    wait("the whole pass", &|| [&a, &b].iter().all(|x| known(x).is_some_and(|v| thumbs(x, v) && e.st.library.covers.color(&e.st.db, x, v).is_some())));
    wait("the whole pass ends", &|| !e.st.library.covers.progress().2);
    let (va, vb) = (known(&a).unwrap(), known(&b).unwrap());
    let progress = e.st.library.covers.progress();
    assert_eq!((progress.0, progress.1), (2, 2));

    // both pictures change on disk; only A's songs changed
    std::thread::sleep(std::time::Duration::from_millis(20));
    for aid in [&a, &b] {
        image::RgbImage::from_pixel(700, 700, image::Rgb([10, 200, 30])).save(e.dir.join("music").join(aid).join("cover.jpg")).unwrap();
    }
    e.st.library.refresh_covers(e.st.db.clone(), vec![a.clone()]);
    wait("A looked at again", &|| known(&a).is_some_and(|v| v != va && thumbs(&a, v) && e.st.library.covers.color(&e.st.db, &a, v).is_some()));
    wait("the pass ends", &|| !e.st.library.covers.progress().2);
    assert!(!encore_media::covers::thumb_path(&e.st.db, &a, 200, va, true).exists(), "A's old thumbnails went");
    assert_eq!(known(&b), Some(vb), "B was not looked at");
    assert!(thumbs(&b, vb), "B's thumbnails stay");
    let progress = e.st.library.covers.progress();
    assert_eq!((progress.0, progress.1), (2, 2), "Diagnostics keeps the whole pass's numbers");

    // A's songs leave the library: its thumbnails and colour go, B keeps its own
    let va2 = known(&a).unwrap();
    e.st.db.execute("DELETE FROM tracks WHERE album_id=?", p![a]).unwrap();
    e.st.library.refresh_covers(e.st.db.clone(), vec![a.clone()]);
    wait("A's thumbnails removed", &|| !thumbs(&a, va2) && !encore_media::covers::thumb_path(&e.st.db, &a, 200, va2, false).exists());
    wait("the pass ends", &|| !e.st.library.covers.progress().2);
    assert!(e.st.db.one("SELECT 1 FROM cover_color WHERE album_id=?", p![a]).unwrap().is_none(), "A's colour went");
    assert!(e.st.db.one("SELECT 1 FROM cover_color WHERE album_id=?", p![b]).unwrap().is_some());
    assert!(thumbs(&b, vb));
}
