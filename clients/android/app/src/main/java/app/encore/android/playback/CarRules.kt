package app.encore.android.playback

import androidx.media3.common.Player
import androidx.media3.session.CommandButton
import app.encore.android.R

/**
 * The car's rules without Android in them (CarRulesTest): the now-playing buttons, the subtitle line, the
 * "More by <artist>" browse node and how big a lyric line can be drawn.
 */
object CarRules {
    // ---- the now-playing buttons -------------------------------------------------------------------------------

    /**
     * One custom button of the now-playing row: a session command, its name (screen readers, hosts that show
     * names), Media3's icon constant and our own drawable (0: Media3's drawable for [icon]).
     */
    data class Button(val command: String, val name: String, val icon: Int, val customIcon: Int = 0)

    /**
     * Every button goes to the overflow slot only. Media3 1.11 (MediaSessionLegacyStub
     * .updateCustomLayoutAndLegacyExtrasForMediaButtonPreferences → CommandButton
     * .getCustomLayoutFromMediaButtonPreferences) turns the preferences into the platform session's custom actions:
     * a button that may take the back or forward slot comes first and replaces Previous or Next; the overflow ones
     * follow in list order. With none in back or forward, the custom actions are exactly this list, in this order,
     * and the session's slot reservations stay on, so Android Auto keeps Previous and Next where they are.
     */
    val SLOTS = intArrayOf(CommandButton.SLOT_OVERFLOW)

    /**
     * The custom buttons. In the car ([car]): Shuffle, Repeat, Like, Lyrics, exactly four, which is what Android
     * Auto's now-playing row shows beside Play/Pause, Previous and Next before it needs its "⋯" (a fifth button
     * would push the last ones behind it). On the phone (notification, lock screen) Like comes first, since the
     * system media controls show the first two.
     */
    fun buttons(car: Boolean, liked: Boolean, shuffled: Boolean, repeatMode: Int, lyricsOn: Boolean): List<Button> {
        val like = Button(
            CarControls.CMD_LIKE,
            if (liked) "Unlike" else "Like",
            if (liked) CommandButton.ICON_HEART_FILLED else CommandButton.ICON_HEART_UNFILLED,
            if (liked) R.drawable.ic_favorite else R.drawable.ic_favorite_border,
        )
        val shuffle = Button(
            CarControls.CMD_SHUFFLE,
            if (shuffled) "Shuffle on" else "Shuffle off",
            if (shuffled) CommandButton.ICON_SHUFFLE_ON else CommandButton.ICON_SHUFFLE_OFF,
        )
        val repeat = when (repeatMode) {
            Player.REPEAT_MODE_ALL -> Button(CarControls.CMD_REPEAT, "Repeat all", CommandButton.ICON_REPEAT_ALL)
            Player.REPEAT_MODE_ONE -> Button(CarControls.CMD_REPEAT, "Repeat one", CommandButton.ICON_REPEAT_ONE)
            else -> Button(CarControls.CMD_REPEAT, "Repeat off", CommandButton.ICON_REPEAT_OFF)
        }
        // no predefined lyrics icon in Media3: ours
        val lyrics = Button(
            CarControls.CMD_LYRICS,
            if (lyricsOn) "Live lyrics on" else "Live lyrics off",
            CommandButton.ICON_UNDEFINED,
            if (lyricsOn) R.drawable.ic_lyrics_on else R.drawable.ic_lyrics,
        )
        return if (car) listOf(shuffle, repeat, like, lyrics) else listOf(like, shuffle, repeat, lyrics)
    }

    // ---- the subtitle line ---------------------------------------------------------------------------------------

    /**
     * The car's subtitle line: the artist with the quality detail after it ("DROELOE · FLAC 24/96"), the detail
     * alone without an artist; "Group listen · " first in a group listen. [detail] null or empty: none.
     */
    fun subtitle(artist: String, detail: String?, group: Boolean): String {
        val d = detail?.takeIf { it.isNotEmpty() }
        val line = when {
            d == null -> artist
            artist.isEmpty() -> d
            else -> "$artist · $d"
        }
        if (!group) return line
        return if (line.isEmpty()) "Group listen" else "Group listen · $line"
    }

