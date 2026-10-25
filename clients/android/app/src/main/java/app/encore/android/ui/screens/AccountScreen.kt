package app.encore.android.ui.screens

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.AuthInfo
import app.encore.android.data.repo.Me
import app.encore.android.data.repo.SessionRow
import app.encore.android.data.repo.TotpSetup
import app.encore.android.data.repo.formatAgo
import app.encore.android.data.api.settingsApi
import app.encore.android.data.session.Session
import app.encore.android.features.settings.AboutEncore
import app.encore.android.playback.ResumeStore
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Avatar
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.LoadableContent
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.rememberLoad
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private class AccountData(val me: Me, val info: AuthInfo)

/**
 * Account (web/js/pages/account.js): profile with the avatar picker and the username pencil, password, email,
 * two-step sign-in (QR code), the active sessions with revoke, "Sign out everywhere" and sign out.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AccountScreen(nav: Nav, session: Session, onBack: () -> Unit = nav.back) {
    val features = nav.features
    val graph = nav.graph
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var version by remember { mutableStateOf(0) }
    val loader = rememberLoad("account", session.server, version) {
        AccountData(features.me(), try { features.info() } catch (_: Exception) { AuthInfo("Encore", "", "", emptyList()) })
    }
    var sessionsVersion by remember { mutableStateOf(0) }
    val sessions = rememberLoad("sessions", sessionsVersion) { features.sessions() }
    var avatarVersion by remember { mutableStateOf(System.currentTimeMillis().toDouble()) }

    // forms
    var nameOpen by remember { mutableStateOf(false) }
    var nameValue by remember { mutableStateOf("") }
    var pwOpen by remember { mutableStateOf(false) }
    var pwCurrent by remember { mutableStateOf("") }
    var pwNew by remember { mutableStateOf("") }
    var email by remember { mutableStateOf<String?>(null) }
    var totp by remember { mutableStateOf<TotpSetup?>(null) }
    var totpCode by remember { mutableStateOf("") }
    var totpOffOpen by remember { mutableStateOf(false) }
    var totpPassword by remember { mutableStateOf("") }
    var confirmLogoutAll by remember { mutableStateOf(false) }
    var confirmSignOut by remember { mutableStateOf(false) }

    val pickImage = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            try {
                withContext(Dispatchers.IO) {
                    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: throw IllegalStateException("Couldn't read the picture")
                    val mime = context.contentResolver.getType(uri) ?: "image/jpeg"
                    features.uploadAvatar(bytes, mime)
                }
                Toasts.show("Picture updated", "ok")
                avatarVersion = System.currentTimeMillis().toDouble()
                version += 1
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Upload failed", "err")
            }
        }
    }

    fun run(call: () -> Unit, ok: String, after: () -> Unit = {}) {
        scope.launch {
            try {
                withContext(Dispatchers.IO) { call() }
                if (ok.isNotEmpty()) Toasts.show(ok, "ok")
                after()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }

    val scrollBehavior = TopAppBarDefaults.enterAlwaysScrollBehavior()
    Scaffold(
        modifier = Modifier.fillMaxSize().nestedScroll(scrollBehavior.nestedScrollConnection),
        containerColor = p.bg,
        contentWindowInsets = WindowInsets(0, 0, 0, 0),
        topBar = { EncoreTopBar("Account", scrollBehavior = scrollBehavior, onBack = onBack) },
    ) { padding ->
        LoadableContent(loader) { data ->
            val me = data.me
            val has = { f: String -> f in data.info.features }
            val admin = me.isAdmin
            LaunchedEffect(me.email) { if (email == null) email = me.email }
            LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                // profile card
                item {
                    Box(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                        CardBox {
                            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Avatar(me.username, if (me.avatarAt > 0) features.avatarUrl(me.id, avatarVersion) else null, graph.imageLoader, size = 52.dp)
                                    Spacer(Modifier.width(12.dp))
                                    Column(Modifier.weight(1f)) {
                                        Row(verticalAlignment = Alignment.CenterVertically) {
                                            Text(me.username, style = MaterialTheme.typography.titleLarge)
                                            if (has("rename")) {
                                                IconButton(onClick = { nameOpen = !nameOpen; nameValue = me.username }, modifier = Modifier.size(32.dp)) {
                                                    Icon(painterResource(R.drawable.ic_edit), contentDescription = "Change username", tint = p.muted, modifier = Modifier.size(16.dp))
                                                }
                                            }
                                        }
                                        Text(
                                            (if (me.role == "owner") "Owner" else if (admin) "Administrator" else "Member") + " · joined " + formatAgo(me.createdAt),
                                            style = MaterialTheme.typography.bodySmall, color = p.muted,
                                        )
                                    }
                                }
                                if (nameOpen && has("rename")) {
                                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                        EncoreField(nameValue, { nameValue = it }, placeholder = "Username", imeAction = ImeAction.Done, modifier = Modifier.fillMaxWidth())
                                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                            EncoreButton("Save", primary = true) {
                                                val n = nameValue.trim()
                                                run({ features.updateProfile(username = n) }, "You're now $n") { nameOpen = false; version += 1 }
                                            }
                                            EncoreButton("Cancel") { nameOpen = false; nameValue = me.username }
                                        }
                                        Text("What you sign in with and what others see. 3–32 letters, numbers, . _ -", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                        if (me.email.isEmpty()) {
                                            Text(
                                                "⚠ There's no email on this account, so your username is the only way you'll be able to sign in – make it one you'll remember. Add an email below to be able to reset your password.",
                                                style = MaterialTheme.typography.bodySmall, color = EncoreColors.Warn,
                                            )
                                        }
                                    }
                                }
                                if (has("avatar")) {
                                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                        SmallButton("Change picture", iconRes = R.drawable.ic_add) {
                                            pickImage.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                                        }
                                        if (me.avatarAt > 0) SmallButton("Remove") { run({ features.deleteAvatar() }, "") { version += 1 } }
                                    }
                                }
                                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text(
                                        "Requests this week: ${me.requestsThisWeek}" + if (me.requestLimit > 0 && !admin) " of ${me.requestLimit}" else "",
                                        style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.align(Alignment.CenterVertically),
                                    )
                                    if (me.autoApprove || admin) Badge("auto-approved", kind = "ok")
                                }
                            }
                        }
                    }
                }
                // password
                item {
                    Box(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                        CardBox {
                            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Text("Password", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                                    SmallButton(if (pwOpen) "Cancel" else "Change password") { pwOpen = !pwOpen; if (!pwOpen) { pwCurrent = ""; pwNew = "" } }
                                }
                                if (pwOpen) {
                                    EncoreField(pwCurrent, { pwCurrent = it }, label = "Current password", password = true, keyboardType = KeyboardType.Password, imeAction = ImeAction.Next, modifier = Modifier.fillMaxWidth())
                                    EncoreField(pwNew, { pwNew = it }, label = "New password", password = true, keyboardType = KeyboardType.Password, imeAction = ImeAction.Done, modifier = Modifier.fillMaxWidth())
                                    EncoreButton("Update password", primary = true, enabled = pwCurrent.isNotEmpty() && pwNew.length >= 8) {
                                        run({ features.changePassword(pwCurrent, pwNew) }, "Password updated – other sessions signed out") {
                                            pwOpen = false; pwCurrent = ""; pwNew = ""; sessionsVersion += 1
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // email
                item {
                    Box(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                        CardBox {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text("Email", style = MaterialTheme.typography.titleMedium)
                                Text("Used only to send you a link if you forget your password.", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    EncoreField(email ?: "", { email = it }, placeholder = "you@example.com", keyboardType = KeyboardType.Email, imeAction = ImeAction.Done, modifier = Modifier.weight(1f))
                                    Spacer(Modifier.width(8.dp))
                                    EncoreButton("Save", primary = true) { run({ features.updateProfile(email = (email ?: "").trim()) }, "Email saved") { version += 1 } }
                                }
                            }
                        }
                    }
                }
                // two-step sign-in
                if (has("totp")) {
                    item {
                        Box(Modifier.padding(horizontal = Gutter, vertical = 6.dp)) {
                            CardBox {
                                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                    Text("Two-step sign-in", style = MaterialTheme.typography.titleMedium)
                                    Text(
                                        if (me.totpEnabled) "✅ On – signing in asks for a code from your authenticator app."
                                        else "Ask for a 6-digit code from an authenticator app (Google Authenticator, 1Password, Aegis…) when signing in.",
                                        style = MaterialTheme.typography.bodySmall, color = p.muted,
                                    )
                                    val setup = totp
                                    when {
                                        me.totpEnabled && totpOffOpen -> {
                                            EncoreField(totpPassword, { totpPassword = it }, placeholder = "Your password", password = true, keyboardType = KeyboardType.Password, modifier = Modifier.fillMaxWidth())
                                            EncoreButton("Turn off two-step sign-in", enabled = totpPassword.isNotEmpty()) {
                                                run({ features.totpDisable(totpPassword) }, "Two-step sign-in is off") { totpOffOpen = false; totpPassword = ""; version += 1 }
                                            }
                                        }
                                        me.totpEnabled -> EncoreButton("Turn off") { totpOffOpen = true }
                                        setup != null -> {
                                            val bitmap = remember(setup.qrBase64) {
                                                try {
                                                    val bytes = Base64.decode(setup.qrBase64, Base64.DEFAULT)
                                                    BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
                                                } catch (_: Exception) { null }
                                            }
                                            if (bitmap != null) {
                                                Box(Modifier.size(196.dp).clip(RoundedCornerShape(12.dp)).background(Color.White).padding(8.dp)) {
                                                    Image(bitmap = bitmap, contentDescription = "QR code", modifier = Modifier.fillMaxSize())
                                                }
                                            }
                                            Text("Scan the code with your authenticator app, or enter this key:", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                            Text(setup.secret, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
                                            Row(verticalAlignment = Alignment.CenterVertically) {
                                                EncoreField(totpCode, { totpCode = it }, placeholder = "6-digit code", keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done, modifier = Modifier.width(160.dp))
                                                Spacer(Modifier.width(8.dp))
                                                EncoreButton("Turn on", primary = true, enabled = totpCode.isNotEmpty()) {
                                                    run({ features.totpEnable(totpCode) }, "Two-step sign-in is on") { totp = null; totpCode = ""; version += 1 }
                                                }
                                            }
                                        }
                                        else -> EncoreButton("Set up", primary = true) {
                                            scope.launch {
                                                try { totp = withContext(Dispatchers.IO) { features.totpSetup() } } catch (e: Exception) { Toasts.show(e.message ?: "That didn't work", "err") }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                // sessions
                item {
                    SectionHead(
                        "Active sessions",
                        action = if (has("logout_all")) "Sign out everywhere" else null,
                        onAction = if (has("logout_all")) ({ confirmLogoutAll = true }) else null,
                    )
                }
                item {
                    Box(Modifier.padding(horizontal = Gutter)) {
                        CardBox(flat = true) {
                            LoadableContent(sessions) { rows: List<SessionRow> ->
                                Column {
                                    for (s in rows) {
                                        Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                                            Column(Modifier.weight(1f)) {
                                                Text(s.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 2)
                                                Text("${s.ip} · signed in ${formatAgo(s.createdAt)}", style = MaterialTheme.typography.bodySmall, color = p.muted)
                                            }
                                            if (s.current) Badge("This device", kind = "ok")
                                            else SmallButton("Revoke", iconRes = R.drawable.ic_logout) { run({ features.revokeSession(s.id) }, "") { sessionsVersion += 1 } }
                                        }
                                    }
                                    if (rows.isEmpty()) Text("No sessions", color = p.muted, modifier = Modifier.padding(14.dp))
                                }
                            }
                        }
                    }
                }
                // members get the About card here; admins have it under Settings → General (as on the web)
                if (!admin) {
                    item { Box(Modifier.padding(horizontal = Gutter)) { AboutEncore(graph.settingsApi) } }
                }
                item {
                    Row(Modifier.fillMaxWidth().padding(Gutter), horizontalArrangement = Arrangement.Center) {
                        EncoreButton("Sign out", iconRes = R.drawable.ic_logout) { confirmSignOut = true }
                    }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }

    if (confirmLogoutAll) {
        ConfirmDialog(
            "Sign out everywhere?",
            "Every phone, computer and app signed in to your account is signed out – including this one.",
            "Sign out everywhere", danger = true, onDismiss = { confirmLogoutAll = false },
        ) {
            nav.player.stopAndClear()
            ResumeStore(context).clear()
            scope.launch {
                try { withContext(Dispatchers.IO) { features.logoutAll() } } catch (_: Exception) {}
                graph.likes.clear()
                graph.dislikes.clear()
                graph.prefs.clear()
                graph.store.signOut()
            }
        }
    }
    if (confirmSignOut) {
        ConfirmDialog("Sign out?", "You can sign in again any time.", "Sign out", onDismiss = { confirmSignOut = false }) {
            nav.player.stopAndClear()
            ResumeStore(context).clear()
            scope.launch {
                try { withContext(Dispatchers.IO) { graph.api.logout() } } catch (_: Exception) {}
                graph.likes.clear()
                graph.dislikes.clear()
                graph.prefs.clear()
                graph.store.signOut()
            }
        }
    }
}
