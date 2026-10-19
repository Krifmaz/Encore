//! encore_http::github::get through a server's state: the ETag of the last answer, GitHub's rate limit in
//! one sentence.

/// A second check of the same URL sends If-None-Match and takes the 304 as the kept body; a rate-limited
/// answer is one sentence, not GitHub's.
#[tokio::test]
async fn github_checks_reuse_the_last_answer_through_etags() {
    use axum::{extract::State as S, http::HeaderMap, response::IntoResponse, routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering as O};
    use std::sync::Arc;
    #[derive(Clone)]
    struct Hits { full: Arc<AtomicUsize>, conditional: Arc<AtomicUsize> }
    async fn latest(S(h): S<Hits>, headers: HeaderMap) -> axum::response::Response {
        if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"abc\"") {
            h.conditional.fetch_add(1, O::SeqCst);
            return (axum::http::StatusCode::NOT_MODIFIED, [("etag", "\"abc\"")]).into_response();
        }
        h.full.fetch_add(1, O::SeqCst);
        (axum::http::StatusCode::OK, [("etag", "\"abc\""), ("content-type", "application/json")], r#"{"tag_name":"v1.2.3"}"#).into_response()
    }
    async fn limited() -> axum::response::Response {
        (axum::http::StatusCode::FORBIDDEN, [("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "4102444800"), ("content-type", "application/json")],
            r#"{"message":"API rate limit exceeded for 1.2.3.4. (But here's the good news: ...)"}"#).into_response()
    }
    let hits = Hits { full: Arc::new(AtomicUsize::new(0)), conditional: Arc::new(AtomicUsize::new(0)) };
    let app = Router::new().route("/repos/x/y/releases/latest", get(latest)).route("/repos/x/z/releases/latest", get(limited)).with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("encore-ghcache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let st = crate::test_support::state_in(&dir);
    let url = format!("{base}/repos/x/y/releases/latest");
    let (code, v) = encore_http::github::get(&st, &url).await.unwrap();
    assert_eq!((code, v["tag_name"].as_str()), (200, Some("v1.2.3")));
    let (code, v) = encore_http::github::get(&st, &url).await.unwrap();
    assert_eq!((code, v["tag_name"].as_str()), (200, Some("v1.2.3")), "the 304 answer is served from the kept body");
    assert_eq!((hits.full.load(O::SeqCst), hits.conditional.load(O::SeqCst)), (1, 1));
    let e = encore_http::github::get(&st, &format!("{base}/repos/x/z/releases/latest")).await.unwrap_err().to_string();
    // (the token wording depends on whether GITHUB_TOKEN is set where the tests run)
    assert!(e.starts_with("GitHub is rate-limiting update checks"), "{e}");
    assert!(!e.contains("good news"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
