package app.encore.android.features.settings

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.PersistableBundle
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.ui.components.CopyIconButton
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// Settings → Server, drawn from the server's settings schema (SchemaModels.kt) like the web's renderer
// (web-rs/src/settings/schema_form.rs for the fields, groups.rs for the cards): each page's cards in the schema's
// order under the heading of their part, each card with its title, description and Test button, the result line,
// the fields and Save. A switch for `bool`, check boxes for `checks`, a drop-down for `select`, a text area, a
// password input that starts empty (Show and Copy for the server's owner), and a text or number input for the rest
// (`path` and `url` are text inputs with the keyboard for them). Under each field: its help line and where its value
// comes from. A field with a `when` rule shows only while its select has the value, unsaved choices included.
// Save sends only the keys that changed (SchemaRules.changes); Test saves first, then asks the server.

/**
 * What the Server pages share while Settings is open (the web's `Hub`): the schema, the values of
 * GET /api/admin/settings, and what was typed and not saved yet. Make it once for the Settings screen
 * ([SchemaApi.loadState]) so edits on a card survive going to another page and back.
 *
 * [canReveal]: Show and Copy for saved secrets. The route (GET /api/admin/settings/secret/{key}) is the server
 * owner's: pass `"reveal" in /api/auth/info features && the account's role is "owner"`.
 *
 * [target]: where Save and clear send the keys instead of PUT /api/admin/settings, answering the values from then
 * on (a plugin's own settings: PUT /api/plugins/{id}/settings, PluginsScreen.kt). Blocking; run on Dispatchers.IO.
 */
