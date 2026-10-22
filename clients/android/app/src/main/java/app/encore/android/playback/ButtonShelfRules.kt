package app.encore.android.playback

/**
 * Settings → Android Auto, without Android in it (ButtonShelfRules): the page shows each set of car buttons as two
 * horizontal rows, like Home's Recently added: "Shown" in the order the car gets them and "Hidden". A tap on a shown
 * button hides it, a tap on a hidden one adds it to the end of Shown (when there is room), and ◀ / ▶ move a shown
 * one a place. Used for the car player's buttons ([CarButtonRules]) and the Encore Lyrics strip ([LyricsStripRules]).
 */
object ButtonShelfRules {
    /** [x] off the shown row. */
    fun <T> hide(shown: List<T>, x: T): List<T> = shown - x

    /** [x] onto the end of the shown row, unless it is there already or the row holds [max]. */
    fun <T> show(shown: List<T>, x: T, max: Int = Int.MAX_VALUE): List<T> =
        if (x in shown || shown.size >= max) shown else shown + x

    /** One place left; the first stays. */
    fun <T> left(shown: List<T>, x: T): List<T> {
        val i = shown.indexOf(x)
        if (i <= 0) return shown
        return shown.toMutableList().apply { add(i - 1, removeAt(i)) }
    }

    /** One place right; the last stays. */
    fun <T> right(shown: List<T>, x: T): List<T> {
        val i = shown.indexOf(x)
        if (i < 0 || i == shown.lastIndex) return shown
        return shown.toMutableList().apply { add(i + 1, removeAt(i)) }
    }

    /** The hidden row: every button not shown, in [all]'s order. */
    fun <T> hidden(all: List<T>, shown: List<T>): List<T> = all.filter { it !in shown }

    /** "Shown · 4 of 4" with a [max], "Shown · 5" without. */
    fun shownTitle(count: Int, max: Int? = null): String = if (max == null) "Shown · $count" else "Shown · $count of $max"
}
