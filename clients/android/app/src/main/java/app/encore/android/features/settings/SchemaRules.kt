package app.encore.android.features.settings

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import java.math.BigDecimal

/**
 * What the schema-drawn cards need without Android: the `when` rules, the text a field starts with, the number
 * parsing, the note under a field, the cards a server has, and what Save sends (only the keys that changed, in the
 * web's body shape). The web's renderer is web-rs/src/settings/schema_form.rs and fields.rs; the server's side is
 * server/crates/config (settings.toml, when.rs, `Config::schema`) and PUT /api/admin/settings.
 *
 * `values` is the answer of GET (or PUT) /api/admin/settings; `drafts` holds what the person typed and has not
 * saved yet, as text per key ([textFor] says how each type is written).
 */
object SchemaRules {
    /** What GET /api/admin/settings sends for a saved secret ("•••• (saved)"). */
    const val MASK = "••••"

    /** The secret that is never shown again, not even to the owner (the server refuses it too). */
    const val WIREGUARD_KEY = "vpn_wireguard_config"

    fun isMasked(v: String): Boolean = v.startsWith(MASK)

    // ---- visibility ------------------------------------------------------------------------------------------

    /** A parsed `when`: shown while the select [key] has [value] ([equal]), or while it has any other value. */
    data class When(val key: String, val value: String, val equal: Boolean) {
        fun shows(current: String): Boolean = (current == value) == equal
    }

    /** Reads `key=value` or `key!=value` (fields.rs `When::parse`); null for anything else (the field is always shown). */
    fun parseWhen(rule: String?): When? {
        if (rule.isNullOrBlank()) return null
        val ne = rule.indexOf("!=")
        val (key, value, equal) = if (ne >= 0) {
            Triple(rule.substring(0, ne), rule.substring(ne + 2), false)
        } else {
            val eq = rule.indexOf('=')
            if (eq < 0) return null
            Triple(rule.substring(0, eq), rule.substring(eq + 1), true)
        }
        val k = key.trim()
        val v = value.trim()
        if (k.isEmpty() || v.isEmpty()) return null
        return When(k, v, equal)
    }

    /**
     * Whether [field] shows now. [current] gives a setting's value as text, unsaved edits first, so a rule follows
     * the select as it changes. A field hidden by a rule is still saved with its card.
     */
    fun isShown(field: SchemaField, current: (String) -> String): Boolean {
        val w = parseWhen(field.rule) ?: return true
        return w.shows(current(w.key))
    }

    /** A setting's value as text: unsaved edits first, then the saved value ([textOf]). */
    fun current(key: String, values: JsonObject, drafts: Map<String, String>): String = drafts[key] ?: textOf(values[key])

    // ---- values as text --------------------------------------------------------------------------------------

    /**
     * A value as the web reads it into a form (json.rs `js_string`): text as it is, a whole number without ".0",
     * true/false, "" for null or a missing key.
     */
    fun textOf(v: JsonElement?): String = when (v) {
        null, is JsonNull -> ""
        is JsonPrimitive -> if (v.isString) v.content else numberText(v.content) ?: v.content
        else -> v.toString()
    }

    /** JavaScript truthiness of a value: null, false, 0, "" and NaN are false. */
    fun truthy(v: JsonElement?): Boolean = when (v) {
        null, is JsonNull -> false
        is JsonPrimitive -> when {
            v.isString -> v.content.isNotEmpty()
            v.content == "true" -> true
            v.content == "false" -> false
            else -> (v.content.toDoubleOrNull() ?: 0.0).let { it != 0.0 && !it.isNaN() }
        }
        else -> true
    }

    /** "2.0" → "2", "0.850" → "0.85", "1e3" → "1000"; null when [s] is not a number. */
    private fun numberText(s: String): String? {
        val d = s.toDoubleOrNull() ?: return null
        if (!d.isFinite()) return null
        if (d == Math.floor(d) && Math.abs(d) < 1e15) return d.toLong().toString()
        return try { BigDecimal(s).stripTrailingZeros().toPlainString() } catch (_: NumberFormatException) { d.toString() }
    }

    /**
     * The text a field starts with: "" for a secret (the input starts empty; empty keeps the saved value),
     * "true"/"false" for a switch, the comma-separated values of `checks`, and the value as text for the rest.
     */
    fun initialText(field: SchemaField, values: JsonObject): String = when {
        field.isSecret -> ""
        field.kind == "bool" -> if (truthy(values[field.key])) "true" else "false"
        else -> textOf(values[field.key])
    }