@Stable
class SchemaFormState(
    val schema: SettingsSchema,
    values: JsonObject,
    val canReveal: Boolean,
    private val target: ((JsonObject) -> JsonObject)? = null,
) {
    /** GET /api/admin/settings, then each Save's answer. */
    var values: JsonObject by mutableStateOf(SchemaRules.withoutWarnings(values))
        private set

    /** What was typed per key and not saved yet (SchemaRules says how each type is written). */
    val drafts = mutableStateMapOf<String, String>()

    /** Secrets fetched with Show, per key: sent again only when they were changed. */
    val revealed = mutableStateMapOf<String, String>()

    /** Password fields shown as text. */
    internal val visible = mutableStateMapOf<String, Boolean>()

    /** The lines under each card (by card id): the Test result, or the warnings Save came back with. */
    val results = mutableStateMapOf<String, List<ResultLine>>()

    internal val testing = mutableStateMapOf<String, Boolean>()
    internal val saving = mutableStateMapOf<String, Boolean>()
    internal val advanced = mutableStateMapOf<String, Boolean>()

    /** The text a field shows: what was typed, else the saved value ([SchemaRules.initialText]). */
    fun text(field: SchemaField): String = drafts[field.key] ?: SchemaRules.initialText(field, values)

    /** A setting's value as text, unsaved edits first (for the `when` rules). */
    fun current(key: String): String = SchemaRules.current(key, values, drafts)

    fun edit(key: String, text: String) {
        drafts[key] = text
    }

    /** The fields of [group] this server has. */
    fun fieldsOf(group: SchemaGroup): List<SchemaField> = SchemaRules.visibleFields(group, values)

    /** What Save would send for [group] now. */
    fun changes(group: SchemaGroup): JsonObject = SchemaRules.changes(fieldsOf(group), values, drafts, revealed)

    /** Values that changed elsewhere (a card's extras saved something, a plugin card, a reload): unsaved edits stay. */
    fun replaceValues(answer: JsonObject) {
        values = SchemaRules.withoutWarnings(answer)
    }

    /** After a Save of [fields]: the answer is the values from now on, and their edits are done. */
    private fun saved(answer: JsonObject, fields: List<SchemaField>) {
        values = SchemaRules.withoutWarnings(answer)
        for (f in fields) {
            drafts.remove(f.key)
            revealed.remove(f.key)
            visible.remove(f.key)
        }
    }

    /**
     * Save of a card, and the quiet save before Test (groups.rs `Hub::save`): PUT /api/admin/settings with the keys
     * that changed, plus [extra] (a card's own keys, like Cadenza's engine settings). The answer becomes the values;
     * a folder Encore can't write to comes back as a line under the card. Errors are toasted. Returns null when it
     * saved (or there was nothing to save), else the error's words. [onSaved] runs after a PUT that carried [extra]
     * worked (the card's extras take what they sent as saved).
     */
    suspend fun save(api: SchemaApi, group: SchemaGroup, quiet: Boolean, extra: JsonObject? = null, onSaved: (() -> Unit)? = null): String? {
        val fields = fieldsOf(group)
        val bad = SchemaRules.invalid(fields, values, drafts)
        if (bad.isNotEmpty()) {
            val msg = "${bad.first().label}: Enter a number"
            Toasts.show(msg, "err")
            return msg
        }
        val body = LinkedHashMap<String, JsonElement>(changes(group))
        if (extra != null) body.putAll(extra)
        if (body.isEmpty()) {
            if (!quiet) Toasts.show("Settings saved", "ok")
            return null
        }
        saving[group.id] = true
        try {
            val answer = withContext(Dispatchers.IO) { send(api, JsonObject(body)) }
            saved(answer, fields)
            if (extra != null) onSaved?.invoke()
            val warnings = SchemaRules.warnings(answer)
            if (warnings.isNotEmpty()) results[group.id] = warnings
            if (!quiet) Toasts.show("Settings saved", "ok")
            return null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            val msg = e.message ?: "Something went wrong"
            Toasts.show(msg, "err")
            return msg
        } finally {
            saving.remove(group.id)
        }
    }

    /** The keys of a Save or a clear, to [target] or PUT /api/admin/settings: the values from then on. */
    private fun send(api: SchemaApi, body: JsonObject): JsonObject = target?.invoke(body) ?: api.save(body)

    /** Test (groups.rs): saves the card quietly, then POST /api/system/test/<test>; the answer is the line under it. */
    suspend fun test(api: SchemaApi, group: SchemaGroup, extra: JsonObject? = null, onSaved: (() -> Unit)? = null) {
        if (group.test.isEmpty() || testing[group.id] == true) return
        testing[group.id] = true
        results.remove(group.id)
        try {
            val failed = save(api, group, quiet = true, extra = extra, onSaved = onSaved)
            results[group.id] = listOf(
                if (failed != null) {
                    SchemaRules.testFailed(failed)
                } else {
                    try {
                        SchemaRules.testLine(withContext(Dispatchers.IO) { api.test(group.test) })
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        SchemaRules.testFailed(e.message)
                    }
                },
            )
        } finally {
            testing.remove(group.id)
        }
    }

    /** "clear" under a saved secret: PUT {key: ""} (the web's `data-clear`). */
    suspend fun clear(api: SchemaApi, key: String) {
        try {
            val answer = withContext(Dispatchers.IO) { send(api, JsonObject(mapOf(key to JsonPrimitive("")))) }
            values = SchemaRules.withoutWarnings(answer)
            drafts.remove(key)
            revealed.remove(key)
            visible.remove(key)
            Toasts.show("Cleared", "ok")
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Something went wrong", "err")
        }
    }

    /**
     * Show / Hide of a saved secret: Show fills an empty field with the saved value (asked for only now) and shows it
     * as text; Hide hides it again and empties the field when it still holds what Show put there.
     */
    suspend fun toggleShown(api: SchemaApi, field: SchemaField) {
        val key = field.key
        if (visible[key] == true) {
            hide(key)
            return
        }
        if (text(field).isEmpty()) {
            try {
                val v = withContext(Dispatchers.IO) { api.secret(key) }
                revealed[key] = v
                drafts[key] = v
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
                return
            }
        }
        visible[key] = true
    }

    /**
     * Hide of a secret shown as text (also when its field leaves the screen: another page, the card closed): the
     * field is emptied when it still holds what Show put there; what was typed over it stays, hidden.
     */
    fun hide(key: String) {
        if (visible[key] != true) return
        val r = revealed[key]
        if (r != null && drafts[key] == r) drafts.remove(key)
        revealed.remove(key)
        visible.remove(key)
    }

    /** Copy: what the field holds, else the saved secret (asked for now). */
    suspend fun copy(api: SchemaApi, field: SchemaField, context: Context) {
        val typed = text(field)
        val value = typed.ifEmpty {
            try {
                withContext(Dispatchers.IO) { api.secret(field.key) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
                return
            }
        }
        copySecret(context, value)
    }
}

/** Loads the schema and the values (the web asks for both when Settings opens). Blocking: run it on Dispatchers.IO. */
fun SchemaApi.loadState(canReveal: Boolean): SchemaFormState = SchemaFormState(schema(), values(), canReveal)

/** Puts a secret on the clipboard, marked sensitive so the keyboard's clipboard preview hides it (Android 13+). */
private fun copySecret(context: Context, text: String) {
    try {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: throw IllegalStateException()
        val clip = ClipData.newPlainText("Encore", text)
        // ClipDescription.EXTRA_IS_SENSITIVE (API 33); older versions ignore the extra
        clip.description.extras = PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
        cm.setPrimaryClip(clip)
        Toasts.show("Copied to clipboard", "ok")
    } catch (_: Exception) {
        Toasts.show("Copy failed – select and copy manually", "err")
    }
}

// ---- what a card shows besides its fields ----------------------------------------------------------------------

/**
 * A card's own extras, drawn by the Settings sections and not by the schema (the web's apps.rs `card_body`): the
 * YouTube block above the fields, Tidal sign-in and the streamrip config under streamrip's, the pre-render cache
 * and engine settings under Cadenza's, the engine's status under Soulseek's.
 *
 * - [above] / [below]: drawn above the fields / between the fields and Save;
 * - [vpnFooter]: under the VPN block's fields (the tunnel's state and Test VPN; [SchemaCardScope.save] saves first);
 * - [extraBody]: more keys for Save and the save before Test (Cadenza's `cadenza_engine_settings`), null for none;
 * - [afterSave]: runs after a Save that carried [extraBody]'s keys worked (what was sent is saved now);
 * - [hide]: hides a field by a rule of the card's own, read live from the unsaved values (Soulseek's listen port
 *   while the VPN is on: the VPN's forwarded port is used then).
 */
class CardSlots(
    val above: (@Composable ColumnScope.(SchemaCardScope) -> Unit)? = null,
    val below: (@Composable ColumnScope.(SchemaCardScope) -> Unit)? = null,
    val vpnFooter: (@Composable ColumnScope.(SchemaCardScope) -> Unit)? = null,
    val extraBody: (() -> JsonObject?)? = null,
    val afterSave: (() -> Unit)? = null,
    val hide: ((field: SchemaField, current: (String) -> String) -> Boolean)? = null,
) {
    companion object {
        val None = CardSlots()
    }
}

/** What a card's extras can do: save the card (quietly before their own test), and set the lines under it. */
class SchemaCardScope internal constructor(
    val group: SchemaGroup,
    val state: SchemaFormState,
    private val api: SchemaApi,
    private val scope: CoroutineScope,
    private val extra: () -> JsonObject?,
    private val afterSave: (() -> Unit)? = null,
) {
    /** Saves the card (quiet: no "Settings saved" toast); true when it saved. */
    suspend fun save(quiet: Boolean = true): Boolean = state.save(api, group, quiet, extra(), afterSave) == null

    /** Runs [block] in the card's scope (for buttons). */
    fun launch(block: suspend CoroutineScope.() -> Unit) {
        scope.launch(block = block)
    }

    /** The lines under the card's title (a result, a warning); empty to clear them. */
    fun show(lines: List<ResultLine>) {
        if (lines.isEmpty()) state.results.remove(group.id) else state.results[group.id] = lines
    }
}

// ---- a page ------------------------------------------------------------------------------------------------------

/**
 * The cards of one Server page ([section]) that this server has, under the heading of their part, as groups.rs
 * `cards_html`: the first card starts open (or [focus], the card a link asked for; a page with one card has it open).
 * [partTop] draws what a part starts with under its heading; [slots]
 * gives a card's extras. The caller scrolls (put it in a scrolling Column) and draws what the page has of its own
 * (the task list, How requests are fulfilled, the API key and About).
 */
@Composable
fun SchemaPage(
    state: SchemaFormState,
    api: SchemaApi,
    section: SchemaSection,
    modifier: Modifier = Modifier,
    focus: String? = null,
    partTop: @Composable ColumnScope.(part: String) -> Unit = {},
    slots: (SchemaGroup) -> CardSlots = { CardSlots.None },
) {
    val cards = SchemaRules.visibleCards(section, state.values)
    // a secret shown as text keeps the window out of screenshots and the Recents thumbnail
    SecureWindow(state.visible.values.any { it })
    val open = remember(section.id, focus) {
        mutableStateMapOf<String, Boolean>().apply {
            val first = cards.firstOrNull()?.first?.id
            val want = focus?.takeIf { f -> cards.any { it.first.id == f } } ?: first
            if (want != null) put(want, true)
        }
    }
    Column(modifier.fillMaxWidth()) {
        var part: String? = null
        for ((group, fields) in cards) {
            if (part != group.part) {
                part = group.part
                if (group.part.isNotEmpty()) {
                    Text(
                        group.part.uppercase(),
                        style = MaterialTheme.typography.labelSmall,
                        fontWeight = FontWeight.SemiBold,
                        color = MaterialTheme.palette.muted,
                        modifier = Modifier.padding(start = 4.dp, end = 4.dp, top = 18.dp, bottom = 8.dp),
                    )
                }
                partTop(group.part)
            }
            key(group.id) {
                SchemaCard(
                    state = state,
                    api = api,
                    group = group,
                    fields = fields,
                    open = open[group.id] == true,
                    onOpen = { open[group.id] = it },
                    slots = slots(group),
                )
            }
            Spacer(Modifier.height(12.dp))
        }
    }
}

/**
 * One card (groups.rs `card_html`): a drop-down with its title, description and Test button, then the result
 * line, the extras above, the fields, Show advanced, the extras below and Save.
 */
@Composable
fun SchemaCard(
    state: SchemaFormState,
    api: SchemaApi,
    group: SchemaGroup,
    fields: List<SchemaField> = state.fieldsOf(group),
    open: Boolean,
    onOpen: (Boolean) -> Unit,
    slots: CardSlots = CardSlots.None,
    modifier: Modifier = Modifier,
) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val extra: () -> JsonObject? = { slots.extraBody?.invoke() }
    val afterSave = slots.afterSave
    val cardScope = remember(group, state, api, scope, slots) { SchemaCardScope(group, state, api, scope, extra, afterSave) }
    val shape = RoundedCornerShape(14.dp)
    val testing = state.testing[group.id] == true
    Column(modifier.fillMaxWidth().clip(shape).background(p.panel).border(1.dp, p.border, shape)) {
        // the summary: title and description, Test, the chevron
        Row(
            Modifier.fillMaxWidth().clickable { onOpen(!open) }.padding(14.dp),
            verticalAlignment = Alignment.Top,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text(group.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                if (group.desc.isNotEmpty()) {
                    Text(group.desc, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 4.dp))
                }
            }
            if (group.test.isNotEmpty()) {
                SmallButton(SchemaRules.testLabel(group), iconRes = R.drawable.ic_refresh, enabled = !testing) {
                    onOpen(true)
                    scope.launch { state.test(api, group, extra(), afterSave) }
                }
            }
            Icon(
                painterResource(R.drawable.ic_expand_more),
                contentDescription = null,
                tint = p.muted,
                modifier = Modifier.padding(top = 2.dp).rotate(if (open) 180f else 0f),
            )
        }
        if (!open) return@Column
        Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 14.dp)) {
            ResultLines(testing, state.results[group.id].orEmpty())
            slots.above?.invoke(this, cardScope)
            SchemaFields(state, api, group, fields, slots, cardScope)
            AdvancedToggle(state, group, fields)
            slots.below?.invoke(this, cardScope)
            Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.End) {
                EncoreButton("Save", iconRes = R.drawable.ic_check, primary = true, enabled = state.saving[group.id] != true && !testing) {
                    scope.launch { state.save(api, group, quiet = false, extra = extra(), onSaved = afterSave) }
                }
            }
        }
    }
}

