//! Upkeep of the request queue, run by the scheduler: lossless upgrades of albums only held in lossy
//! formats, and the clean-up of old requests, leftover download folders and the trash.
use crate::Ctx;
use encore_db::{now, p, RowExt};

/// Clear finished requests older than the cleanup age, leftover staging folders and the trash. → what was done
pub fn cleanup(st: &impl Ctx) -> anyhow::Result<String> {
    let days = st.cfg().int("task_cleanup_days").max(1);
    let cutoff = now() - days as f64 * 86400.0;
    let n = st.db().one("SELECT COUNT(*) n FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", p![cutoff])?.map(|r| r.i("n")).unwrap_or(0);
    st.db().execute("DELETE FROM requests WHERE status IN ('completed','denied','cancelled') AND updated_at < ?", p![cutoff])?;
    // a request's staging folder stays while it runs or waits, and while it keeps downloaded files for Import
    // again (a storage problem, a failed import, a link's files that could not be imported): those are never
    // cleared here
    let active: std::collections::HashSet<String> = st.db().query("SELECT id FROM requests WHERE status IN ('queued','searching','downloading','monitoring','importing','waiting')
        OR (status IN ('failed','completed') AND json_extract(detail, '$.kept') IS NOT NULL)", p![])?
        .iter().map(|r| format!("request-{}", r.i("id"))).collect();
    let mut removed = 0;
    if let Ok(rd) = std::fs::read_dir(st.cfg().str("staging_path")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| m.elapsed().ok()).is_some_and(|d| d.as_secs() > 3600);
            if name.starts_with("request-") && !active.contains(&name) && old {
                let _ = std::fs::remove_dir_all(e.path());
                removed += 1;
            }
        }
    }
    let trashed = crate::report::purge_trash(st);
    // uploads for "Replace with my own file" that were staged and never used (a day old)
    encore_library::own_file::purge_staged(st);
    Ok(format!("{removed} leftover download folder(s) removed{}{}", if n > 0 { format!(", {n} old request(s) cleared") } else { String::new() },
        if trashed > 0 { format!(", {trashed} reported file(s) deleted from the trash") } else { String::new() }))
}

/// Re-request MusicBrainz-tagged albums that are only lossy (lossless sources only).
pub fn queue_lossy_upgrades(st: &impl Ctx, limit: usize) -> anyhow::Result<usize> {
    let days = st.cfg().int("upgrade_lossy_days").max(1) as f64;
    let rows = st.db().query("SELECT mb_releasegroupid rg, MAX(albumartist) artist, MAX(album) album, MAX(year) year FROM tracks
        WHERE mb_releasegroupid IS NOT NULL AND mb_releasegroupid != '' GROUP BY album_id
        HAVING SUM(CASE WHEN format IN ('flac','wav','alac','aiff','ape','wv') THEN 1 ELSE 0 END) = 0", p![])?;
    let t = now();
    let mut n = 0;
    for a in rows {
        if n >= limit {
            break;
        }
        let rg = a.s("rg");
        let scope = format!("upg:{rg}");
        if let Some(last) = st.db().one("SELECT value FROM sync_state WHERE connection_id=0 AND scope=?", p![scope])? {
            if t - last.s("value").parse::<f64>().unwrap_or(0.0) < days * 86400.0 {
                continue;
            }
        }
        if st.db().one("SELECT 1 FROM requests WHERE mbid=? AND status IN ('queued','searching','downloading','monitoring','importing','waiting','pending')", p![rg])?.is_some() {
            continue;
        }
        st.db().execute("INSERT OR REPLACE INTO sync_state VALUES (0,?,?)", p![scope, t.to_string()])?;
        st.db().execute("INSERT INTO requests (user_id, kind, mbid, artist, title, album, year, cover_url, status, message, detail, created_at, updated_at)
                       VALUES (NULL,'album',?,?,?,?,?,?,'queued',?,?,?,?)",
            p![rg, a.s("artist"), a.s("album"), a.s("album"), a.s("year"), encore_http::mb::cover_rg(&rg, 250), "Lossless upgrade", r#"{"lossless_only": true}"#, t, t])?;
        n += 1;
    }
    if n > 0 {
        st.db().log("info", &format!("Queued {n} album(s) for a lossless upgrade"));
    }
    Ok(n)
}