    /** How a draft is written for each type: a switch as "true"/"false", `checks` as [checksText], the rest as typed. */
    fun textFor(on: Boolean): String = if (on) "true" else "false"

    // ---- checks ----------------------------------------------------------------------------------------------

    /** Release kinds and extras compare without case, spaces and punctuation: a saved "Spoken word" ticks "Spokenword". */
    fun typeKey(s: String): String = s.lowercase().filter { it in 'a'..'z' || it in '0'..'9' }

    /** The option values of [field] that [text] (comma-separated) ticks. */
    fun ticked(field: SchemaField, text: String): Set<String> {
        val on = text.split(',').map { typeKey(it) }.filter { it.isNotEmpty() }.toSet()
        return field.options.map { it.value }.filter { typeKey(it) in on }.toSet()
    }

    /** The comma-separated values Save sends for the ticked [values], in the options' order (the web's `on.join(",")`). */
    fun checksText(field: SchemaField, values: Set<String>): String =
        field.options.map { it.value }.filter { it in values }.joinToString(",")

    /** [text] with [value] ticked or not. */
    fun toggle(field: SchemaField, text: String, value: String, on: Boolean): String {
        val now = ticked(field, text).toMutableSet()
        if (on) now.add(value) else now.remove(value)
        return checksText(field, now)
    }

    // ---- numbers ---------------------------------------------------------------------------------------------

    /**
     * A number as typed: spaces around it are fine, and a decimal comma (a German keyboard's "0,5") reads as a point.
     * null for "", for anything that is not a number, and for NaN or infinity.
     */
    fun parseNumber(text: String): Double? {
        var t = text.trim()
        if (t.isEmpty()) return null
        if (',' in t && '.' !in t && t.count { it == ',' } == 1) t = t.replace(',', '.')
        if (!t.all { it.isDigit() || it == '.' || it == '-' || it == '+' || it == 'e' || it == 'E' }) return null
        val d = t.toDoubleOrNull() ?: return null
        return if (d.isFinite()) d else null
    }

    /** "Enter a number" when [text] is neither empty (the setting's default) nor a number; null when it is fine. */
    fun numberError(text: String): String? = if (text.isBlank() || parseNumber(text) != null) null else "Enter a number"

    /**
     * A number outside the field's usual range, said under the field (the server takes it anyway: min and max are a
     * hint); null when it is inside, empty or not a number.
     */
    fun rangeNote(field: SchemaField, text: String): String? {
        val d = parseNumber(text) ?: return null
        val lo = field.min
        val hi = field.max
        val outside = (lo != null && d < lo) || (hi != null && d > hi)
        if (!outside) return null
        return when {
            lo != null && hi != null -> "Usually ${plain(lo)} to ${plain(hi)}"
            lo != null -> "Usually ${plain(lo)} or more"
            else -> "Usually ${plain(hi!!)} or less"
        }
    }

    /** A number as people read it: 65535, 0.85. */
    fun plain(d: Double): String = numberText(d.toString()) ?: d.toString()

    /** A number as JSON: whole numbers without a fraction (65535, not 65535.0), like the server's own answer. */
    fun numberJson(d: Double): JsonPrimitive =
        if (d == Math.floor(d) && Math.abs(d) < 1e15) JsonPrimitive(d.toLong()) else JsonPrimitive(d)

    /**
     * Whether the number keyboard needs a point or a minus: a step below 1, a fractional default (1.0 is sent as a
     * float for a float setting), or a range that goes below 0.
     */
    fun needsDecimalKeys(field: SchemaField): Boolean =
        (field.step != null && field.step < 1.0) || (field.min != null && field.min < 0.0) ||
            ((field.default as? JsonPrimitive)?.takeIf { !it.isString }?.content?.contains('.') == true)

    // ---- where a value comes from ----------------------------------------------------------------------------

    private fun meta(values: JsonObject, key: String): JsonObject? = (values["_meta"] as? JsonObject)?.get(key) as? JsonObject

    /** `_meta.<key>.source`: "env", "ui" or "default" ("" when the server didn't say). */
    fun source(values: JsonObject, key: String): String = textOf(meta(values, key)?.get("source"))

