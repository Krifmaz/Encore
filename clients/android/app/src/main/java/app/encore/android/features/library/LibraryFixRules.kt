package app.encore.android.features.library

import app.encore.android.data.api.Track
import app.encore.android.data.repo.Me

/** The rules of the library's correcting tools (web-rs/src/pages/library.rs), pure so the unit tests check them. */
object LibraryFixRules {
    /**
     * Whether [me] may correct a song: admins any song, members with `fix` a song their own request brought in. Where
     * the row says who requested it (the album page), a member's entries show only on their own songs: the server
     * answers 403 for anyone else's (library.rs `owns_request`). Other lists don't say, and leave it to the server,
     * as the web does.
     */
    fun canFix(t: Track, me: Me?): Boolean {
        if (me == null) return false
        if (me.isAdmin) return true
        if (!me.can("fix") || t.requestId <= 0L) return false
        return t.requestedBy.isEmpty() || t.requestedBy == me.username
    }

    /** The audio check's badge beside a song (library.rs `verify_badge`): (label, kind), or null for none. */
    fun verifyBadge(status: String): Pair<String, String>? = when (status) {
        "manual" -> "✓ confirmed" to "ok"
        "verified" -> "✓ verified" to "ok"
        "mismatch" -> "wrong audio" to "err"
        else -> null
    }

    /** A badge kind for a match score: ok above 85 %, warn above 60 %, else err. */
    fun matchKind(x: Double): String = if (x > 0.85) "ok" else if (x > 0.6) "warn" else "err"

    /** library.rs `round_pct`: 0.857 → 86. */
    fun pct(x: Double): Long = Math.round(x * 100.0)

    /**
     * A pasted release link as the id the server takes (library.rs `match_modal`): a MusicBrainz id as it is,
     * "deezer:N", "apple:N", "discogs:rN" / "discogs:mN"; null for anything else (then the names are searched).
     */
    fun releaseIdOf(link: String): String? {
        Regex("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", RegexOption.IGNORE_CASE).find(link)?.let { return it.value }
        Regex("deezer\\.com/(?:\\w+/)?album/(\\d+)").find(link)?.let { return "deezer:" + it.groupValues[1] }
        Regex("apple\\.com/.*?/album/(?:[^/]+/)?(?:id)?(\\d+)").find(link)?.let { return "apple:" + it.groupValues[1] }
        Regex("discogs\\.com/(?:[^/]+/)?(release|master)/(\\d+)").find(link)?.let { return "discogs:" + it.groupValues[1].first() + it.groupValues[2] }
        return null
    }
}
