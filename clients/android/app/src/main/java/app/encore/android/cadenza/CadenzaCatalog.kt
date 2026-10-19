package app.encore.android.cadenza

import app.encore.android.core.arr
import app.encore.android.core.parseJsonObject
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * The filters and dithers a Cadenza engine offers, as (id, label) pairs, with its defaults: the one reader of the
 * catalog document `{filters: [[id, label]…], dithers: [[id, label]…], default_filter, default_dither}` that both
 * the phone's engine ([phone], CadenzaEngine.catalog) and the server (GET /api/playback/dsp, DspInfo) send.
 */
class CadenzaCatalog(
    val filters: List<Pair<String, String>>,
    val dithers: List<Pair<String, String>>,
    /** null when the document has no default (an older server) */
    val defaultFilter: String? = null,
    val defaultDither: String? = null,
) {
    /** The filter's label; "" (the engine's default) is named after the default filter. */
    fun filterLabel(id: String): String = label(filters, id, defaultFilter)

    fun ditherLabel(id: String): String = label(dithers, id, defaultDither)

    private fun label(list: List<Pair<String, String>>, id: String, default: String?): String {
        list.firstOrNull { it.first == id }?.let { return it.second }
        if (id.isNotEmpty()) return id
        return list.firstOrNull { it.first == default }?.second ?: "Cadenza default"
    }

    companion object {
        val EMPTY = CadenzaCatalog(emptyList(), emptyList())

        /** The catalog in a document (the phone engine's, or the server's GET /api/playback/dsp). */
        fun parse(o: JsonObject?): CadenzaCatalog {
            if (o == null) return EMPTY
            fun pairs(k: String): List<Pair<String, String>> = o.arr(k)?.mapNotNull { e ->
                val a = e as? JsonArray ?: return@mapNotNull null
                val id = text(a.getOrNull(0)) ?: return@mapNotNull null
                id to (text(a.getOrNull(1)) ?: id)
            } ?: emptyList()
            return CadenzaCatalog(pairs("filters"), pairs("dithers"), text(o["default_filter"]), text(o["default_dither"]))
        }

        private fun text(e: Any?): String? = (e as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content

        /** The phone engine's catalog (empty without the library), read once. */
        val phone: CadenzaCatalog by lazy {
            val json = try { CadenzaEngine.catalog() } catch (_: Throwable) { "" }
            try { parse(parseJsonObject(json)) } catch (_: Exception) { EMPTY }
        }
    }
}
