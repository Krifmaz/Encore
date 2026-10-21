package app.encore.android.features.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.Translations
import app.encore.android.data.api.settingsApi
import app.encore.android.data.session.Settings
import app.encore.android.data.session.ThemeChoice
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.Appearance
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Appearance (web-rs/src/settings/appearance.rs): presets, theme (Encore, Dark, OLED black, Light, Match
 * device), the accent colour (ten swatches or any colour as hex) and the background. Kept on this phone and, where the
 * server keeps preferences (its "theme" feature), in the account's `prefs.appearance` with the web's keys, so the look
 * follows the account to every device. The web's Player bar choice is the web's own and has no place here.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun AppearanceScreen(nav: Nav, serverPrefs: Boolean, onBack: () -> Unit) {
    val store = nav.graph.store
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    val p = MaterialTheme.palette
    val s by store.settings.collectAsState()
    var hex by remember(s.custom) { mutableStateOf(s.custom) }

    fun set(theme: ThemeChoice = s.theme, accent: String = s.accent, custom: String = s.custom, background: String = s.background) {
        store.setAppearance(theme, accent, custom, background)
        if (!serverPrefs) return
        val look = JsonObject(
            mapOf(
                "theme" to JsonPrimitive(theme.id),
                "accent" to JsonPrimitive(accent),
                "custom" to JsonPrimitive(custom),
                "background" to JsonPrimitive(background),
            ),
        )
        scope.launch {
            try { withContext(Dispatchers.IO) { api.putPrefs(JsonObject(mapOf("appearance" to look))) } } catch (e: CancellationException) { throw e } catch (_: Exception) { /* kept on this device */ }
        }
    }

    SettingsPage("Appearance", onBack) {
        SetCard {
            Text("Appearance", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Muted(if (serverPrefs) "Saved to your account, so it follows you to every device." else "Saved on this device.")

            SubHead("Presets")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (x in Appearance.PRESETS) {
                    val on = s.theme == x.theme && s.accent == x.accent && s.background == x.background && (x.accent != "custom" || s.custom == x.custom)
                    val (a, b) = Appearance.accentHex(x.accent, x.custom ?: Appearance.DEFAULT_CUSTOM)
                    val bg = when (x.theme) {
                        ThemeChoice.ENCORE -> Color(0xFF161617)
                        ThemeChoice.OLED -> Color.Black
                        ThemeChoice.LIGHT -> Color(0xFFF6F6FB)
                        else -> Color(0xFF0B0B12)
                    }
                    Row(
                        Modifier
                            .clip(RoundedCornerShape(10.dp))
                            .background(if (on) p.panel2 else p.panel)
                            .border(1.dp, if (on) p.accent else p.border, RoundedCornerShape(10.dp))
                            .clickable { set(x.theme, x.accent, x.custom ?: s.custom, x.background) }
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(
                            Modifier.size(22.dp).clip(RoundedCornerShape(6.dp)).background(
                                Brush.linearGradient(
                                    0f to Appearance.colorOf(a), 0.22f to Appearance.colorOf(a),
                                    0.22f to Appearance.colorOf(b), 0.34f to Appearance.colorOf(b),
                                    0.34f to bg, 1f to bg,
                                ),
                            ),
                        )
                        Spacer(Modifier.width(8.dp))
                        Text(x.name, style = MaterialTheme.typography.labelLarge, fontWeight = if (on) FontWeight.SemiBold else null)
                    }
                }
            }

            SubHead("Theme")
            SegRow(
                listOf("encore" to "Encore", "dark" to "Dark", "oled" to "OLED black", "light" to "Light", "system" to "Match device"),
                s.theme.id,
            ) { set(theme = ThemeChoice.of(it)) }

            SubHead("Accent colour")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for (a in Appearance.ACCENTS) {
                    val on = s.accent == a.id
                    Box(
                        Modifier
                            .size(34.dp)
                            .clip(CircleShape)
                            .background(Brush.linearGradient(listOf(Appearance.colorOf(a.a), Appearance.colorOf(a.b))))
                            .border(if (on) 3.dp else 1.dp, if (on) p.text else p.border, CircleShape)
                            // the web's title/aria-label: TalkBack reads the accent's name and which one is chosen
                            .selectable(selected = on, role = Role.RadioButton) { set(accent = a.id) }
                            .semantics { contentDescription = a.name },
                    )
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Muted("Any colour:")
                Box(Modifier.size(30.dp).clip(RoundedCornerShape(8.dp)).background(Appearance.colorOf(Appearance.cleanHex(hex) ?: s.custom)).border(1.dp, p.border, RoundedCornerShape(8.dp)))
                FormField("", hex, { hex = it.take(7) }, placeholder = "#ff7a00", mono = true, modifier = Modifier.width(120.dp).semantics { contentDescription = "Hex colour" })
                SmallButton("Save") {
                    val c = Appearance.cleanHex(hex)
                    if (c == null) Toasts.show("Use a hex colour like #ff7a00", "err") else set(accent = "custom", custom = c)
                }
                if (s.accent == "custom") Badge("in use", kind = "accent")
            }

            SubHead("Background")
            SegRow(listOf("fade" to "Faded colour", "solid" to "Solid"), s.background) { set(background = it) }

            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.padding(top = 8.dp)) {
                EncoreButton("Example button", iconRes = R.drawable.ic_check, primary = true) {}
                Badge("Example badge", kind = "accent")
            }
        }
    }
}

