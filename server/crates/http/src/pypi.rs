//! PyPI: the newest release of a Python package (the yt-dlp update check).
use crate::Ctx;
use serde_json::Value;
use std::time::Duration;

pub const API: &str = "https://pypi.org/pypi";

/// The newest version of `package` on PyPI: the answer's HTTP status, and the version when the answer
/// names one.
pub async fn latest(st: &impl Ctx, package: &str) -> anyhow::Result<(u16, Option<String>)> {
    let r = st.http().get(format!("{API}/{package}/json")).timeout(Duration::from_secs(20)).send().await?;
    let code = r.status().as_u16();
    let v = r.json::<Value>().await.unwrap_or(Value::Null);
    Ok((code, version(&v)))
}

/// The version in PyPI's JSON answer for a project.
pub fn version(v: &Value) -> Option<String> {
    v["info"]["version"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn reads_the_version() {
        let pypi = json!({ "info": { "version": "2026.10.14", "name": "yt-dlp" }, "releases": {} });
        assert_eq!(super::version(&pypi).as_deref(), Some("2026.10.14"));
        assert_eq!(super::version(&json!({ "message": "Not Found" })), None);
    }
}
