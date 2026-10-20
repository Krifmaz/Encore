package app.encore.android.features.offline

import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.parseJson
import app.encore.android.core.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.net.URLDecoder
import java.util.Locale

/**
 * One song saved on this phone: the shared record of docs/contracts/offline-manifest.md (0.28), with the two
 * fields file-based clients add. [file] is the audio's path relative to `Music/Encore/`
 * (`<Artist>/<Album>/01 - Title.flac`); [art] the cover's path relative to the folder that holds the index
 * (see [OfflineStore] for where that is on each Android version).
 */
data class OfflineRecord(
    val id: Long,
    val title: String,
    val artist: String,
    val album: String,
    val albumId: String = "",
    val ext: String,
    val bytes: Long = 0L,
    val quality: String = "",
    val cover: Boolean = false,
    val lyrics: Boolean = false,
    /** Unix seconds; a fraction (desktop 0.27) is kept on read and dropped on write */
    val savedAt: Double = 0.0,
    val sourceServer: String = "",
    val duration: Double = 0.0,
    val track: Int = 0,
    val disc: Int = 0,
    val format: String = "",
    /** the song's `artists` as the server sent them (names, or objects with a `name`) */
    val artists: List<JsonElement> = emptyList(),
    val file: String = "",
    val art: String = "",
) {
    /** A copy the server converted (the phone has no decoder for the original): marked only by the quality label. */
    val converted: Boolean get() = OfflineIndex.isConverted(quality)

    /** Albums group by id; songs without one by the album's name. */
    val albumKey: String get() = if (albumId.isNotEmpty()) "id:$albumId" else "name:" + album.trim().lowercase(Locale.ROOT)
}

/** The saved songs of one album, in disc and track order. */
data class OfflineAlbum(val key: String, val album: String, val albumId: String, val artist: String, val songs: List<OfflineRecord>) {
    val bytes: Long get() = songs.sumOf { it.bytes }
    val latest: Double get() = songs.maxOfOrNull { it.savedAt } ?: 0.0
}

/**
 * Reading and writing the index (an array of records, oldest first) and the pure rules around it: the upgrade of
 * older shapes, the quality label, file and folder names. No Android here, so it can be checked on its own.
 */
object OfflineIndex {
    // ---- reading -----------------------------------------------------------------------------------------

    /**
     * The records of an index document, leniently: a plain array, or an object holding it under `songs`, `tracks`
     * or `records`; a record without a usable `id` is skipped; a record that appears twice (same server and id)
     * keeps its last entry. Anything unreadable is an empty index.
     */
    fun parse(text: String?): List<OfflineRecord> {
        val arr = when (val el = parseJson(text)) {
            is JsonArray -> el
            is JsonObject -> (el["songs"] ?: el["tracks"] ?: el["records"]) as? JsonArray ?: return emptyList()
            else -> return emptyList()
        }
        val out = LinkedHashMap<Pair<String, Long>, OfflineRecord>()
        for (e in arr) {
            val r = (e as? JsonObject)?.let { record(it) } ?: continue
            val k = r.sourceServer to r.id
            out.remove(k)
            out[k] = r
        }
        return out.values.toList()
    }