/**
 * The fields of a card (or of one run of a plugin card, PluginsScreen.kt), each shown by its rules: advanced ones
 * after Show advanced, `when` rules, the card's own [CardSlots.hide]; the `vpn_` fields in the VPN block with the
 * card's [CardSlots.vpnFooter].
 */
@Composable
fun ColumnScope.SchemaFields(
    state: SchemaFormState,
    api: SchemaApi,
    group: SchemaGroup,
    fields: List<SchemaField>,
    slots: CardSlots,
    cardScope: SchemaCardScope,
) {
    val showAdvanced = state.advanced[group.id] == true
    val shown = { f: SchemaField ->
        (!f.advanced || showAdvanced) && SchemaRules.isShown(f) { k -> state.current(k) } && slots.hide?.invoke(f) { k -> state.current(k) } != true
    }
    val layout = SchemaRules.layout(fields)
    // keyed, so a field's own state (an open drop-down) stays with it when a rule hides the one above
    for (f in layout.before) if (shown(f)) key(f.key) { SchemaFieldView(state, api, f) }
    if (layout.vpn.isNotEmpty()) {
        VpnBlock {
            for (f in layout.vpn) if (shown(f)) key(f.key) { SchemaFieldView(state, api, f) }
            slots.vpnFooter?.invoke(this, cardScope)
        }
    }
    for (f in layout.after) if (shown(f)) key(f.key) { SchemaFieldView(state, api, f) }
}

