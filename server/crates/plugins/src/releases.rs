//! Where new versions come from (`[update]`): GitHub releases (or tags) whose tag fits the manifest's template,
//! PyPI's current version, and the sha256 of a release's asset (a `SHA256SUMS` file in the release, GitHub's
//! own digest of the asset, or a sums file at an https address).
use crate::fetch;
use crate::manifest::{Manifest, Update};
use serde_json::Value;

/// Where the services are (tests point them at stand-ins).
#[derive(Debug, Clone, PartialEq)]
pub struct Hosts {
    /// GitHub's API
    pub github_api: String,
    /// GitHub's site (archive zips of `owner/repo@ref`)
    pub github: String,
    /// PyPI
    pub pypi: String,
    /// Hugging Face
    pub huggingface: String,
}

impl Default for Hosts {
    fn default() -> Self {
        Hosts {
            github_api: "https://api.github.com".into(),
            github: "https://github.com".into(),
            pypi: "https://pypi.org".into(),
            huggingface: "https://huggingface.co".into(),
        }
    }
}

/// A release asset.
#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    /// its file name
    pub name: String,
    /// where to download it
    pub url: String,
    /// GitHub's `digest` ("sha256:<hex>"), when it lists one
    pub digest: Option<String>,
}

/// A release that fits.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    /// its tag
    pub tag: String,
    /// its version, read from the tag (`[update] tag`)
    pub version: String,
    /// its files
    pub assets: Vec<Asset>,
}

impl Release {
    /// The asset named `name`.
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name == name)
    }
}

/// The network side of a check.
#[derive(Clone)]
pub struct Net {
    /// the HTTP client
    pub http: reqwest::Client,
    /// plain http:// is allowed (tests only)
    pub allow_http: bool,
    /// where the services are
    pub hosts: Hosts,
    /// GitHub token (the `github_token` setting): 5,000 checks an hour instead of 60
    pub github_token: String,
}

impl Net {
    /// GET a GitHub API path as JSON (with the token, when set); rate limits and errors as messages.
    async fn github(&self, path: &str) -> anyhow::Result<Value> {
        let mut headers = vec![("Accept", "application/vnd.github+json".to_string()), ("X-GitHub-Api-Version", "2022-11-28".to_string())];
        if !self.github_token.trim().is_empty() {
            headers.push(("Authorization", format!("Bearer {}", self.github_token.trim())));
        }
        let url = format!("{}{path}", self.hosts.github_api.trim_end_matches('/'));
        let (code, v) = fetch::json(&self.http, &url, &headers, self.allow_http).await?;
        if code == 403 || code == 429 {
            anyhow::bail!("GitHub is rate-limiting update checks from this server – try again later, or add a GitHub token under Settings → General");
        }
        if code >= 400 {
            anyhow::bail!("GitHub answered HTTP {code} for {path}: {}", v["message"].as_str().unwrap_or(""));
        }
        Ok(v)
    }
}

