package app.encore.android.features.settings

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import kotlinx.serialization.json.JsonObject

/** The words of Settings → Language that follow from GET /api/translate/status (translate.rs on the web). */
object LanguageRules {
    /**
     * Admins: how far the translations are ("3120 of 3400 titles have a translation"), with the titles left for the
     * local model only while it can run – installed, downloaded and switched on (the `model` source's `on`); otherwise
     * they would read as a queue that never moves. Null without the counts (members, older servers).
     */
    fun counts(st: JsonObject): String? {
        if (SettingsRules.nullish(st, "rows")) return null
        val model = st.arr("tiers").objectsOf().firstOrNull { it.str("id") == "model" }
        val waiting = if (st.bool("translator") && (model == null || model.bool("on"))) " · ${st.str("waiting").ifEmpty { "0" }} waiting for the local model" else ""
        return "${st.str("shown").ifEmpty { "0" }} of ${st.str("rows")} titles have a translation$waiting" + if (st.bool("running")) " · running now" else ""
    }
}
