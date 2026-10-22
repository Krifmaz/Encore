package app.encore.android.playback

/**
 * The control strip of the Encore Lyrics car app (car/LyricsCarAppService.kt), without Android in it
 * (LyricsStripRulesTest): which buttons it has and in what order, picked on the Car buttons page.
 *
 * Encore Lyrics draws the whole free area of the car screen itself (the map surface of MapWithContentTemplate), but
 * Android Auto still draws two things it owns: the one-row song card and the action strip. The strip takes at most
 * [MAX] buttons (Android Auto's limit for a map template's action strip), so the page lets the listener pick up to
 * four and order them.
 */
object LyricsStripRules {
    /** A button the strip can hold: its stored key and its name on the page. */
    enum class Btn(val key: String, val label: String) {
        PREVIOUS("previous", "Previous"),
        PLAY("play", "Play / Pause"),
        NEXT("next", "Next"),
        ARTIST("artist", "More by the artist"),
        LIKE("like", "Like"),
        SHUFFLE("shuffle", "Shuffle"),
        REPEAT("repeat", "Repeat"),
    }

    /** Android Auto's limit for the action strip of a map template. */
    const val MAX = 4

    /** What Encore Lyrics had before the setting: Previous, Play/Pause, Next, More by the artist. */
    val DEFAULT = listOf(Btn.PREVIOUS, Btn.PLAY, Btn.NEXT, Btn.ARTIST)

    private const val VERSION = "v1"

    /** "v1|previous,play,next,artist" ("v1|" when the listener took every button off). */
    fun encode(strip: List<Btn>): String = "$VERSION|" + strip.joinToString(",") { it.key }

    /**
     * What [encode] wrote. Nothing or another version: [DEFAULT]. Unknown keys are dropped, a key listed twice stays
     * where it came first, and only the first [MAX] are kept.
     */
    fun decode(s: String?): List<Btn> {
        val t = s?.trim() ?: return DEFAULT
        if (t.substringBefore('|') != VERSION || !t.contains('|')) return DEFAULT
        return t.substringAfter('|').split(',')
            .mapNotNull { k -> Btn.entries.firstOrNull { it.key == k.trim() } }
            .distinct()
            .take(MAX)
    }

    /** On the strip: off. Off: onto the end of the strip, unless it is full (then unchanged). */
    fun toggle(strip: List<Btn>, b: Btn): List<Btn> = when {
        b in strip -> strip - b
        strip.size >= MAX -> strip
        else -> strip + b
    }

    /** Whether [b] can be turned on: it is on already, or there is room. */
    fun canAdd(strip: List<Btn>, b: Btn): Boolean = b in strip || strip.size < MAX

    /** One place to the left (up on the page); the first stays. */
    fun up(strip: List<Btn>, b: Btn): List<Btn> {
        val i = strip.indexOf(b)
        if (i <= 0) return strip
        return strip.toMutableList().apply { add(i - 1, removeAt(i)) }
    }

    /** One place to the right (down on the page); the last stays. */
    fun down(strip: List<Btn>, b: Btn): List<Btn> {
        val i = strip.indexOf(b)
        if (i < 0 || i == strip.lastIndex) return strip
        return strip.toMutableList().apply { add(i + 1, removeAt(i)) }
    }

    /** The page's rows: the strip's buttons in order, then the others in their own order. */
    fun rows(strip: List<Btn>): List<Btn> = strip + Btn.entries.filter { it !in strip }

    /** "Previous · Play / Pause · Next · More by the artist", or "No buttons". */
    fun summary(strip: List<Btn>): String = if (strip.isEmpty()) "No buttons" else strip.joinToString(" · ") { it.label }

    /** The line under the list: how many spots are left. */
    fun roomLine(strip: List<Btn>): String = when (val left = MAX - strip.size) {
        0 -> "All $MAX spots used. Turn one off to add another."
        1 -> "1 spot left"
        else -> "$left spots left"
    }
}