    /** One record, upgraded from the classic app's and desktop 0.27's shapes; null without a usable id. */
    fun record(o: JsonObject): OfflineRecord? {
        val id = idOf(o["id"]) ?: return null
        val file = assetName(o.str("file"))
        val art = assetName(o.str("art"))
        val format0 = o.str("format").trim().lowercase(Locale.ROOT)
        val ext = cleanExt(o.str("ext")).ifEmpty { cleanExt(file.substringAfterLast('.', "")) }.ifEmpty { cleanExt(format0) }.ifEmpty { "audio" }
        val quality = if (o.containsKey("quality") && o["quality"] !is kotlinx.serialization.json.JsonNull) o.str("quality")
            else if (ext == "audio") "" else ext.uppercase(Locale.ROOT)
        return OfflineRecord(
            id = id,
            title = o.str("title"),
            artist = o.str("artist"),
            album = o.str("album"),
            albumId = o.str("album_id"),
            ext = ext,
            bytes = o.long("bytes").coerceAtLeast(0L),
            quality = quality,
            cover = if (o.containsKey("cover")) o.bool("cover") else art.isNotEmpty(),
            lyrics = o.bool("lyrics"),
            savedAt = o.dbl("saved_at").coerceAtLeast(0.0),
            sourceServer = o.str("source_server").trim().trimEnd('/'),
            duration = o.dbl("duration").let { if (it.isFinite() && it > 0) it else 0.0 },
            track = o.int("track").coerceAtLeast(0),
            disc = o.int("disc").coerceAtLeast(0),
            format = format0.ifEmpty { if (ext == "audio") "" else ext },
            artists = (o["artists"] as? JsonArray)?.toList() ?: emptyList(),
            file = file,
            art = art,
        )
    }

    /** A number, or (older indexes) a string of digits; null for anything else or 0. */
    private fun idOf(e: JsonElement?): Long? {
        val p = e as? JsonPrimitive ?: return null
        val id = if (p.isString) {
            val t = p.content.trim()
            if (t.isEmpty() || !t.all { it in '0'..'9' }) null else t.toLongOrNull()
        } else {
            p.content.toDoubleOrNull()?.takeIf { it.isFinite() && it == Math.floor(it) }?.toLong()
        }
        return id?.takeIf { it > 0L }
    }

    /** An asset URL of the classic app (`asset://…`, `https://asset.localhost/…`) becomes its last path segment. */
    fun assetName(s: String): String {
        val t = s.trim()
        val asset = t.startsWith("asset://") || t.startsWith("https://asset.localhost/") || t.startsWith("http://asset.localhost/")
        if (!asset) return t
        val last = t.substringBefore('?').trimEnd('/').substringAfterLast('/')
        return try { URLDecoder.decode(last.replace("+", "%2B"), "UTF-8") } catch (_: Exception) { last }
    }

    private fun cleanExt(s: String): String = s.trim().trimStart('.').lowercase(Locale.ROOT).filter { it in 'a'..'z' || it in '0'..'9' }.take(8)

    // ---- writing -----------------------------------------------------------------------------------------

    fun toJson(r: OfflineRecord): JsonObject = buildJsonObject {
        put("id", r.id)
        put("title", r.title)
        put("artist", r.artist)
        put("album", r.album)
        put("album_id", r.albumId)
        put("ext", r.ext)
        put("bytes", r.bytes)
        put("quality", r.quality)
        put("cover", r.cover)
        put("lyrics", r.lyrics)
        put("saved_at", Math.floor(r.savedAt).toLong())
        put("source_server", r.sourceServer)
        put("duration", r.duration)
        put("track", r.track)
        put("disc", r.disc)
        put("format", r.format)
        put("artists", JsonArray(r.artists))
        put("file", r.file)
        put("art", r.art)
    }

    /** The index document: always the plain array. */
    fun write(records: List<OfflineRecord>): String = buildJsonArray { for (r in records) add(toJson(r)) }.toString()

    // ---- the quality label (offline-manifest.md "The quality label"; web offline/download.rs) ------------------

    /** A codec as the player writes it. */
    fun codecName(codec: String): String = when (val c = codec.trim().lowercase(Locale.ROOT)) {
        "opus" -> "Opus"
        "vorbis", "ogg vorbis", "ogg", "oga" -> "Vorbis"
        "wavpack", "wv" -> "WavPack"
        "m4a", "mp4" -> "AAC"
        else -> c.uppercase(Locale.ROOT)
    }

    /** Hz as kHz, exact for every common rate (up to three decimals, no trailing zeros): 44100 → "44.1", 96000 → "96", 22050 → "22.05". */
    fun khz(hz: Double): String = String.format(java.util.Locale.US, "%.3f", hz / 1000.0).trimEnd('0').trimEnd('.')