@Composable
private fun SubHead(text: String) {
    Text(text, style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
}

/**
 * Settings → Language (web-rs/src/pages/translate.rs render_language): how titles in another script show (the
 * server's modes) and into which language, saved to the account (`translate_mode`, `translate_lang`); which sources
 * the server uses under it, and for admins each source with its state and switch (the server's `tiers`).
 */
@Composable
fun LanguageScreen(nav: Nav, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    var opts by remember { mutableStateOf<JsonObject?>(null) }
    var status by remember { mutableStateOf<JsonObject?>(null) }
    var mode by remember { mutableStateOf("off") }
    var lang by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    var version by remember { mutableStateOf(0) }
    LaunchedEffect(version) {
        try {
            val o = withContext(Dispatchers.IO) { api.translateLanguages() }
            val st = try { withContext(Dispatchers.IO) { api.translateStatus() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
            val prefs = try { prefsOf(withContext(Dispatchers.IO) { api.me() }) } catch (e: CancellationException) { throw e } catch (_: Exception) { JsonObject(emptyMap()) }
            opts = o
            status = st
            mode = prefs.str("translate_mode").ifEmpty { "off" }
            lang = prefs.str("translate_lang").ifEmpty { o.str("default").ifEmpty { "en" } }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    fun save(key: String, value: String) {
        scope.launch {
            try {
                val prefs = withContext(Dispatchers.IO) { api.putPrefs(JsonObject(mapOf(key to JsonPrimitive(value)))) }
                mode = prefs.str("translate_mode").ifEmpty { "off" }
                if (prefs.str("translate_lang").isNotEmpty()) lang = prefs.str("translate_lang")
                // the phone shows titles the new way at once, and the server starts on what is missing
                Translations.adopt(prefs)
                scope.launch { try { withContext(Dispatchers.IO) { api.translateRun() } } catch (e: CancellationException) { throw e } catch (_: Exception) {} }
                Toasts.show("Saved", "ok")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
            }
        }
    }
    // an admin switched a source on or off: saved as a server setting, then the states are read again
    fun switchTier(key: String, on: Boolean) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { api.putSettings(JsonObject(mapOf(key to JsonPrimitive(on)))) }
                scope.launch { try { withContext(Dispatchers.IO) { api.translateRun() } } catch (e: CancellationException) { throw e } catch (_: Exception) {} }
                Toasts.show("Saved", "ok")
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
            }
            version += 1
        }
    }
    SettingsPage("Language", onBack) {
        val o = opts
        if (o == null) {
            if (error.isNotEmpty()) ErrorBox(error) { version += 1 } else Spinner(24.dp)
            return@SettingsPage
        }
        SetCard {
            Text("Language", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Muted("Song, album and artist names in another script – Japanese, Korean, Chinese, Cyrillic and more – can show translated or romanized. Saved to your account, so it follows you to every device and to the car.")
            SubHead("Translated titles")
            SegRow(o.arr("modes").objectsOf().map { it.str("id") to it.str("label") }, mode) { save("translate_mode", it) }
            Dropdown("Translate into", o.arr("languages").objectsOf().map { it.str("code") to it.str("name") }, lang, enabled = mode != "off") { save("translate_lang", it) }
            // the phone has no edit dialog: the correction is made on this server's website
            val site = nav.graph.store.current?.server?.trim()?.trimEnd('/').orEmpty()
            val where = if (site.isEmpty()) "." else " at $site."
            Muted("Corrections by hand come first, then official names (MusicBrainz and Apple, where the label published one), romanization made on this server, then DeepL and the local model where an admin set them up. Titles on your screen are translated first, the rest of the library in the background. Fix any title with ⋯ → Edit translation$where")
            val st = status
            if (st != null) {
                // which sources this server uses (the server's words)
                if (st.str("summary").isNotEmpty()) Muted(st.str("summary"))
                LanguageRules.counts(st)?.let { Muted(it, color = MaterialTheme.palette.faint) }
            }
        }
        // admins: the sources of translated titles, strongest first, each with its state and its switch
        val tiers = status?.arr("tiers").objectsOf()
        if (tiers.isNotEmpty()) {
            SetCard {
                Text("Translation sources", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                Muted("Strongest first: a source higher up wins over the ones below it, and its results replace theirs when it gets to a title later.")
                for (tier in tiers) {
                    val key = tier.str("setting")
                    if (key.isEmpty()) {
                        Column(Modifier.padding(vertical = 4.dp)) {
                            Text(tier.str("state"), style = MaterialTheme.typography.bodyMedium)
                            Muted(tier.str("help"))
                        }
                    } else {
                        SwitchRow(tier.str("state"), tier.bool("on"), help = tier.str("help")) { on -> switchTier(key, on) }
                    }
                }
                Muted("The DeepL API key and the monthly limit are under Settings → Library → Translated titles.")
            }
        }
    }
}

/** For the shell's list: the look in a few words ("Encore · Logo"). */
fun appearanceSummary(s: Settings): String {
    val theme = when (s.theme) {
        ThemeChoice.ENCORE -> "Encore"
        ThemeChoice.DARK -> "Dark"
        ThemeChoice.OLED -> "OLED black"
        ThemeChoice.LIGHT -> "Light"
        ThemeChoice.SYSTEM -> "Match device"
    }
    val accent = Appearance.ACCENTS.firstOrNull { it.id == s.accent }?.name ?: s.custom
    return "$theme · $accent"
}
