package app.encore.android.features.library

import app.encore.android.data.api.SurroundRequest
import app.encore.android.data.api.SurroundState

/**
 * The rules of the Surround library (0.35, docs/contracts/surround.md; web-rs/src/pages/library.rs
 * `album_versions`, `atmos_button`), pure so the unit tests check them.
 */
object SurroundRules {
    /** Surround songs are numbered from a billion (encore_library::surround `BASE_ID`). */
    const val TRACK_BASE: Long = 1_000_000_000L

    fun isSurroundTrack(id: Long): Boolean = id >= TRACK_BASE

    /** Surround album ids are "s" + hex (stereo ones are hex only). */
    fun isSurroundAlbum(id: String): Boolean = id.startsWith("s")

    /** The album tile's corner badge: "Dolby Atmos", or the format ("E-AC-3 5.1"). */
    fun badge(atmos: Boolean, quality: String): String = if (atmos) "Dolby Atmos" else quality

    /** A stereo album's link to one of its surround versions: "Dolby Atmos version", "E-AC-3 5.1 version". */
    fun versionLabel(quality: String): String =
        (if (quality.contains("Atmos")) "Dolby Atmos" else quality.ifEmpty { "Surround" }) + " version"

    /** The chip of the request fetching a Dolby Atmos version. */
    fun requestLabel(r: SurroundRequest): String = "Dolby Atmos version: ${r.status.ifEmpty { "requested" }}"

    /** "Get Dolby Atmos version" on a stereo album: admins, when the server can fetch one and none is here or coming. */
    fun canFetch(s: SurroundState?, admin: Boolean): Boolean =
        admin && s != null && s.canFetch && s.have.isEmpty() && s.request == null

    fun requestedToast(requestId: Long): String =
        if (requestId > 0L) "Dolby Atmos version requested (#$requestId)" else "Dolby Atmos version requested"

    /** The toast kind for a refused request: 404 (Apple Music has no Atmos version) warns, the rest is an error. */
    fun errorKind(code: Int): String = if (code == 404) "warn" else "err"
}