/// The assets of a GitHub release.
fn assets_of(r: &Value) -> Vec<Asset> {
    r["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some(Asset {
                        name: x["name"].as_str()?.to_string(),
                        url: x["browser_download_url"].as_str()?.to_string(),
                        digest: x["digest"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// From GitHub's release list: the highest version whose tag fits `[update] tag`, not a draft, a pre-release
/// only with `prerelease`, and with an asset `has_asset` accepts.
pub fn pick(list: &Value, u: &Update, has_asset: &(dyn Fn(&Release) -> bool + Sync)) -> Option<Release> {
    let mut best: Option<Release> = None;
    for r in list.as_array()? {
        if r["draft"].as_bool().unwrap_or(false) || (r["prerelease"].as_bool().unwrap_or(false) && !u.prerelease) {
            continue;
        }
        let Some(tag) = r["tag_name"].as_str() else { continue };
        let Some(version) = u.version_in_tag(tag) else { continue };
        let rel = Release { tag: tag.to_string(), version, assets: assets_of(r) };
        if !has_asset(&rel) {
            continue;
        }
        if best.as_ref().is_none_or(|b| encore_core::version::compare(&rel.version, &b.version) == std::cmp::Ordering::Greater) {
            best = Some(rel);
        }
    }
    best
}

/// The newest release of a manifest's `[update] github` that `has_asset` accepts (with `tags`, the highest tag
/// that fits when no release does).
pub async fn newest(net: &Net, u: &Update, has_asset: &(dyn Fn(&Release) -> bool + Sync)) -> anyhow::Result<Option<Release>> {
    let repo = u.github.as_deref().ok_or_else(|| anyhow::anyhow!("no GitHub repository to check"))?;
    let list = net.github(&format!("/repos/{repo}/releases?per_page=50")).await?;
    if let Some(r) = pick(&list, u, has_asset) {
        return Ok(Some(r));
    }
    if !u.tags {
        return Ok(None);
    }
    let tags = net.github(&format!("/repos/{repo}/tags?per_page=100")).await?;
    let best = tags
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let tag = t["name"].as_str()?;
            Some(Release { tag: tag.to_string(), version: u.version_in_tag(tag)?, assets: vec![] })
        })
        .max_by(|a, b| encore_core::version::compare(&a.version, &b.version));
    Ok(best)
}

/// The release with this tag.
pub async fn by_tag(net: &Net, u: &Update, tag: &str) -> anyhow::Result<Release> {
    let repo = u.github.as_deref().ok_or_else(|| anyhow::anyhow!("no GitHub repository"))?;
    let r = net.github(&format!("/repos/{repo}/releases/tags/{}", urlencode(tag))).await?;
    let version = u.version_in_tag(tag).unwrap_or_else(|| tag.trim_start_matches(['v', 'V']).to_string());
    Ok(Release { tag: tag.to_string(), version, assets: assets_of(&r) })
}

/// PyPI's current version of a project.
pub async fn pypi_latest(net: &Net, project: &str) -> anyhow::Result<String> {
    let url = format!("{}/pypi/{}/json", net.hosts.pypi.trim_end_matches('/'), urlencode(project));
    let (code, v) = fetch::json(&net.http, &url, &[], net.allow_http).await?;
    if code >= 400 {
        anyhow::bail!("PyPI answered HTTP {code} for {project}");
    }
    v["info"]["version"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("PyPI answered without a version of {project}"))
}

/// The sha256 of `asset` in `release` by the rule `sums` ("github", a file name in the release, an https
/// address); None when the rule finds none.
pub async fn sha256_of(net: &Net, release: &Release, asset: &str, sums: &str) -> anyhow::Result<String> {
    if sums == "github" {
        let a = release.asset(asset).ok_or_else(|| anyhow::anyhow!("the {} release has no {asset}", release.tag))?;
        return a
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase)
            .ok_or_else(|| anyhow::anyhow!("GitHub lists no sha256 for {asset} in the {} release", release.tag));
    }
    let url = if sums.starts_with("https://") || sums.starts_with("http://") {
        sums.to_string()
    } else {
        release.asset(sums).map(|a| a.url.clone()).ok_or_else(|| anyhow::anyhow!("the {} release has no {sums}", release.tag))?
    };
    let text = fetch::text(&net.http, &url, 1 << 20, net.allow_http).await?;
    crate::install::sum_for(&text, asset)
}

/// The asset of `release` for this machine's `[[binary]]` (the first of its names the release has).
pub fn binary_asset(m: &Manifest, release: &Release, arch: &str) -> Option<Asset> {
    let b = m.binary_for(arch)?;
    let names = b.asset.as_ref()?;
    names.0.iter().map(|n| crate::manifest::fill(n, &release.version, &release.tag)).find_map(|n| release.asset(&n).cloned())
}

/// The sha256 Hugging Face lists for a file of a model repository (its LFS id): `https://huggingface.co/<owner>/<repo>/resolve/main/<file>`.
pub async fn huggingface_sha256(net: &Net, url: &str) -> Option<String> {
    let rest = url.split("huggingface.co/").nth(1)?;
    let mut parts = rest.splitn(5, '/');
    let (owner, repo, _resolve, rev, file) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let api = format!("{}/api/models/{owner}/{repo}/tree/{rev}", net.hosts.huggingface.trim_end_matches('/'));
    let (code, v) = fetch::json(&net.http, &api, &[], net.allow_http).await.ok()?;
    if code >= 400 {
        return None;
    }
    v.as_array()?.iter().find(|f| f["path"].as_str() == Some(file))?["lfs"]["oid"].as_str().map(str::to_ascii_lowercase)
}

/// Percent-encode everything but unreserved characters (RFC 3986).
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}
