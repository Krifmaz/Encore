package app.encore.android.features.settings

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Download folders of Diagnostics (servers since 0.33), as the web's `folder_lines`. */
class DownloadFoldersTest {
    private val d = EncoreJson.parseToJsonElement("""{"version": "0.33.0", "uid": 1000, "checks": [], "download_folders": {"ok": false, "roots": [
        {"key": "staging_path", "label": "Staging folder (yt-dlp, streamrip, links)", "path": "/downloads/encore", "state": "read_only", "result": "Read-only", "ok": false,
         "text": "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount", "checked_at": 20, "since": 10},
        {"key": "qbit_download_path", "label": "Torrent downloads (qBittorrent)", "path": "/downloads/torrents", "state": "denied", "result": "Permission denied", "ok": true,
         "note": "Encore may not write here, which is fine", "checked_at": 20, "since": null},
        {"key": "sab_download_path", "label": "Usenet downloads (SABnzbd)", "path": "/downloads/usenet", "state": "unknown", "result": "Not checked yet", "ok": true, "checked_at": null}]}}""") as JsonObject
    private val date = { t: Double -> "t${t.toLong()}" }

    @Test
    fun eachFolderWithItsResultAndTime() {
        val f = DownloadFolderRules.folders(d)
        assertEquals(3, f.size)
        assertEquals(Triple("Staging folder (yt-dlp, streamrip, links) – /downloads/encore",
            "The downloads folder /downloads/encore is read-only – check the disk (dmesg, fsck) or the mount", "Checked t20 · not writable since t10"),
            DownloadFolderRules.folderLines(f[0], date))
        assertEquals("Permission denied – Encore may not write here, which is fine", DownloadFolderRules.folderLines(f[1], date).second)
        assertEquals("Checked t20", DownloadFolderRules.folderLines(f[1], date).third)
        assertEquals("Not checked yet", DownloadFolderRules.folderLines(f[2], date).third)
        assertEquals(listOf("err", "warn", "ok"), f.map { DownloadFolderRules.kind(it) })
        assertEquals(emptyList<JsonObject>(), DownloadFolderRules.folders(JsonObject(emptyMap())))
        // on the page the result is said once (the badge); no time before the first probe
        assertEquals(Pair("Permission denied – Encore may not write here, which is fine", "Checked t20"), DownloadFolderRules.shownLines(f[1], date))
        assertEquals(Pair<String?, String?>(null, null), DownloadFolderRules.shownLines(f[2], date))
        val fine = EncoreJson.parseToJsonElement("""{"state": "ok", "result": "Writable", "ok": true, "checked_at": 20}""") as JsonObject
        assertEquals(Pair<String?, String?>(null, "Checked t20"), DownloadFolderRules.shownLines(fine, date))
    }

    @Test
    fun theReportListsTheFolders() {
        val text = SettingsRules.diagnosticsReport(d, date) { b -> "$b" }
        assertTrue(text, text.contains("Download folders:\nFAIL Staging folder (yt-dlp, streamrip, links) – /downloads/encore\n     The downloads folder /downloads/encore is read-only"))
        assertTrue(text, text.contains("OK   Torrent downloads (qBittorrent) – /downloads/torrents\n     Permission denied – Encore may not write here, which is fine (Checked t20)"))
    }
}
