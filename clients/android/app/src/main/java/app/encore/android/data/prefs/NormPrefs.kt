package app.encore.android.data.prefs

/**
 * Volume normalization (Settings → Playback → Sound (on this device)): kept on this device only, like the web's
 * localStorage keys `encore.norm.mode`, `encore.norm.target` and `encore.norm.clip`, whose names the phone uses too.
 *
 * mode: "off" | "track" | "album" · target: the loudness to aim for, −23…−5 LUFS · clip: "Prevent clipping" (the
 * gain never lifts a song's peak above full scale).
 */
data class NormPrefs(val mode: String = "off", val target: Int = DEFAULT_TARGET, val clip: Boolean = true) {
    val on: Boolean get() = mode == "track" || mode == "album"

    /** Limits for what is stored or typed. */
    fun clean(): NormPrefs = NormPrefs(
        mode = if (MODES.any { it.first == mode }) mode else "off",
        target = target.coerceIn(MIN_TARGET, MAX_TARGET),
        clip = clip,
    )

    companion object {
        const val KEY_MODE = "encore.norm.mode"
        const val KEY_TARGET = "encore.norm.target"
        const val KEY_CLIP = "encore.norm.clip"
        const val DEFAULT_TARGET = -14
        const val MIN_TARGET = -23
        const val MAX_TARGET = -5
        val MODES: List<Pair<String, String>> = listOf("off" to "Off", "track" to "Track", "album" to "Album")
    }
}