    private val LOSSLESS = setOf("flac", "alac", "wav", "aiff", "aif", "ape", "wavpack", "wv", "tta")

    /** The label of an original: "FLAC 16/44.1", "FLAC 44.1 kHz", "FLAC 24-bit", "MP3 320k", "Opus". */
    fun qualityLabel(codec: String, lossless: Boolean, bits: Int, rateHz: Double, kbps: Double): String {
        val name = codecName(codec)
        if (lossless || codec.trim().lowercase(Locale.ROOT) in LOSSLESS) {
            return when {
                bits > 0 && rateHz > 0 -> "$name $bits/${khz(rateHz)}"
                rateHz > 0 -> "$name ${khz(rateHz)} kHz"
                bits > 0 -> "$name $bits-bit"
                else -> name
            }
        }
        return if (kbps > 0) "$name ${Math.round(kbps)}k" else name
    }

    /** The label of a copy the server converted to FLAC on its own (the phone has no decoder for the original). */
    const val CONVERTED_FLAC = "FLAC (converted)"

    fun isConverted(quality: String): Boolean = quality.trimEnd().endsWith("(converted)")

    /** The file's codec in the decoders list's words, from `/info` (which tells ALAC from AAC) or the extension. */
    fun codecOf(infoCodec: String, ext: String): String = when (val c = infoCodec.trim().lowercase(Locale.ROOT)) {
        "" -> when (ext) { "m4a", "mp4" -> "aac"; "ogg", "oga" -> "vorbis"; else -> ext }
        "ogg vorbis" -> "vorbis"
        else -> c
    }

    // ---- names on the device -------------------------------------------------------------------------------

    /** Most a folder or file name part may take, in UTF-8 bytes (file systems allow 255 for the whole name). */
    private const val MAX_NAME_BYTES = 120

    /**
     * Device names Windows reserves, in any case and with any extension (`nul`, `Con.flac`): Android takes them,
     * but a folder holding one can't be copied to a PC or to an exFAT card or stick.
     */
    private val RESERVED = Regex("(?i)con|prn|aux|nul|conin\\$|conout\\$|com[0-9\u00b9\u00b2\u00b3]|lpt[0-9\u00b9\u00b2\u00b3]")

    /**
     * A server-sent name made safe as one path part: no `/` or `\`, no characters Android's FAT / exFAT cards
     * refuse (`:*?"<>|`), no control characters, no invisible ones (bidi controls, zero-width characters, the soft
     * hyphen: a name could look like another), no leading dots (no hidden files, no `.` or `..`), no trailing dots or
     * spaces, no device name Windows reserves ([RESERVED] gets a `_`), at most [MAX_NAME_BYTES] bytes; [fallback]
     * when nothing is left.
     */
    fun safeName(s: String, fallback: String): String = notReserved(clean(s, fallback))

    /** [safeName] without the reserved-name rule (a song's title before its track number is put in front). */
    private fun clean(s: String, fallback: String): String {
        val sb = StringBuilder()
        var i = 0
        while (i < s.length) {
            val cp = s.codePointAt(i)
            i += Character.charCount(cp)
            when {
                cp < 0x20 || cp == 0x7f || cp in 0x80..0x9f -> sb.append(' ')
                cp == '/'.code || cp == '\\'.code || cp == ':'.code || cp == '|'.code -> sb.append('-')
                cp == '*'.code || cp == '?'.code || cp == '"'.code || cp == '<'.code || cp == '>'.code -> sb.append('_')
                cp == 0x2028 || cp == 0x2029 -> sb.append(' ') // line and paragraph separators
                // the zero-width non-joiner and joiner shape words of other scripts (Persian, Hindi) and emoji
                (cp == 0x200c || cp == 0x200d) && joins(sb, s, i) -> sb.appendCodePoint(cp)
                Character.getType(cp) == Character.FORMAT.toInt() -> {} // invisible: bidi controls, zero-width, BOM, soft hyphen
                else -> sb.appendCodePoint(cp)
            }
        }
        var t = sb.toString().replace(Regex("\\s+"), " ").trim().trimStart('.', ' ').trimEnd('.', ' ')
        t = cutBytes(t, MAX_NAME_BYTES).trimEnd('.', ' ', '\u200c', '\u200d')
        return t.ifEmpty { fallback }
    }

