package app.encore.android.ui.screens

/**
 * How the full player ([FullPlayer]) lays itself out for the window it has: upright with the big cover over the
 * controls, or sideways with the lyrics in one half and the cover and controls in the other. Plain numbers in dp, so
 * the rules are tested without a device (PlayerLayoutTest).
 */
object PlayerLayout {
    /** The least the sideways cover shrinks to, in dp: smaller than this and it stops reading as album art. */
    const val MIN_ART = 72f

    /** The most the sideways cover grows to, in dp (a tablet's half screen would otherwise be all cover). */
    const val MAX_ART = 240f

    /**
     * What the right half needs under the cover row, in dp: the seek bar with its times (~76) and prev / play / next
     * (72) plus a little air; the cover takes the rest of the height.
     */
    const val UNDER_ART = 164f

    /**
     * Whether the window is sideways: wider than it is tall. A phone turned on its side, a tablet held flat or a wide
     * window on a desktop; a square or upright window keeps the upright player.
     *
     * @param widthDp the window's width in dp (`Configuration.screenWidthDp`).
     * @param heightDp the window's height in dp (`Configuration.screenHeightDp`).
     */
    fun isWide(widthDp: Int, heightDp: Int): Boolean = widthDp > heightDp

    /**
     * The sideways cover's side, in dp: the height left above the seek bar and buttons, at most 45 % of the half's
     * width (so the title, links and like / share / ⋯ keep room beside it), kept between [MIN_ART] and [MAX_ART].
     *
     * @param paneWidthDp the width of the right half, in dp.
     * @param paneHeightDp the height of the right half, in dp.
     */
    fun landscapeArt(paneWidthDp: Float, paneHeightDp: Float): Float {
        val fit = minOf(paneHeightDp - UNDER_ART, paneWidthDp * 0.45f)
        return fit.coerceIn(MIN_ART, MAX_ART)
    }
}