    /** `_meta.<key>.env`: the environment variable that sets [key] ("" when the server didn't say). */
    fun envVar(values: JsonObject, key: String): String = textOf(meta(values, key)?.get("env"))

    /** The environment variable that sets [field] ("" for none): the values' `_meta`, else the schema's. */
    fun envOf(field: SchemaField, values: JsonObject): String = textOf(meta(values, field.key)?.get("env")).ifEmpty { field.env }

    /** An environment variable sets [field]: shown disabled, never sent. */
    fun isLocked(field: SchemaField, values: JsonObject): Boolean = field.envLocked || source(values, field.key) == "env"

    /** A secret with a saved value (the server says "•••• (saved)"). */
    fun isSaved(field: SchemaField, values: JsonObject): Boolean = truthy(values[field.key])

    /** The note under a field (schema_form.rs `source_note`); the words are drawn by SchemaForm. */
    enum class Note {
        /** "🔒 Set by ENV in your compose/env – change it there (environment always wins)" */
        LOCKED,
        /** "Saved ✓ (hidden) · Clear" */
        SAVED_SECRET,
        /** "Not set", with " – or set ENV in your compose file" when there is a variable */
        UNSET_SECRET,
        /** "Default · env var ENV" */
        DEFAULT_ENV,
        NONE,
    }

    fun note(field: SchemaField, values: JsonObject): Note = when {
        isLocked(field, values) -> Note.LOCKED
        field.isSecret && isSaved(field, values) -> Note.SAVED_SECRET
        field.isSecret -> Note.UNSET_SECRET
        source(values, field.key) == "ui" -> Note.NONE
        envOf(field, values).isNotEmpty() -> Note.DEFAULT_ENV
        else -> Note.NONE
    }

    /** A password field's placeholder (schema_form.rs). */
    fun passwordPlaceholder(field: SchemaField, values: JsonObject): String = when {
        isLocked(field, values) -> "set by environment"
        isSaved(field, values) -> "saved – paste a new value to replace"
        else -> "not set"
    }

    /** A secret text area's placeholder. */
    fun textareaPlaceholder(field: SchemaField, values: JsonObject): String =
        if (field.isSecret && isSaved(field, values)) "Saved – paste to replace" else ""

    /**
     * Show (and Copy) for a saved secret: when the server lets its owner see saved secrets ([canReveal]: the
     * "reveal" feature of /api/auth/info, for the owner), the field has a value, and it is not the WireGuard config.
     * The web shows them for values set by environment variables too (read-only, but viewable).
     */
    fun canShow(field: SchemaField, values: JsonObject, canReveal: Boolean): Boolean =
        canReveal && field.isSecret && isSaved(field, values) && field.key != WIREGUARD_KEY

    // ---- the cards a server has ------------------------------------------------------------------------------

    /** The fields of [group] this server has (a key of the values); the web leaves out the others. */
    fun visibleFields(group: SchemaGroup, values: JsonObject): List<SchemaField> = group.fields.filter { values.containsKey(it.key) }

    /** The cards of a page this server has, each with its fields; a card with none left is left out. */
    fun visibleCards(section: SchemaSection, values: JsonObject): List<Pair<SchemaGroup, List<SchemaField>>> =
        section.groups.map { it to visibleFields(it, values) }.filter { it.second.isNotEmpty() }

    /**
     * A card's fields in the order they are drawn: the `vpn_` fields (Soulseek's VPN) as a block of their own at the
     * place of the first one, the others before and after it (schema_form.rs `fields_html`).
     */
    data class Layout(val before: List<SchemaField>, val vpn: List<SchemaField>, val after: List<SchemaField>)

    fun layout(fields: List<SchemaField>): Layout {
        val isVpn = { f: SchemaField -> f.key.startsWith("vpn_") }
        val at = fields.indexOfFirst(isVpn)
        if (at < 0) return Layout(fields, emptyList(), emptyList())
        val rest = fields.filterNot(isVpn)
        val cut = minOf(at, rest.size)
        return Layout(rest.subList(0, cut), fields.filter(isVpn), rest.subList(cut, rest.size))
    }

    /** Whether a card has advanced fields (they wait behind "Show advanced"). */
    fun hasAdvanced(fields: List<SchemaField>): Boolean = fields.any { it.advanced }

    // ---- Save ------------------------------------------------------------------------------------------------

