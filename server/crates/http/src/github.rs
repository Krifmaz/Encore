//! GitHub's REST API: release checks of Encore itself and of its tools (Cadenza, slskd, streamrip, llama.cpp).
use crate::Ctx;
use serde_json::{json, Value};
use std::time::Duration;

/// GitHub's API with a conditional request. The ETag and body of the last answer for a URL are kept (a week,
/// in the small cache) and sent back as `If-None-Match`: a 304 answer gives the kept body as a 200, and GitHub
/// does not count 304s against the rate limit (60 requests an hour per address without a token, 5,000 with
/// the `github_token` setting). A rate-limited answer (403 or 429 with X-RateLimit-Remaining: 0) becomes a
/// sentence with the reset time instead of GitHub's text.
pub async fn get(st: &impl Ctx, url: &str) -> anyhow::Result<(u16, Value)> {
    let key = format!("gh:etag:{url}");
    let kept = st.cache_get(&key);
    let token = st.cfg().str("github_token");
    let token = token.trim();
    let mut req = st.http().get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(Duration::from_secs(20));
    if !token.is_empty() {
        req = req.bearer_auth(token);
    }
    if let Some(etag) = kept.as_ref().and_then(|k| k["etag"].as_str()) {
        req = req.header("If-None-Match", etag);
    }
    let r = req.send().await.map_err(|e| anyhow::anyhow!("GitHub unreachable: {e}"))?;
    let code = r.status().as_u16();
    if code == 304 {
        if let Some(k) = kept {
            return Ok((200, k["body"].clone()));
        }
    }
    let header = |name: &str| r.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string());
    let remaining = header("x-ratelimit-remaining").and_then(|v| v.parse::<i64>().ok());
    let reset = header("x-ratelimit-reset").and_then(|v| v.parse::<i64>().ok());
    let etag = header("etag");
    let v = r.json::<Value>().await.unwrap_or(Value::Null);
    if (code == 403 || code == 429) && remaining == Some(0) {
        anyhow::bail!("{}", rate_limit_message(reset, !token.is_empty(), encore_db::now() as i64));
    }
    if code < 400 {
        if let Some(etag) = etag {
            st.cache_put(&key, json!({ "etag": etag, "body": v }), 7 * 86400);
        }
    }
    Ok((code, v))
}

/// What a rate-limited check says: when it clears (from X-RateLimit-Reset, epoch seconds) and the way out.
pub fn rate_limit_message(reset_epoch: Option<i64>, has_token: bool, now: i64) -> String {
    let when = match reset_epoch {
        Some(t) if t > now => {
            let mins = ((t - now) + 59) / 60;
            let s = t.rem_euclid(86400);
            format!(" until about {:02}:{:02} UTC (in {} min)", s / 3600, (s % 3600) / 60, mins.max(1))
        }
        _ => String::new(),
    };
    if has_token {
        format!("GitHub is rate-limiting update checks for the GitHub token{when} – try again then")
    } else {
        format!("GitHub is rate-limiting update checks from this server's address (60 an hour without a token){when} – try again then, or add a GitHub token under Settings → Apps & sources → General, which allows 5,000 an hour")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_limited_check_says_when_it_clears_and_what_helps() {
        // 10:00:00 UTC now, reset at 10:42:30
        let now = 10 * 3600;
        let m = rate_limit_message(Some(now + 42 * 60 + 30), false, now);
        assert!(m.starts_with("GitHub is rate-limiting update checks from this server's address (60 an hour without a token) until about 10:42 UTC (in 43 min)"), "{m}");
        assert!(m.contains("add a GitHub token under Settings"), "{m}");
        let m = rate_limit_message(None, true, now);
        assert_eq!(m, "GitHub is rate-limiting update checks for the GitHub token – try again then");
        // a reset in the past says nothing about a time
        assert!(!rate_limit_message(Some(now - 5), false, now).contains("until"));
    }
}