    /**
     * The subtitle while a lyric line is the title (Settings → Playback → In the car → Title): the song comes first
     * and the quality last ("Gravity · DROELOE · FLAC 24/96"), so a car that cuts the line short cuts the quality,
     * never the song.
     */
    fun lineSubtitle(title: String, artist: String, detail: String?, group: Boolean): String =
        subtitle(listOf(title, artist).filter { it.isNotEmpty() }.joinToString(" · "), detail, group)

    // ---- "More by <artist>" ---------------------------------------------------------------------------------------

    /** `more-by:<mbid or name>`: the playing song's artist at the top of Home. */
    const val MORE_BY = "more-by:"
    /** `artist-albums:<mbid or name>`: the artist's albums in the library. */
    const val ARTIST_ALBUMS = "artist-albums:"
    /** `artist-shuffle:<name>`: the artist's songs, shuffled (a playable entry). */
    const val ARTIST_SHUFFLE = "artist-shuffle:"
    /** `artist-popular:<mbid>`: the artist's popular songs that are in the library. */
    const val ARTIST_POPULAR = "artist-popular:"
    /** `artist-liked:<mbid or name>`: the artist's songs the listener liked. */
    const val ARTIST_LIKED = "artist-liked:"

    enum class Kind { PLAY, FOLDER }

    /** A row of a browse node before it becomes a MediaItem. */
    data class Row(val id: String, val title: String, val subtitle: String?, val kind: Kind)

    fun moreByTitle(name: String): String = "More by $name"

    /**
     * The "More by <artist>" node: Shuffle <artist>, Popular songs (needs the mbid), Albums, Liked from this artist.
     * Rows with nothing behind them are left out. [key]: the node's key (mbid, else name).
     */
    fun moreByRows(name: String, mbid: String, key: String, popular: Int, albums: Int, liked: Int): List<Row> {
        if (name.isEmpty()) return emptyList()
        val out = ArrayList<Row>()
        out += Row("$ARTIST_SHUFFLE$name", "Shuffle $name", null, Kind.PLAY)
        if (mbid.isNotEmpty() && popular > 0) out += Row("$ARTIST_POPULAR$mbid", "Popular songs", plural(popular, "song"), Kind.FOLDER)
        if (albums > 0) out += Row("$ARTIST_ALBUMS$key", "Albums", plural(albums, "album"), Kind.FOLDER)
        if (liked > 0) out += Row("$ARTIST_LIKED$key", "Liked from this artist", plural(liked, "song"), Kind.FOLDER)
        return out
    }

    /** The Encore Lyrics app's "More by <artist>": the message after it added songs ([added] of them). */
    fun moreByMessage(name: String, added: Int, group: Boolean): String = when {
        added <= 0 -> "No other songs by $name in your library"
        group -> "Added ${plural(added, "song")} by $name to the group's Up next"
        else -> "Added ${plural(added, "song")} by $name after this song"
    }

    /**
     * The Encore Lyrics app's "More by <artist>" when the songs couldn't be fetched: the phone's Encore didn't answer
     * or the server failed, so it names neither.
     */
    fun moreByFailed(name: String): String = "Couldn't load songs by $name. Try again in a moment."

    /** How many of the artist's songs "More by <artist>" in Encore Lyrics adds after the current one. */
    const val MORE_BY_SONGS = 25

    fun plural(n: Int, what: String) = "$n $what${if (n == 1) "" else "s"}"

    // ---- lyric text as large as it fits --------------------------------------------------------------------------

    /** Lines wrapped at one text size: [size] in px, [lines] the text of each line. */
    data class Fit(val size: Float, val lines: List<String>) {
        /** the block's height at [leading] (line height = size × leading) */
        fun height(leading: Float): Float = lines.size * size * leading
    }

