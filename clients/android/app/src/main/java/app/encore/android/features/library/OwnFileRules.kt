package app.encore.android.features.library

import app.encore.android.core.dbl
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.str
import app.encore.android.data.repo.Me
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Bring your own music (web-rs/src/pages/own_file.rs; server routes::own_file): what the sheets show and send, without
 * Compose so it is unit-tested. ⋯ → *Replace with my own file* puts an uploaded file in a song's place; Library →
 * *Upload your own files* matches a batch of files to their songs by their tags. The song keeps its id, so plays,
 * likes, playlists and lyrics stay; the old file waits in the server's trash for 7 days.
 */
object OwnFileRules {
    /** The permission that lets a member replace songs' files (admins always may). */
    const val PERM = "own_files"

    /** The document types the picker offers (the server checks the bytes too). */
    val MIME_TYPES = arrayOf("audio/*", "application/ogg", "application/octet-stream")

    /** May this account replace songs' files? Admins, and members with the permission. */
    fun allowed(me: Me?): Boolean = me?.can(PERM) == true

    /** 44100 → "44.1", 96000 → "96". */
    fun khz(hz: Long): String = if (hz % 1000L == 0L) (hz / 1000L).toString() else String.format(java.util.Locale.ROOT, "%.1f", hz / 1000.0)

    /** "FLAC · 24-bit / 96 kHz" (or "MP3 · 320 kbps") of a file's facts `{format, bit_depth, sample_rate, bitrate}`. */
    fun qualityText(f: JsonObject?): String {
        if (f == null) return ""
        val parts = mutableListOf(f.str("format").uppercase())
        val (depth, rate) = f.long("bit_depth") to f.long("sample_rate")
        if (depth > 0 && rate > 0) parts.add("$depth-bit / ${khz(rate)} kHz") else if (f.long("bitrate") > 0) parts.add("${f.long("bitrate")} kbps")
        return parts.filter { it.isNotEmpty() }.joinToString(" · ")
    }

    /** The line once a song is replaced: "MP3 · 320 kbps → FLAC · 24-bit / 96 kHz", then any warnings. */
    fun resultText(v: JsonObject): String {
        val sb = StringBuilder("${qualityText(v.obj("before"))} → ${qualityText(v.obj("after"))}")
        for (w in (v["warnings"] as? kotlinx.serialization.json.JsonArray).orEmpty()) {
            (w as? JsonPrimitive)?.content?.let { sb.append(". ").append(it) }
        }
        val q = v.obj("quality")
        if (q?.str("status") == "suspect") sb.append(". Quality check: ").append(q.str("summary"))
        return sb.toString()
    }

    /**
     * The Signal path's note for a song someone replaced with their own file (`/info`'s `own_file`): "Your own file –
     * uploaded by alex on 24 Oct 2026, replacing an MP3"; "" when it isn't one. [day] formats the time (seconds).
     */
    fun note(o: JsonObject?, day: (Double) -> String): String {
        if (o == null) return ""
        val sb = StringBuilder("Your own file")
        if (o.str("by").isNotEmpty()) sb.append(" – uploaded by ").append(o.str("by"))
        if (o.dbl("at") > 0.0) sb.append(" on ").append(day(o.dbl("at")))
        if (o.str("old_format").isNotEmpty()) sb.append(", replacing ").append(article(o.str("old_format").uppercase()))
        return sb.toString()
    }

    /** "an MP3" / "a FLAC": the article a format's name takes when read out. */
    fun article(name: String): String {
        // letters said with a vowel sound first: A, E, F, H, I, L, M, N, O, R, S, X
        val vowel = name.isNotEmpty() && name[0] in "AEFHILMNORSX" && name != "FLAC"
        return (if (vowel) "an " else "a ") + name
    }

    /** One choice for a staged upload: a song ([id]) or "Leave out" (null). */
    data class Choice(val id: Long?, val label: String)

    /** A staged upload's choices: "Leave out", then the songs it may be ("Artist – Title (Album)"). */
    fun choices(staged: JsonObject): List<Choice> = listOf(Choice(null, "Leave out")) + staged["candidates"].objects().mapNotNull { c ->
        val t = c.obj("track") ?: return@mapNotNull null
        val album = if (t.str("album").isNotEmpty()) " (${t.str("album")})" else ""
        Choice(t.long("id"), "${t.str("artist")} – ${t.str("title")}$album")
    }

    /** The song a staged upload starts with: its sure match, else none (the person chooses or leaves it out). */
    fun defaultPick(staged: JsonObject): Long? = staged.obj("match")?.obj("track")?.long("id")?.takeIf { it > 0L }

    /** "Replace 1 song" / "Replace 3 songs". */
    fun applyLabel(n: Int): String = if (n == 1) "Replace 1 song" else "Replace $n songs"

    /** How one replaced song went, from Replace's answer `{replaced: [...], failed: [...]}` (null: not in it). */
    fun outcome(answer: JsonObject, token: String): Pair<String, Boolean>? {
        answer["replaced"].objects().firstOrNull { it.str("token") == token }?.let { r ->
            val warn = (r["warnings"] as? kotlinx.serialization.json.JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }.joinToString("") { " · $it" }
            return "Replaced ${r.str("artist")} – ${r.str("title")} – now ${r.str("format").uppercase()}$warn" to true
        }
        answer["failed"].objects().firstOrNull { it.str("token") == token }?.let { return it.str("reason") to false }
        return null
    }
}