    /** A joiner is kept only between two visible characters outside ASCII (`a` + joiner + `b` looks like `ab`). */
    private fun joins(before: StringBuilder, s: String, next: Int): Boolean {
        if (before.isEmpty() || next >= s.length) return false
        return visibleBeyondAscii(before.codePointBefore(before.length)) && visibleBeyondAscii(s.codePointAt(next))
    }

    private fun visibleBeyondAscii(cp: Int): Boolean =
        cp > 0x9f && !Character.isWhitespace(cp) && !Character.isSpaceChar(cp) && Character.getType(cp) != Character.FORMAT.toInt()

    /** `name` with `_` after its part before the first dot when that part is a device name Windows reserves. */
    private fun notReserved(name: String): String {
        val stem = name.substringBefore('.')
        return if (RESERVED.matches(stem.trimEnd())) stem.trimEnd() + "_" + name.substring(stem.length) else name
    }

    /** At most `max` UTF-8 bytes, never cutting a character (or a surrogate pair) in two. */
    private fun cutBytes(s: String, max: Int): String {
        if (s.toByteArray(Charsets.UTF_8).size <= max) return s
        val sb = StringBuilder()
        var bytes = 0
        var i = 0
        while (i < s.length) {
            val cp = s.codePointAt(i)
            val n = String(Character.toChars(cp)).toByteArray(Charsets.UTF_8).size
            if (bytes + n > max) break
            sb.appendCodePoint(cp)
            bytes += n
            i += Character.charCount(cp)
        }
        return sb.toString()
    }

    /** `<Artist>/<Album>`: the album artist's folder and the album's, under `Music/Encore/`. */
    fun albumDir(albumArtist: String, album: String): String =
        safeName(albumArtist, "Unknown artist") + "/" + safeName(album, "Unknown album")

    /** `NN - Title.ext` (the track number with two digits; just `Title.ext` without one). */
    fun fileName(track: Int, title: String, ext: String): String {
        val e = cleanExt(ext).ifEmpty { "audio" }
        val base = clean(title, "Unknown song")
        val name = if (track > 0) String.format(Locale.ROOT, "%02d - %s", track, base) else base
        return notReserved(cutBytes(name, MAX_NAME_BYTES + 6).trimEnd('.', ' ')) + "." + e
    }

    /** `name` with " (2)", " (3)" … before its extension, for a name that is taken. */
    fun numbered(name: String, n: Int): String {
        val dot = name.lastIndexOf('.')
        return if (dot > 0) name.substring(0, dot) + " ($n)" + name.substring(dot) else "$name ($n)"
    }

    /**
     * A relative path from the index is used only when it stays inside its folder: no absolute path, no `..`
     * part, no empty part, no backslash. Null otherwise (an index edited by hand cannot point elsewhere).
     */
    fun safeRelative(path: String): String? {
        val p = path.trim()
        if (p.isEmpty() || p.startsWith("/") || p.contains('\\') || p.contains('\u0000')) return null
        val parts = p.split('/')
        if (parts.any { it.isEmpty() || it == "." || it == ".." }) return null
        return p
    }

