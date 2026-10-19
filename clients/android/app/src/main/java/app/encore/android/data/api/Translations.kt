package app.encore.android.data.api

import android.content.Context
import android.content.SharedPreferences
import app.encore.android.core.str
import kotlinx.serialization.json.JsonObject

/**
 * Translated titles (Settings → Language on the web, account prefs `translate_mode`): the server adds
 * `title_t` / `title_r` (translation / romanization) and the same for `album`, `artist`, `albumartist`
 * and an artist's `name`; this picks what to show, with the same rules as web/js/ui.js (tr, trSub,
 * trInline). The mode is kept on the phone too, so the car shows it before the app has asked the server.
 */
object Translations {
    @Volatile
    var mode: String = "off"
        private set
    private var store: SharedPreferences? = null

    fun init(context: Context) {
        val p = context.getSharedPreferences("encore_translations", Context.MODE_PRIVATE)
        store = p
        mode = clean(p.getString("mode", "off"))
    }

    /** The account's prefs (GET /api/auth/me → prefs). */
    fun adopt(prefs: JsonObject?) {
        val m = clean(prefs?.str("translate_mode"))
        if (m != mode) {
            mode = m
            store?.edit()?.putString("mode", m)?.apply()
        }
    }

    fun clear() {
        mode = "off"
        store?.edit()?.remove("mode")?.apply()
    }

    private fun clean(m: String?): String = when (m) { "under", "instead", "romanized" -> m; else -> "off" }

    /** `tr`: the original, its translation (instead; the romanization when there is none) or its romanization. */
    fun shown(original: String, t: Tr): String = when (mode) {
        "instead" -> t.t.ifEmpty { t.r }.ifEmpty { original }
        "romanized" -> t.r.ifEmpty { original }
        else -> original
    }

    /** `trSub`: the line under a title when the translation shows under the original (null otherwise). */
    fun under(t: Tr): String? = if (mode == "under") t.t.ifEmpty { t.r }.ifEmpty { null } else null

    /** `trInline`: "Original (Translation)" under the original, else as [shown]. */
    fun inline(original: String, t: Tr): String {
        if (mode != "under") return shown(original, t)
        val x = t.t.ifEmpty { t.r }
        return if (x.isEmpty()) original else "$original ($x)"
    }
}

/** A field's translation and romanization (empty: none). */
data class Tr(val t: String = "", val r: String = "") {
    val isEmpty: Boolean get() = t.isEmpty() && r.isEmpty()

    companion object {
        val NONE = Tr()

        /** `<field>_t` / `<field>_r` of a server row. */
        fun of(o: JsonObject, field: String): Tr = Tr(o.str(field + "_t"), o.str(field + "_r"))
    }
}
