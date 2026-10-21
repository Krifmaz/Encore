package app.encore.android.features.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.core.arr
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.serialization.json.JsonObject

/**
 * The download folders in Settings → Diagnostics (diagnostics.rs `download_folders_html`, servers since 0.33, backlog
 * item 17): each folder downloads are saved in – the staging folder, Soulseek's, the torrent and Usenet clients' and the
 * blackhole folders – with the result and the time of its probe (`download_folders` of GET /api/system/diagnostics).
 */
object DownloadFolderRules {
    /** The folders of a diagnostics answer (none from an older server). */
    fun folders(d: JsonObject): List<JsonObject> = d.obj("download_folders")?.arr("roots").objectsOf()

    /**
     * A folder in words, as the web's `folder_lines`: (what it is, the result with a note or the banner's sentence, when
     * it was probed). [date] formats a unix time.
     */
    fun folderLines(r: JsonObject, date: (Double) -> String): Triple<String, String, String> {
        val what = "${r.str("label")} – ${r.str("path")}"
        val result = r.str("result").ifEmpty { "Not checked yet" }
        val ok = r.bool("ok")
        val detail = when {
            !ok && r.str("text").isNotEmpty() -> r.str("text")
            r.str("note").isNotEmpty() -> "$result – ${r.str("note")}"
            else -> result
        }
        val when_ = if (r.dbl("checked_at") > 0) {
            val since = if (!ok && r.dbl("since") > 0) " · not writable since ${date(r.dbl("since"))}" else ""
            "Checked ${date(r.dbl("checked_at"))}$since"
        } else "Not checked yet"
        return Triple(what, detail, when_)
    }

    /** What Diagnostics says about the probe (the web's `FOLDERS_HELP`). */
    const val HELP = "Probed every minute while downloads run: a file is written, flushed to the disk and removed. While nothing downloads they are looked at every 10 minutes without writing, so idle disks can spin down"

    /**
     * The lines under a folder's name on the page, as the web's: the detail only when it says more than the badge
     * (a note, the banner's sentence), the time only once the folder was probed.
     */
    fun shownLines(r: JsonObject, date: (Double) -> String): Pair<String?, String?> {
        val (_, detail, whenText) = folderLines(r, date)
        val result = r.str("result").ifEmpty { "Not checked yet" }
        return (detail.takeIf { it != result }) to (whenText.takeIf { r.dbl("checked_at") > 0 })
    }

    /** The badge kind of a folder: err while it holds downloads up, warn for a result with a note, else ok. */
    fun kind(r: JsonObject): String = when {
        !r.bool("ok") -> "err"
        r.str("state") !in setOf("ok", "unknown", "") -> "warn"
        else -> "ok"
    }

    /** The folders' lines of Copy report (none from an older server). */
    fun reportLines(last: JsonObject, date: (Double) -> String): List<String> {
        val folders = folders(last)
        if (folders.isEmpty()) return emptyList()
        return listOf("", "Download folders:") + folders.map { r ->
            val (what, detail, whenText) = folderLines(r, date)
            "${if (r.bool("ok")) "OK  " else "FAIL"} $what\n     $detail ($whenText)"
        }
    }
}

/** The Download folders section of Diagnostics (nothing from a server without the probe). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun DownloadFolders(d: JsonObject, date: (Double) -> String) {
    val folders = DownloadFolderRules.folders(d)
    if (folders.isEmpty()) return
    val p = MaterialTheme.palette
    PageSection("Download folders", DownloadFolderRules.HELP)
    SetCard(flat = true) {
        folders.forEachIndexed { i, r ->
            if (i > 0) RowDivider(startIndent = 0.dp)
            val (detail, whenText) = DownloadFolderRules.shownLines(r, date)
            val ok = r.bool("ok")
            val kind = DownloadFolderRules.kind(r)
            Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.Top) {
                // the dot in the badge's colour: warn for a client's folder Encore only reads
                val dot = when (kind) { "err" -> EncoreColors.Err; "warn" -> EncoreColors.Warn; else -> EncoreColors.Ok }
                Box(Modifier.padding(top = 6.dp).size(9.dp).clip(CircleShape).background(dot))
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(r.str("label"), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
                        Badge(r.str("result").ifEmpty { "Not checked yet" }, kind = kind)
                    }
                    Mono(r.str("path"), color = p.faint)
                    if (detail != null) Muted(detail, color = if (ok) p.muted else EncoreColors.Err)
                    if (whenText != null) Muted(whenText, color = p.faint)
                }
            }
        }
    }
}
