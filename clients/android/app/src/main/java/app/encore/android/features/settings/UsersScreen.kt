package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.settingsApi
import app.encore.android.data.repo.formatAgo
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Avatar
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * Settings → Users & permissions (web-rs/src/settings/users.rs): every account with its role, weekly limit,
 * auto-approve, permissions, password reset, disable and delete; permissions for everyone at once; and what new
 * members start with. On the phone each user is a card instead of a table row.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun UsersScreen(nav: Nav, meId: Long, onBack: () -> Unit, onOpenInvites: () -> Unit) {
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    var version by remember { mutableIntStateOf(0) }
    var users by remember { mutableStateOf<List<JsonObject>?>(null) }
    var error by remember { mutableStateOf("") }
    var perms by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var permsVersion by remember { mutableIntStateOf(0) }
    var defaults by remember { mutableStateOf<JsonObject?>(null) }
    var adding by remember { mutableStateOf(false) }
    var permsFor by remember { mutableStateOf<JsonObject?>(null) }
    var passwordFor by remember { mutableStateOf<JsonObject?>(null) }
    var deleteFor by remember { mutableStateOf<JsonObject?>(null) }
    LaunchedEffect(version) {
        try {
            users = withContext(Dispatchers.IO) { api.users() }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    LaunchedEffect(permsVersion) {
        perms = try { withContext(Dispatchers.IO) { api.permissions() }.arr("perms").objectsOf() } catch (e: CancellationException) { throw e } catch (_: Exception) { perms }
    }
    LaunchedEffect(Unit) {
        defaults = try { withContext(Dispatchers.IO) { api.adminSettings() } } catch (e: CancellationException) { throw e } catch (_: Exception) { null }
    }
    fun patch(id: String, body: JsonObject, after: () -> Unit = {}) {
        scope.serverCall(ok = "Saved", onError = { msg -> app.encore.android.ui.components.Toasts.show(msg, "err"); version += 1 }, after = { _: Unit -> version += 1; after() }) {
            api.patchUser(id, body)
        }
    }

    SettingsPage("Users", onBack, subtitle = "Manage who can access your library and what they can request.") {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(bottom = 12.dp)) {
            EncoreButton("Invites", iconRes = R.drawable.ic_link) { onOpenInvites() }
            EncoreButton("Add user", iconRes = R.drawable.ic_add, primary = true) { adding = true }
        }
        val list = users
        when {
            list == null && error.isNotEmpty() -> ErrorBox(error) { version += 1 }
            list == null -> Spinner(24.dp)
            else -> Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for (u in list) UserCard(nav, u, isMe = u.long("id") == meId, onPatch = { body -> patch(u.str("id"), body) },
                    onPerms = { permsFor = u }, onPassword = { passwordFor = u }, onDelete = { deleteFor = u })
            }
        }

        PageSection("Permissions for everyone", "applies to every non-admin user; fine-tune one user with the gear in their row")
        SetCard(flat = true) {
            for (pm in perms) {
                Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(pm.str("label"), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        Muted("${pm.str("on")} of ${pm.str("of")} user(s) have it")
                    }
                    for ((v, label) in listOf(true to "All on", false to "All off")) {
                        Spacer(Modifier.width(6.dp))
                        SmallButton(label) {
                            scope.serverCall(ok = "Updated for everyone", after = { _: Unit -> permsVersion += 1; version += 1 }) { api.bulkPermission(pm.str("key"), v) }
                        }
                    }
                }
            }
        }

        PageSection("New members start with", "copied onto every new account (sign-up, invite or added here); change one person later with their gear")
        val d = defaults
        if (d != null) SetCard { NewMemberDefaults(nav, d) { defaults = it } }
    }

    if (adding) AddUserDialog(nav, onDismiss = { adding = false }) { adding = false; version += 1 }
    permsFor?.let { u ->
        PermissionsDialog(u, perms, onDismiss = { permsFor = null }) { chosen ->
            permsFor = null
            patch(u.str("id"), JsonObject(mapOf("perms" to JsonObject(chosen.mapValues { JsonPrimitive(it.value) })))) { permsVersion += 1 }
        }
    }
    passwordFor?.let { u ->
        var pw by remember(u) { mutableStateOf("") }
        FormDialog("Reset password for ${u.str("username")}", onDismiss = { passwordFor = null }, okLabel = "Set password", okEnabled = pw.length >= 8, onOk = {
            passwordFor = null
            patch(u.str("id"), JsonObject(mapOf("password" to JsonPrimitive(pw))))
        }) {
            FormField("New password", pw, { pw = it }, password = true)
            Muted("All of their sessions will be signed out.")
        }
    }
    deleteFor?.let { u ->
        val name = u.str("username")
        ConfirmDialog("Delete user", "Delete $name? Their requests stay in history.", "Delete", danger = true, onDismiss = { deleteFor = null }) {
            scope.serverCall(after = { _: Unit -> version += 1 }) { api.deleteUser(u.str("id")) }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun UserCard(nav: Nav, u: JsonObject, isMe: Boolean, onPatch: (JsonObject) -> Unit, onPerms: () -> Unit, onPassword: () -> Unit, onDelete: () -> Unit) {
    val p = MaterialTheme.palette
    val role = u.str("role")
    val admin = role == "admin" || role == "owner"
    val disabled = u.bool("disabled")
    var limit by remember(u) { mutableStateOf(if (SchemaRules.truthy(u["request_limit"])) u.str("request_limit") else "") }
    SetCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val avatar = if (u.dbl("avatar_at") > 0) nav.graph.api.url("/api/users/${u.str("id")}/avatar?v=${u.dbl("avatar_at").toLong()}") else null
            Avatar(u.str("username"), avatar, nav.graph.imageLoader, size = 36.dp)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f)) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(u.str("username"), style = MaterialTheme.typography.bodyLarge, fontWeight = FontWeight.SemiBold)
                    if (isMe) Badge("you")
                    if (disabled) Badge("disabled", kind = "err")
                }
                val meta = listOfNotNull(
                    u.str("email").ifEmpty { null },
                    u.str("invite_code").ifEmpty { null }?.let { "invite $it" },
                    "joined ${formatAgo(u.dbl("created_at"))}",
                )
                Muted(meta.joinToString(" · "), color = p.faint)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            if (role == "owner") Badge("Owner", kind = "accent")
            else Dropdown(null, listOf("user" to "User", "admin" to "Admin"), role, enabled = !isMe) { v -> if (v != role) onPatch(JsonObject(mapOf("role" to JsonPrimitive(v)))) }
            Column {
                Muted("Requests")
                Text(u.str("request_count").ifEmpty { "0" }, style = MaterialTheme.typography.bodyMedium)
            }
            Column {
                Muted("Last seen")
                Text(formatAgo(u.dbl("last_login")), style = MaterialTheme.typography.bodyMedium)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            FormField("Weekly limit", limit, { limit = it.filter { c -> c.isDigit() } }, placeholder = "∞", keyboard = KeyboardType.Number, modifier = Modifier.width(130.dp))
            SmallButton("Save") { onPatch(JsonObject(mapOf("request_limit" to JsonPrimitive(SettingsRules.parseIntOrZero(limit))))) }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(checked = u.bool("auto_approve"), onCheckedChange = { onPatch(JsonObject(mapOf("auto_approve" to JsonPrimitive(it)))) })
            Spacer(Modifier.width(10.dp))
            Text("Auto-approve", style = MaterialTheme.typography.bodyMedium)
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (!admin) SmallButton("Permissions", iconRes = R.drawable.ic_settings) { onPerms() }
            SmallButton("Reset password") { onPassword() }
            if (!isMe) {
                SmallButton(if (disabled) "Enable" else "Disable") { onPatch(JsonObject(mapOf("disabled" to JsonPrimitive(!disabled)))) }
                SmallButton("Delete", iconRes = R.drawable.ic_delete) { onDelete() }
            }
        }
    }
}