    /** Only letters, digits, `-` and `_` (an album id as part of a file name). */
    fun cleanId(s: String): String = s.filter { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' || it == '_' }.take(80)

    // ---- the Downloaded list -------------------------------------------------------------------------------

    /** Every word of the search appears in the song's title, artist or album. */
    fun matches(r: OfflineRecord, query: String): Boolean {
        val hay = "${r.title} ${r.artist} ${r.album}".lowercase(Locale.ROOT)
        return query.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.all { hay.contains(it.lowercase(Locale.ROOT)) }
    }

    /** The artist an album shows: the one most of its songs have ("Various artists" when none stands out). */
    fun albumArtist(songs: List<OfflineRecord>): String {
        val counts = LinkedHashMap<String, Pair<String, Int>>()
        for (r in songs) {
            val a = r.artist.trim()
            if (a.isEmpty()) continue
            val k = a.lowercase(Locale.ROOT)
            val was = counts[k]
            counts[k] = (was?.first ?: r.artist) to ((was?.second ?: 0) + 1)
        }
        return when (counts.size) {
            0 -> ""
            1 -> counts.values.first().first
            else -> {
                val best = counts.values.maxOf { it.second }
                if (best * 2 > songs.size) counts.values.first { it.second == best }.first else "Various artists"
            }
        }
    }

    /** The songs that match `query`, by album (newest download first), each album in disc and track order. */
    fun group(records: List<OfflineRecord>, query: String = ""): List<OfflineAlbum> {
        val by = LinkedHashMap<String, MutableList<OfflineRecord>>()
        for (r in records) if (query.isBlank() || matches(r, query)) by.getOrPut(r.albumKey) { ArrayList() }.add(r)
        return by.map { (key, list) ->
            val songs = list.sortedWith(compareBy<OfflineRecord>({ it.disc }, { it.track }, { it.title.lowercase(Locale.ROOT) }))
            val first = songs.first()
            OfflineAlbum(key, first.album, first.albumId, albumArtist(songs), songs)
        }.sortedWith(compareByDescending<OfflineAlbum> { it.latest }.thenBy { it.album.lowercase(Locale.ROOT) })
    }

    // ---- words -----------------------------------------------------------------------------------------------

    fun songs(n: Int): String = if (n == 1) "1 song" else "$n songs"

    /** A failed answer in plain words (web offline/download.rs `status_words`). */
    fun statusWords(status: Int): String = when (status) {
        0 -> "Not connected to the server"
        401, 403 -> "Not signed in to the server"
        404 -> "The song is not on the server any more"
        else -> "The server answered $status"
    }

    /** "Downloading 3 of 12 · <title>". */
    fun progressText(done: Int, total: Int, current: String): String {
        val n = (done + 1).coerceAtMost(total.coerceAtLeast(1))
        return if (current.isBlank()) "Downloading $n of $total" else "Downloading $n of $total · ${current.trim()}"
    }

    /** The end of a download: "12 songs downloaded", "10 downloaded, 2 failed – <why>" and the toast kind. */
    fun summaryText(saved: Int, skipped: Int, failed: Int, why: String?): Pair<String, String> {
        val reason = if (why.isNullOrBlank()) "" else " – $why"
        return when {
            saved == 0 && failed == 0 -> (if (skipped > 0) "Already downloaded" else "Nothing to download") to "ok"
            failed == 0 -> "${songs(saved)} downloaded" to "ok"
            saved == 0 -> "${songs(failed)} couldn't be downloaded$reason" to "err"
            else -> "$saved downloaded, $failed failed$reason" to "warn"
        }
    }

    /** "Download again in original quality" for `n` songs of which `still` stay converted. */
    fun againText(n: Int, still: Int): Pair<String, String> {
        val done = (n - still).coerceAtLeast(0)
        return when {
            still == 0 && n == 1 -> "Downloaded again in original quality" to "ok"
            still == 0 -> "${songs(n)} downloaded again in original quality" to "ok"
            done == 0 && still == 1 -> STILL_CONVERTED to "warn"
            done == 0 -> STILL_CONVERTED_ALL to "warn"
            else -> "${songs(done)} downloaded again in original quality; $still ${if (still == 1) "stays" else "stay"} converted because this phone can't play the original" to "warn"
        }
    }

    const val STILL_CONVERTED = "This phone can't play the original file, so the converted copy stays"
    const val STILL_CONVERTED_ALL = "This phone can't play the original files, so the converted copies stay"
}
