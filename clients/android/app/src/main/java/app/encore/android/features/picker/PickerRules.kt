package app.encore.android.features.picker

import app.encore.android.data.repo.PickSearch
import app.encore.android.data.repo.PickerSource
import java.util.Calendar
import java.util.TimeZone

/** The line under a source's head when its rows came from a kept search: the words, and whether "Search again" follows. */
data class FoundLine(val text: String, val again: Boolean)

/**
 * The picker's rules (web-rs/src/pages/picker.rs, and the kept-search words of the design in req-slsk-cache §5).
 * Pure, so the unit tests check them; PickerScreen draws what they say.
 */
object PickerRules {
    /** Another Soulseek search holds the turn (one runs at a time): this one waits for it. */
    const val WAITING = "Waiting for another Soulseek search, then searching…"

    /** The word after a tab's name for a source that can't search right now. */
    fun stateWord(state: String): String = when (state) {
        "sign_in" -> "sign in"
        "not_connected" -> "not connected"
        "not_installed" -> "not installed"
        "off" -> "off"
        else -> ""
    }

    /**
     * Whether the picker asks this source. A ready source always; Soulseek also while it is not connected, because a
     * server that keeps searches can still answer from them (a server that keeps none says 409 and the hint shows, as
     * before).
     */
    fun searches(s: PickerSource): Boolean = s.ready || (s.id == "slskd" && s.state == "not_connected")

    /** "Searching Soulseek… (Soulseek takes a few seconds)" while a first search runs. */
    fun searchingText(id: String, name: String): String = "Searching $name…" + if (id == "slskd") " (Soulseek takes a few seconds)" else ""

    /** While "Search again" runs, the kept rows stay and this line shows. */
    fun searchingAgainText(name: String): String = "Searching $name again…"

    /** What Copy takes from a source's error (picker.rs): "Torrents & Usenet: <the whole error>". */
    fun errorCopy(name: String, message: String): String = "$name: $message"

    /**
     * "Found at 14:02" for a kept answer: the server's own words when it sent them (the same clock as its "next attempt
     * at 14:20"), else made here from `cached_at`: today, yesterday, or the day.
     */
    fun foundText(f: PickSearch, nowSec: Double, zone: TimeZone = TimeZone.getDefault()): String {
        if (f.found.isNotBlank()) return f.found.trim()
        if (f.cachedAt <= 0.0) return ""
        val at = Calendar.getInstance(zone).apply { timeInMillis = (f.cachedAt * 1000).toLong() }
        val now = Calendar.getInstance(zone).apply { timeInMillis = (nowSec * 1000).toLong() }
        val time = "%02d:%02d".format(at.get(Calendar.HOUR_OF_DAY), at.get(Calendar.MINUTE))
        val days = dayNumber(now) - dayNumber(at)
        return when (days) {
            0L -> "Found at $time"
            1L -> "Found yesterday at $time"
            // kept answers last a day, so this is a server with a longer keep: the day goes before the time
            else -> "Found at " + "%d/%d %s".format(at.get(Calendar.DAY_OF_MONTH), at.get(Calendar.MONTH) + 1, time)
        }
    }

    private fun dayNumber(c: Calendar): Long =
        c.get(Calendar.YEAR) * 400L + c.get(Calendar.DAY_OF_YEAR)

    /** The words of two searches, compared without case, punctuation or word order. */
    fun sameQuery(a: String, b: String): Boolean = words(a) == words(b)

    private fun words(s: String): List<String> =
        s.lowercase().split(Regex("[^\\p{L}\\p{N}']+")).filter { it.isNotEmpty() }.sorted()

    /**
     * The line for a kept answer, or null for a live one (nothing to say):
     * - "Found at 14:02" + " · Search again"
     * - "No results. · Found at 14:02 · Search again" when the kept answer found nothing
     * - "Found at 14:02 for "Illuminate EP"" when it was searched with other words than the search box has
     * - "Found at 14:02 · Soulseek isn't connected – next attempt at 14:20" (no Search again) while it can't search
     */
    fun foundLine(f: PickSearch?, typed: String, empty: Boolean, nowSec: Double, zone: TimeZone = TimeZone.getDefault()): FoundLine? {
        if (f == null || !f.kept) return null
        val parts = ArrayList<String>()
        if (empty) parts.add("No results.")
        var found = foundText(f, nowSec, zone)
        if (f.query.isNotBlank() && !sameQuery(f.query, typed)) found += " for \"${f.query.trim()}\""
        if (found.isNotEmpty()) parts.add(found)
        if (!f.canSearch && f.notice.isNotBlank()) parts.add(f.notice.trim())
        return FoundLine(parts.joinToString(" · "), again = f.canSearch)
    }
}