/** The gear of a user: each permission on or off (PATCH {perms}). */
@Composable
private fun PermissionsDialog(u: JsonObject, perms: List<JsonObject>, onDismiss: () -> Unit, onSave: (Map<String, Boolean>) -> Unit) {
    val have = u.obj("perms")
    val chosen = remember(u) { mutableStateMapOf<String, Boolean>().apply { for (pm in perms) put(pm.str("key"), SchemaRules.truthy(have?.get(pm.str("key")))) } }
    FormDialog("Permissions – ${u.str("username")}", onDismiss = onDismiss, okLabel = "Save", onOk = { onSave(chosen.toMap()) }) {
        for (pm in perms) {
            val k = pm.str("key")
            SwitchRow(pm.str("label"), chosen[k] == true) { chosen[k] = it }
        }
    }
}

@Composable
private fun AddUserDialog(nav: Nav, onDismiss: () -> Unit, onCreated: () -> Unit) {
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    var username by remember { mutableStateOf("") }
    var email by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var role by remember { mutableStateOf("user") }
    FormDialog("Add user", onDismiss = onDismiss, okLabel = "Create", okEnabled = username.trim().length >= 3 && password.length >= 8, onOk = {
        val body = buildJsonObject {
            put("username", username)
            put("email", email)
            put("password", password)
            put("role", role)
        }
        scope.serverCall(ok = "User created", after = { _: Unit -> onCreated() }) { api.createUser(body) }
    }) {
        FormField("Username", username, { username = it })
        FormField("Email (optional)", email, { email = it }, keyboard = KeyboardType.Email)
        FormField("Password", password, { password = it }, password = true)
        Dropdown("Role", listOf("user" to "User", "admin" to "Admin"), role) { role = it }
    }
}

/** What every new account starts with (saved one key at a time: "Saved for new members"). */
@Composable
private fun NewMemberDefaults(nav: Nav, values: JsonObject, onValues: (JsonObject) -> Unit) {
    val api = nav.graph.settingsApi
    val scope = rememberCoroutineScope()
    var limit by remember { mutableStateOf(if (SettingsRules.nullish(values, "default_request_limit")) "0" else values.str("default_request_limit")) }
    fun save(key: String, v: kotlinx.serialization.json.JsonElement) {
        scope.serverCall(ok = "Saved for new members", after = { r: JsonObject -> onValues(r) }) { api.putSettings(JsonObject(mapOf(key to v))) }
    }
    // a key an environment variable sets is not saved by the server: shown disabled with where to change it
    fun locked(k: String) = SchemaRules.source(values, k) == "env"
    for ((k, label) in SettingsRules.NEW_MEMBER) {
        SwitchRow(label, SchemaRules.truthy(values[k]), enabled = !locked(k)) { save(k, JsonPrimitive(it)) }
        if (locked(k)) EnvLockedNote(SchemaRules.envVar(values, k), Modifier.padding(bottom = 4.dp))
    }
    val limitLocked = locked("default_request_limit")
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        FormField("Weekly request limit (0 = unlimited)", limit, { limit = it }, keyboard = KeyboardType.Number, enabled = !limitLocked, modifier = Modifier.weight(1f))
        SmallButton("Save", enabled = !limitLocked) { save("default_request_limit", SettingsRules.jsPlus(limit)) }
    }
    if (limitLocked) EnvLockedNote(SchemaRules.envVar(values, "default_request_limit"))
}