    /**
     * [text] wrapped to [width] at the largest size in [minSize]..[maxSize] whose lines fit in [height] (each line
     * size × [leading] high) and number at most [maxLines]. Words break at spaces; a word wider than the line
     * breaks between characters (text without spaces, long words). Lines come out balanced (the narrowest width
     * that keeps the same number of lines), so centred text has no lonely last word. When even [minSize] does not
     * fit, the lines that do are kept and the last one ends in "…". [measure] gives a string's width at a size.
     */
    fun fit(
        text: String,
        width: Float,
        height: Float,
        minSize: Float,
        maxSize: Float,
        leading: Float,
        maxLines: Int = Int.MAX_VALUE,
        measure: (String, Float) -> Float,
    ): Fit {
        val t = text.trim().ifEmpty { "♪" }
        val lo0 = minSize.coerceAtLeast(1f)
        val hi0 = maxSize.coerceAtLeast(lo0)
        fun fits(size: Float): List<String>? {
            val lines = wrap(t, width, size, measure)
            return if (lines.size <= maxLines && lines.size * size * leading <= height) lines else null
        }
        fits(hi0)?.let { return Fit(hi0, balance(t, width, hi0, it, measure)) }
        // the largest whole-pixel size that fits
        var lo = lo0
        var hi = hi0
        var best: List<String>? = fits(lo0)
        if (best == null) {
            // not even the smallest size: what fits, the last line shortened
            val all = wrap(t, width, lo0, measure)
            val room = minOf(maxLines, (height / (lo0 * leading)).toInt()).coerceAtLeast(1)
            if (all.size <= room) return Fit(lo0, all)
            val kept = all.take(room).toMutableList()
            kept[room - 1] = ellipsize(all.subList(room - 1, all.size).joinToString(" "), width, lo0, measure)
            return Fit(lo0, kept)
        }
        while (hi - lo > 1f) {
            val mid = kotlin.math.floor((lo + hi) / 2f)
            if (mid <= lo) break
            val got = fits(mid)
            if (got != null) { lo = mid; best = got } else hi = mid
        }
        return Fit(lo, balance(t, width, lo, best!!, measure))
    }

    /** Greedy wrapping at [width]: words at spaces, words wider than the line between characters. */
    fun wrap(text: String, width: Float, size: Float, measure: (String, Float) -> Float): List<String> {
        val words = text.trim().split(Regex("\\s+")).filter { it.isNotEmpty() }
        if (words.isEmpty()) return listOf("")
        val out = ArrayList<String>()
        var line = ""
        for (w in words) {
            val joined = if (line.isEmpty()) w else "$line $w"
            if (measure(joined, size) <= width) { line = joined; continue }
            if (line.isNotEmpty()) out += line
            if (measure(w, size) <= width) { line = w; continue }
            // a word wider than the line: as many characters per line as fit
            var rest = w
            while (rest.isNotEmpty()) {
                var n = rest.length
                while (n > 1 && measure(rest.substring(0, n), size) > width) n--
                val piece = rest.substring(0, n)
                rest = rest.substring(n)
                if (rest.isEmpty()) line = piece else out += piece
            }
        }
        if (line.isNotEmpty()) out += line
        return out
    }

    /**
     * The same text over the same number of lines at the narrowest width that still gives that many, without
     * breaking a word that the full width kept whole.
     */
    private fun balance(text: String, width: Float, size: Float, lines: List<String>, measure: (String, Float) -> Float): List<String> {
        if (lines.size < 2) return lines
        val pieces = lines.flatMap { it.split(' ') }
        var lo = width * 0.5f
        var hi = width
        var best = lines
        repeat(8) {
            val mid = (lo + hi) / 2f
            val got = wrap(text, mid, size, measure)
            if (got.size == lines.size && got.flatMap { it.split(' ') } == pieces) { best = got; hi = mid } else lo = mid
        }
        return best
    }

    private fun ellipsize(text: String, width: Float, size: Float, measure: (String, Float) -> Float): String {
        if (measure(text, size) <= width) return text
        var n = text.length
        while (n > 0 && measure(text.substring(0, n).trimEnd() + "…", size) > width) n--
        return text.substring(0, n).trimEnd() + "…"
    }

    /** The text a lyric line shows: an empty line (an instrumental break) is a note. */
    fun shown(text: String?): String = text?.trim()?.ifEmpty { null } ?: "♪"

    // ---- the lyrics card (the now-playing artwork in Android Auto) -------------------------------------------------

    /** The card is this many pixels square: sharp on a car screen's cover box, small enough to send. */
    const val CARD_SIZE = 1024

    /** The card's text: the current line ([current], at [currentTop]) and the next one ([next], smaller, below). */
    data class Card(val current: Fit, val currentTop: Float, val next: Fit?, val nextTop: Float)

    const val CARD_MARGIN = 64f
    const val CARD_LEADING = 1.12f
    const val CARD_GAP = 40f
    const val CARD_NEXT_MAX = 62f
    const val CARD_NEXT_MIN = 40f
    const val CARD_CURRENT_MAX = 168f
    const val CARD_CURRENT_MIN = 56f

