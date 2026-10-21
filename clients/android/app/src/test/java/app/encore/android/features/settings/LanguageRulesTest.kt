package app.encore.android.features.settings

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Settings → Language: the counts line, as the web's `counts_line`. */
class LanguageRulesTest {
    private fun status(translator: Boolean, modelOn: Boolean) = EncoreJson.parseToJsonElement("""{"translator": $translator, "rows": 3400, "shown": 3120, "waiting": 280,
        "tiers": [{"id": "manual", "setting": null, "on": true}, {"id": "model", "setting": "translate_local_model", "on": $modelOn}]}""") as JsonObject

    @Test
    fun theCountsSayTheModelOnlyWhenItRuns() {
        assertEquals("3120 of 3400 titles have a translation", LanguageRules.counts(status(translator = false, modelOn = true)))
        assertEquals("3120 of 3400 titles have a translation · 280 waiting for the local model", LanguageRules.counts(status(translator = true, modelOn = true)))
        assertEquals("3120 of 3400 titles have a translation", LanguageRules.counts(status(translator = true, modelOn = false)))
        assertNull("a member", LanguageRules.counts(EncoreJson.parseToJsonElement("""{"translator": true}""") as JsonObject))
    }
}