/** Show advanced / Hide advanced under a card's fields, when it has advanced ones. */
@Composable
fun AdvancedToggle(state: SchemaFormState, group: SchemaGroup, fields: List<SchemaField>) {
    if (!SchemaRules.hasAdvanced(fields)) return
    val showAdvanced = state.advanced[group.id] == true
    Text(
        if (showAdvanced) "Hide advanced" else "Show advanced",
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.palette.accent,
        modifier = Modifier
            .padding(top = 6.dp)
            .clip(RoundedCornerShape(6.dp))
            .clickable { state.advanced[group.id] = !showAdvanced }
            .padding(vertical = 6.dp, horizontal = 2.dp),
    )
}

/** The VPN for Soulseek block around the `vpn_` fields (apps.rs `vpn_block`). */
@Composable
private fun VpnBlock(content: @Composable ColumnScope.() -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(10.dp)
    Column(
        Modifier.fillMaxWidth().padding(vertical = 8.dp).clip(shape).background(p.panel2).border(1.dp, p.border, shape).padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Text("VPN for Soulseek", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
        content()
    }
}

/** The lines under a card's title: "Testing…" while a Test runs, then its result (mod.rs `test_result`). */
@Composable
internal fun ResultLines(testing: Boolean, lines: List<ResultLine>) {
    val p = MaterialTheme.palette
    if (testing) {
        Row(Modifier.padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            CircularProgressIndicator(Modifier.size(14.dp), color = p.accent, trackColor = p.border, strokeWidth = 2.dp)
            Spacer(Modifier.width(8.dp))
            Text("Testing…", style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        return
    }
    for (l in lines) {
        val tone = if (l.ok) EncoreColors.Ok else EncoreColors.Err
        val shape = RoundedCornerShape(8.dp)
        Row(
            Modifier
                .fillMaxWidth()
                .padding(bottom = 8.dp)
                .clip(shape)
                .background(tone.copy(alpha = 0.10f))
                .border(1.dp, tone.copy(alpha = 0.35f), shape)
                .padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Icon(painterResource(if (l.ok) R.drawable.ic_check else R.drawable.ic_close), contentDescription = null, tint = tone, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(8.dp))
            // a failed test's whole text is selectable and has Copy (settings::test_result)
            if (l.ok) {
                Text(l.message, style = MaterialTheme.typography.bodySmall)
            } else {
                SelectionContainer(Modifier.weight(1f)) { Text(l.message, style = MaterialTheme.typography.bodySmall) }
                CopyIconButton(l.message, size = 24.dp)
            }
        }
    }
}

// ---- a field -----------------------------------------------------------------------------------------------------

/** One field with its help line and the note on where its value comes from (schema_form.rs `field_html`). */
@Composable
fun SchemaFieldView(state: SchemaFormState, api: SchemaApi, field: SchemaField, modifier: Modifier = Modifier) {
    val values = state.values
    val locked = SchemaRules.isLocked(field, values)
    Column(modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        when (field.kind) {
            "bool" -> BoolField(state, field, locked)
            "checks" -> ChecksField(state, field, locked)
            "select" -> SelectField(state, field, locked)
            else -> TextLikeField(state, api, field, locked)
        }
        if (field.help.isNotEmpty()) FieldHelp(field.help)
        SourceNote(state, api, field)
    }
}

@Composable
private fun FieldHelp(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(top = 4.dp))
}

@Composable
private fun BoolField(state: SchemaFormState, field: SchemaField, locked: Boolean) {
    val on = state.text(field) == "true"
    Row(
        Modifier
            .fillMaxWidth()
            .clickable(enabled = !locked) { state.edit(field.key, SchemaRules.textFor(!on)) }
            .padding(vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Switch(checked = on, onCheckedChange = { state.edit(field.key, SchemaRules.textFor(it)) }, enabled = !locked)
        Spacer(Modifier.width(12.dp))
        Text(field.label, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f).alpha(if (locked) 0.6f else 1f))
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun ChecksField(state: SchemaFormState, field: SchemaField, locked: Boolean) {
    val text = state.text(field)
    val ticked = SchemaRules.ticked(field, text)
    Text(field.label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted)
    FlowRow(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        for (o in field.options) {
            val on = o.value in ticked
            Row(
                Modifier.clickable(enabled = !locked) { state.edit(field.key, SchemaRules.toggle(field, text, o.value, !on)) },
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Checkbox(checked = on, onCheckedChange = { state.edit(field.key, SchemaRules.toggle(field, text, o.value, it)) }, enabled = !locked)
                Text(o.label, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(end = 4.dp))
            }
        }
    }
}

@Composable
private fun SelectField(state: SchemaFormState, field: SchemaField, locked: Boolean) {
    val p = MaterialTheme.palette
    var expanded by remember { mutableStateOf(false) }
    val value = state.text(field)
    Text(field.label, style = MaterialTheme.typography.bodySmall, color = p.muted)
    Box(Modifier.padding(top = 4.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(8.dp))
                .background(p.panel2)
                .border(1.dp, p.border, RoundedCornerShape(8.dp))
                .clickable(enabled = !locked) { expanded = true }
                .padding(horizontal = 12.dp, vertical = 10.dp)
                .alpha(if (locked) 0.6f else 1f),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(SchemaRules.optionLabel(field, value).ifEmpty { "–" }, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
            Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted)
        }
        DropdownMenu(expanded = expanded, onDismissRequest = { expanded = false }) {
            for (o in field.options) {
                DropdownMenuItem(
                    text = { Text(o.label, fontWeight = if (o.value == value) FontWeight.SemiBold else null) },
                    onClick = {
                        expanded = false
                        state.edit(field.key, o.value)
                    },
                )
            }
        }
    }
}

/** text, path, url, number, password, textarea (and any type this app does not know yet, as text). */
@Composable
private fun TextLikeField(state: SchemaFormState, api: SchemaApi, field: SchemaField, locked: Boolean) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val values = state.values
    val text = state.text(field)
    val isPassword = field.kind == "password"
    val isArea = field.kind == "textarea"
    val shown = state.visible[field.key] == true
    val error = if (field.kind == "number" && !locked) SchemaRules.numberError(text) else null
    val keyboard = when (field.kind) {
        "number" -> if (SchemaRules.needsDecimalKeys(field)) KeyboardType.Decimal else KeyboardType.Number
        "url" -> KeyboardType.Uri
        // also while shown: the password keyboard learns nothing (no transformation is what makes it readable)
        "password" -> KeyboardType.Password
        else -> KeyboardType.Text
    }
    // a secret shown as text is hidden again when its field leaves the screen (another page, the card closed)
    DisposableEffect(state, field.key) { onDispose { state.hide(field.key) } }
    val placeholder = when {
        isPassword -> SchemaRules.passwordPlaceholder(field, values)
        isArea -> SchemaRules.textareaPlaceholder(field, values)
        else -> ""
    }
    OutlinedTextField(
        value = text,
        onValueChange = { state.edit(field.key, it) },
        label = { Text(field.label) },
        placeholder = if (placeholder.isEmpty()) null else ({ Text(placeholder) }),
        enabled = !locked,
        singleLine = !isArea,
        minLines = if (isArea) 4 else 1,
        isError = error != null,
        supportingText = error?.let { e -> { Text(e) } },
        visualTransformation = if (isPassword && !shown) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(
            capitalization = KeyboardCapitalization.None,
            autoCorrectEnabled = false,
            keyboardType = keyboard,
        ),
        textStyle = if (isArea) MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace) else MaterialTheme.typography.bodyMedium,
        modifier = Modifier.fillMaxWidth(),
    )
    if (error == null && field.kind == "number") {
        SchemaRules.rangeNote(field, text)?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn, modifier = Modifier.padding(top = 4.dp)) }
    }
    // Show and Copy for a saved secret (the web's eye and copy buttons; "Show saved" on a text area)
    if (SchemaRules.canShow(field, values, state.canReveal)) {
        Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val showWord = if (isArea) "Show saved" else "Show"
            SmallButton(if (shown) "Hide" else showWord) { scope.launch { state.toggleShown(api, field) } }
            SmallButton("Copy", iconRes = R.drawable.ic_content_copy) { scope.launch { state.copy(api, field, context) } }
        }
    }
}