    /**
     * What Save sends for [field], or null when it sends nothing for it: nothing typed, the value unchanged, an
     * environment variable sets it, an empty secret (keeps the saved one), a mask the server sent, or the secret
     * that Show put in the field, unchanged ([revealed]). The web's body shape (schema_form.rs `form_values`): a
     * switch as true/false, `checks` as comma-separated values, a secret trimmed, text, selects and paths as text,
     * and a number as a JSON number (the server reads "5" and 5 alike; an empty number field sends "", which the
     * server reads as the default, like the web). A number that doesn't parse sends nothing ([numberError] says so).
     */
    fun changedValue(field: SchemaField, values: JsonObject, text: String?, revealed: String? = null): JsonElement? {
        if (text == null || isLocked(field, values)) return null
        val saved = values[field.key]
        if (field.isSecret) {
            val t = text.trim()
            if (t.isEmpty() || isMasked(t)) return null
            if (revealed != null && t == revealed.trim()) return null
            return JsonPrimitive(t)
        }
        if (field.kind != "bool" && isMasked(text)) return null
        return when (field.kind) {
            "bool" -> {
                val on = text == "true"
                if (on == truthy(saved)) null else JsonPrimitive(on)
            }
            "checks" -> {
                val was = textOf(saved).split(',').map { typeKey(it) }.filter { it.isNotEmpty() }.toSet()
                val now = text.split(',').map { typeKey(it) }.filter { it.isNotEmpty() }.toSet()
                if (was == now) null else JsonPrimitive(text)
            }
            "number" -> {
                val t = text.trim()
                if (t.isEmpty()) return if (textOf(saved).isEmpty()) null else JsonPrimitive("")
                val d = parseNumber(t) ?: return null
                if (d == parseNumber(textOf(saved))) null else numberJson(d)
            }
            else -> if (text == textOf(saved)) null else JsonPrimitive(text)
        }
    }

    /** The body of Save for a card: only the keys of [fields] that changed ([changedValue]), hidden ones included. */
    fun changes(
        fields: List<SchemaField>,
        values: JsonObject,
        drafts: Map<String, String>,
        revealed: Map<String, String> = emptyMap(),
    ): JsonObject {
        val out = LinkedHashMap<String, JsonElement>()
        for (f in fields) {
            val v = changedValue(f, values, drafts[f.key], revealed[f.key]) ?: continue
            out[f.key] = v
        }
        return JsonObject(out)
    }

    /** The number fields of a card that hold something that is not a number (Save waits until they are fixed). */
    fun invalid(fields: List<SchemaField>, values: JsonObject, drafts: Map<String, String>): List<SchemaField> =
        fields.filter { f -> f.kind == "number" && !isLocked(f, values) && drafts[f.key]?.let { numberError(it) } != null }

    /** The answer of PUT /api/admin/settings without its `_warnings`: the values from now on. */
    fun withoutWarnings(answer: JsonObject): JsonObject = JsonObject(answer.filterKeys { it != "_warnings" })

    /** The `_warnings` of a PUT answer, as lines under the card (a folder Encore can't write to, with the chown line). */
    fun warnings(answer: JsonObject): List<ResultLine> =
        (answer["_warnings"] as? JsonArray).orEmpty()
            .mapNotNull { (it as? JsonObject)?.get("message")?.let { m -> textOf(m) } }
            .filter { it.isNotEmpty() }
            .map { ResultLine(false, it) }

    /** A Test's answer `{ok, message}` as its line ("Connected" / "Failed" when it has no message), as mod.rs `test_result`. */
    fun testLine(answer: JsonObject): ResultLine {
        val ok = truthy(answer["ok"])
        val msg = answer["message"].let { if (truthy(it)) textOf(it) else if (ok) "Connected" else "Failed" }
        return ResultLine(ok, msg)
    }

    /** A Test that did not get an answer: the error's words, or "Test failed". */
    fun testFailed(message: String?): ResultLine = ResultLine(false, message?.takeIf { it.isNotBlank() } ?: "Test failed")

    /** The Test button's words: the schema's `test_label` (the web's old default when a server leaves it out). */
    fun testLabel(group: SchemaGroup): String = group.testLabel.ifEmpty { "Test connection" }

    /** The label a select shows for [value]: its option's label, or the value itself when it is not one of them. */
    fun optionLabel(field: SchemaField, value: String): String =
        field.options.firstOrNull { it.value == value }?.label ?: value
}
