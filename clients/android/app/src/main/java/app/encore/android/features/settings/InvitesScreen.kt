package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.dbl
import app.encore.android.core.str
import app.encore.android.data.api.settingsApi
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Invites (web-rs/src/settings/invites.rs): the sign-up mode, the invites with their uses, role, expiry
 * and who used them, Copy link, delete, and New invite.
 */
@Composable
fun InvitesScreen(nav: Nav, onBack: () -> Unit) {
    val api = nav.graph.settingsApi
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val now = rememberNow()
    var version by remember { mutableIntStateOf(0) }
    var settings by remember { mutableStateOf<JsonObject?>(null) }
    var invites by remember { mutableStateOf<List<JsonObject>?>(null) }
    var error by remember { mutableStateOf("") }
    var making by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        settings = try { withContext(Dispatchers.IO) { api.adminSettings() } } catch (e: CancellationException) { throw e } catch (e: Exception) { error = e.message ?: ""; null }
    }
    LaunchedEffect(version) {
        try {
            invites = withContext(Dispatchers.IO) { api.invites() }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    val base = settings?.str("base_url")?.ifEmpty { null } ?: api.base
    val link = { code: String -> SettingsRules.inviteLink(base, code) }

    SettingsPage("Invites", onBack, subtitle = "Share an invite link so friends can create an account.") {
        EncoreButton("New invite", iconRes = R.drawable.ic_add, primary = true, modifier = Modifier.padding(bottom = 12.dp)) { making = true }
        val s = settings
        SetCard(Modifier.padding(bottom = 14.dp)) {
            Text("Sign-up mode", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
            Muted("Who is allowed to create an account on the login page.")
            val modes = listOf("invite" to "Invite only", "open" to "Open", "closed" to "Closed")
            // ENCORE_SIGNUP_MODE wins: the server does not save the mode then
            val locked = s != null && SchemaRules.source(s, "signup_mode") == "env"
            SegRow(modes, s?.str("signup_mode").orEmpty(), enabled = s != null && !locked) { m ->
                scope.serverCall(after = { r: JsonObject ->
                    settings = r
                    app.encore.android.ui.components.Toasts.show("Sign-ups: ${modes.first { it.first == m }.second}", "ok")
                }) { api.putSettings(JsonObject(mapOf("signup_mode" to JsonPrimitive(m)))) }
            }
            if (s != null && locked) EnvLockedNote(SchemaRules.envVar(s, "signup_mode"), Modifier)
        }
        val list = invites
        when {
            list == null && error.isNotEmpty() -> ErrorBox(error) { version += 1 }
            list == null -> Spinner(24.dp)
            list.isEmpty() -> EmptyState(R.drawable.ic_link, "No invites yet", "Create one and send the link to a friend.")
            else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (i in list) {
                    val dead = SettingsRules.inviteDead(i, now)
                    SetCard(flat = true) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(
                                    i.str("code"),
                                    style = MaterialTheme.typography.bodyLarge,
                                    fontWeight = FontWeight.Bold,
                                    fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace,
                                    textDecoration = if (dead) TextDecoration.LineThrough else null,
                                    color = if (dead) MaterialTheme.palette.faint else MaterialTheme.palette.text,
                                )
                                Muted(i.str("note"), color = MaterialTheme.palette.faint)
                            }
                            if (i.str("role") == "admin" || i.str("role") == "owner") Badge("admin", kind = "warn") else Muted("user")
                        }
                        val expires = when {
                            !SchemaRules.truthy(i["expires_at"]) -> "never"
                            i.dbl("expires_at") < now -> "expired"
                            else -> formatDay(i.dbl("expires_at"))
                        }
                        Muted("Uses ${i.str("uses")} / ${i.str("max_uses")} · Expires $expires · Used by ${i.str("used_by").ifEmpty { "—" }}")
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            if (!dead) SmallButton("Copy link", iconRes = R.drawable.ic_content_copy) { copyToClipboard(context, link(i.str("code"))) }
                            SmallButton("Delete", iconRes = R.drawable.ic_delete) {
                                scope.serverCall(after = { _: Unit -> version += 1 }) { api.deleteInvite(i.str("id")) }
                            }
                        }
                    }
                }
            }
        }
    }
    if (making) NewInviteDialog(nav, link, onDismiss = { making = false }) { version += 1 }
}

@Composable
private fun NewInviteDialog(nav: Nav, link: (String) -> String, onDismiss: () -> Unit, onMade: () -> Unit) {
    val api = nav.graph.settingsApi
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var uses by remember { mutableStateOf("1") }
    var days by remember { mutableStateOf("7") }
    var role by remember { mutableStateOf("user") }
    var note by remember { mutableStateOf("") }
    var made by remember { mutableStateOf<String?>(null) }
    val code = made
    FormDialog(
        "New invite",
        onDismiss = onDismiss,
        okLabel = if (code == null) "Create invite" else "",
        cancelLabel = "Close",
        onOk = {
            val maxUses = SettingsRules.jsPlus(uses).let { if (SchemaRules.truthy(it)) it else JsonPrimitive(1) }
            val expires = SettingsRules.jsPlus(days).let { if (SchemaRules.truthy(it)) it else JsonNull }
            val body = JsonObject(
                mapOf(
                    "max_uses" to maxUses,
                    "expires_days" to expires,
                    "role" to JsonPrimitive(role),
                    "note" to if (note.isNotEmpty()) JsonPrimitive(note) else JsonNull,
                ),
            )
            scope.serverCall(after = { inv: JsonObject -> made = inv.str("code"); onMade() }) { api.createInvite(body) }
        },
    ) {
        if (code == null) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FormField("Max uses", uses, { uses = it }, keyboard = KeyboardType.Number, modifier = Modifier.weight(1f))
                FormField("Expires after (days)", days, { days = it }, help = "0 = never", keyboard = KeyboardType.Decimal, modifier = Modifier.weight(1f))
            }
            Dropdown("Role", listOf("user" to "User", "admin" to "Admin"), role) { role = it }
            FormField("Note", note, { note = it }, placeholder = "e.g. for Sam")
        } else {
            Muted("Send this link – it signs them up with the invite filled in:")
            Mono(link(code))
            Row(verticalAlignment = Alignment.CenterVertically) {
                SmallButton("Copy link", iconRes = R.drawable.ic_content_copy, primary = true) { copyToClipboard(context, link(code)) }
                Spacer(Modifier.width(10.dp))
                Muted("Code: $code")
            }
        }
    }
}