/** Where a value comes from (schema_form.rs `source_note`). */
@Composable
private fun SourceNote(state: SchemaFormState, api: SchemaApi, field: SchemaField) {
    val p = MaterialTheme.palette
    val values = state.values
    val env = SchemaRules.envOf(field, values)
    val mono = SpanStyle(fontFamily = FontFamily.Monospace)
    val style = MaterialTheme.typography.bodySmall
    val pad = Modifier.padding(top = 4.dp)
    when (SchemaRules.note(field, values)) {
        SchemaRules.Note.LOCKED -> Text(
            buildAnnotatedString {
                append("🔒 Set by ")
                withStyle(mono) { append(env.ifEmpty { "an environment variable" }) }
                append(" in your compose/env – change it there (environment always wins)")
            },
            style = style,
            color = EncoreColors.Info,
            modifier = pad,
        )
        SchemaRules.Note.SAVED_SECRET -> {
            val scope = rememberCoroutineScope()
            Row(pad, verticalAlignment = Alignment.CenterVertically) {
                Text("Saved ✓ (hidden) · ", style = style, color = p.muted)
                Text(
                    "clear",
                    style = style,
                    color = p.accent,
                    modifier = Modifier.clip(RoundedCornerShape(4.dp)).clickable { scope.launch { state.clear(api, field.key) } }.padding(horizontal = 2.dp),
                )
            }
        }
        SchemaRules.Note.UNSET_SECRET -> Text(
            buildAnnotatedString {
                append("Not set")
                if (env.isNotEmpty()) {
                    append(" – or set ")
                    withStyle(mono) { append(env) }
                    append(" in your compose file")
                }
            },
            style = style,
            color = p.muted,
            modifier = pad,
        )
        SchemaRules.Note.DEFAULT_ENV -> Text(
            buildAnnotatedString {
                append("Default · env var ")
                withStyle(mono) { append(env) }
            },
            style = style,
            color = p.faint,
            modifier = pad,
        )
        SchemaRules.Note.NONE -> {}
    }
}