    /**
     * The card's layout: the current line as large as it fits (up to [CARD_CURRENT_MAX] px, wrapped, centred),
     * the next line below it at up to [CARD_NEXT_MAX] px on at most two lines, the pair centred on the card.
     * [bold] measures the current line's typeface, [regular] the next line's.
     */
    fun card(current: String, next: String?, bold: (String, Float) -> Float, regular: (String, Float) -> Float): Card {
        val size = CARD_SIZE.toFloat()
        val w = size - 2 * CARD_MARGIN
        val n = next?.let { fit(shown(it), w, CARD_NEXT_MAX * CARD_LEADING * 2, CARD_NEXT_MIN, CARD_NEXT_MAX, CARD_LEADING, 2, regular) }
        val nextH = n?.height(CARD_LEADING) ?: 0f
        val room = size - 2 * CARD_MARGIN - (if (n != null) nextH + CARD_GAP else 0f)
        val c = fit(shown(current), w, room, CARD_CURRENT_MIN, CARD_CURRENT_MAX, CARD_LEADING, measure = bold)
        val total = c.height(CARD_LEADING) + (if (n != null) CARD_GAP + nextH else 0f)
        val top = ((size - total) / 2f).coerceAtLeast(CARD_MARGIN * 0.5f)
        return Card(c, top, n, top + c.height(CARD_LEADING) + CARD_GAP)
    }

    // ---- the Encore Lyrics surface ---------------------------------------------------------------------------------

    /** One block of text on the surface: lyric line [index] (-1: the note before the first line). */
    data class Block(val index: Int, val fit: Fit, val top: Float, val alpha: Float, val current: Boolean)

    const val SURFACE_LEADING = 1.1f

    /**
     * The lyrics on the car screen's free area ([width] × [height] px at [density] px per dp): the current line
     * as large as it fits (up to a fifth of the height, at most 96 dp; half the height for up to three lines),
     * the next line below it at about half that size, and the lines before it above, dimmed, one row each, as many
     * as there is room for (two at most). Tops are relative to the area's top; the text is centred across its width.
     * [lines] the song's lines, [at] the one playing (-1 before the first: a note, then the first line).
     */
    fun surface(
        lines: List<String>,
        at: Int,
        width: Float,
        height: Float,
        density: Float,
        bold: (String, Float) -> Float,
        regular: (String, Float) -> Float,
    ): List<Block> {
        if (lines.isEmpty() || width <= 0f || height <= 0f) return emptyList()
        val padX = width * 0.05f
        val padY = height * 0.06f
        val w = width - 2 * padX
        val maxCur = minOf(height * 0.2f, 96f * density).coerceAtLeast(20f * density)
        val minCur = (22f * density).coerceAtMost(maxCur)
        val curText = if (at in lines.indices) shown(lines[at]) else "♪"
        val cur = fit(curText, w, height * 0.5f, minCur, maxCur, SURFACE_LEADING, 3, bold)
        val small = (cur.size * 0.5f).coerceIn(16f * density, 40f * density).coerceAtMost(cur.size)
        val gap = small * 0.6f
        val nextIndex = at + 1
        val next = if (nextIndex in lines.indices) fit(shown(lines[nextIndex]), w, small * SURFACE_LEADING * 2, small * 0.8f, small, SURFACE_LEADING, 2, regular) else null
        val curH = cur.height(SURFACE_LEADING)
        val nextH = next?.let { gap + it.height(SURFACE_LEADING) } ?: 0f
        // the current line's middle a little above the middle; up when the next line would not fit below
        var top = height * 0.44f - curH / 2f
        top = minOf(top, height - padY - nextH - curH)
        top = top.coerceAtLeast(padY)
        val out = ArrayList<Block>()
        out += Block(if (at in lines.indices) at else -1, cur, top, 1f, true)
        if (next != null) out += Block(nextIndex, next, top + curH + gap, 0.62f, false)
        // the lines before, dimmed and a little smaller, one row each, while there is room above
        var y = top - gap
        val alphas = floatArrayOf(0.34f, 0.2f)
        val past = small * 0.85f
        for (k in 1..2) {
            val i = at - k
            if (i < 0) break
            val f = fit(shown(lines[i]), w, past * SURFACE_LEADING, past * 0.8f, past, SURFACE_LEADING, 1, regular)
            val h = f.height(SURFACE_LEADING)
            if (y - h < padY * 0.5f) break
            y -= h
            out += Block(i, f, y, alphas[k - 1], false)
            y -= gap
        }
        return out
    }
}
