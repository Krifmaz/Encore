//! Finished downloads in Encore's category that Encore didn't start (added by hand in qBittorrent or
//! SABnzbd, or by another app with Encore's category): every two minutes each new one becomes a request of
//! its own ("Import downloads Encore didn't start", on by default). The request imports a copy of the files by
//! their own tags, as a link's files are imported; the download stays in its client as it is (it may still
//! seed), and is never removed from it ([`torznab::KEEP`]).
use super::*;
use crate::sources::dl::{Client, Outside};

/// How often the clients are asked (seconds).
const EVERY: u64 = 120;

/// Ask the clients now and then: started and not waited for.
pub(super) fn tick<C: Ctx>(st: &C) {
    if !st.cfg().bool("import_client_downloads") || st.cache_get("outside_scan").is_some() {
        return;
    }
    st.cache_put("outside_scan", json!(true), EVERY);
    let st = st.clone();
    tokio::spawn(async move { scan(&st).await });
}

async fn scan(st: &impl Ctx) {
    for c in [Client::torrent(st), Client::usenet(st)] {
        if !(st.plugins().enabled(c.plugin_id()) && c.ready(st)) {
            continue;
        }
        match c.finished_outside(st).await {
            Ok(list) => {
                for o in list {
                    if let Err(e) = adopt(st, c, &o).await {
                        tracing::warn!("{}: couldn't import {}: {e:#}", c.name(), o.name);
                    }
                }
            }
            Err(e) => tracing::debug!("{}: no look at its finished downloads: {e:#}", c.name()),
        }
    }
}

/// The detail of the request that imports a download Encore didn't start: its one "release" is the download
/// itself, already handed to its client.
fn detail(c: Client, o: &Outside, id: &str, t: f64) -> Value {
    let proto = if matches!(c, Client::Sab | Client::NzbGet | Client::NzbHole) { "usenet" } else { "torrent" };
    json!({ "sources": [], "torznab": {
        "id": id, "outside": true, "idx": 0, "started": t, "last_change": t, "last_progress": 0,
        "candidates": [{ "title": o.name, "indexer": format!("{} (added outside Encore)", c.name()), "protocol": proto, "url": "" }],
    } })
}

/// One download Encore didn't start: a request that imports it, unless one did already. A download Encore
/// can't see where its client says it is is left alone (the next look tries again).
async fn adopt(st: &impl Ctx, c: Client, o: &Outside) -> anyhow::Result<()> {
    let id = format!("{}{}", torznab::KEEP, o.id);
    if st.db().one("SELECT id FROM requests WHERE json_extract(detail, '$.torznab.id')=? LIMIT 1", p![id])?.is_some() {
        return Ok(());
    }
    if !o.path.exists() {
        // said in the log once a day per download (the next look tries again): mount the client's folder at
        // the same path, or correct the download folder on the client's card
        let said = format!("outside_unseen:{}", o.id);
        if st.cache_get(&said).is_none() {
            st.cache_put(&said, json!(true), 86_400);
            st.db().log("warning", &format!("{}: {} finished at {}, which Encore can't see – mount the client's download folder in Encore's container \
                at the same path, or correct the download folder on the {} card under Settings → Plugins", c.name(), o.name, o.path.display(), c.name()));
        }
        return Ok(());
    }
    let Some(admin) = st.db().one("SELECT id FROM users WHERE role='admin' ORDER BY created_at, id LIMIT 1", p![])? else { return Ok(()) };
    let t = now();
    let msg = format!("Found finished in {} – importing", c.name());
    let rid = st.db().execute("INSERT INTO requests (user_id, kind, title, status, source, message, detail, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?)",
        p![admin.i("id"), "url", o.name, "downloading", "torznab", msg, detail(c, o, &id, t).to_string(), t, t])?;
    st.db().log("info", &format!("{}: importing {} (added outside Encore) as request #{rid}", c.name(), o.name));
    let own = o.id.split_once(':').map(|(_, x)| x).unwrap_or(&o.id);
    c.mark_imported(st, own).await;
    crate::events::emit(st, rid);
    st.requests().wake.notify_one();
    Ok(())
}

/// A finished download Encore didn't start: its audio files are copied into the request's staging folder
/// (the client's files are never changed) and imported from there by their own tags.
pub(super) async fn import_copy(st: &impl Ctx, r: &mut Req, files: &[PathBuf], path: &Path, via: &str) -> anyhow::Result<()> {
    let dir = staging(st, r);
    let base = if path.is_file() { path.parent().unwrap_or(path).to_path_buf() } else { path.to_path_buf() };
    let (from, to) = (files.to_vec(), dir.clone());
    let copies = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<PathBuf>> {
        let mut out = vec![];
        for f in &from {
            let rel = f.strip_prefix(&base).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(f.file_name().unwrap_or_default()));
            let dest = to.join(rel);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::copy(f, &dest)?;
            out.push(dest);
        }
        Ok(out)
    }).await??;
    let name = r.d["torznab"]["candidates"][0]["title"].as_str().unwrap_or("").to_string();
    let origin = json!({ "source": via, "detail": format!("added outside Encore: {name}") });
    let mut spec = storage::Spec::new("torznab", true, origin, format!("Imported from {via}"));
    spec.how = "url";
    match storage::import_checked(st, r, &copies, &dir, spec).await {
        Ok(_) => Ok(()),
        Err(e) if e.is::<Rejected>() => {
            rm(&dir);
            fail(st, r, &format!("{via}: {e}")).await;
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_holds_the_download_as_its_only_release() {
        let o = Outside { id: "qbit:abc".into(), name: "Artist - Album (2020) [FLAC]".into(), path: PathBuf::from("/downloads/x") };
        let d = detail(Client::Qbit, &o, "keep:qbit:abc", 5.0);
        assert_eq!(d["torznab"]["id"], "keep:qbit:abc");
        assert_eq!(d["torznab"]["idx"], 0);
        assert_eq!(d["torznab"]["candidates"][0]["protocol"], "torrent");
        assert_eq!(d["torznab"]["candidates"][0]["title"], "Artist - Album (2020) [FLAC]");
        assert_eq!(d["sources"], json!([]));
        assert_eq!(detail(Client::Sab, &o, "keep:sab:abc", 5.0)["torznab"]["candidates"][0]["protocol"], "usenet");
    }
}
