package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.obj
import app.encore.android.core.str
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/**
 * Settings → Downloads → YouTube sign-in, above its fields (web-rs/src/pages/youtube.rs): which cookies yt-dlp
 * uses (a sign-in handed over by the Encore browser extension, the extension's sync of an everyday browser, or the
 * pasted cookies of the field below), Refresh now, Sign out and the extension token. The cookie paste itself is the
 * schema's `ytdlp_cookies` field of the same card.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun YouTubeCard(api: SettingsApi) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val now = rememberNow()
    var info by remember { mutableStateOf<JsonObject?>(null) }
    var error by remember { mutableStateOf("") }
    var token by remember { mutableStateOf("") }
    var refreshing by remember { mutableStateOf(false) }
    var askSignOut by remember { mutableStateOf(false) }
    var askToken by remember { mutableStateOf(false) }
    suspend fun load() {
        try {
            info = withContext(Dispatchers.IO) { api.youtubeAuth() }
            error = ""
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: "Something went wrong"
        }
    }
    fun newToken() {
        scope.launch {
            try {
                token = withContext(Dispatchers.IO) { api.youtubeExtensionToken() }.str("token")
                load()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Something went wrong", "err")
            }
        }
    }
    LaunchedEffect(Unit) { load() }

    SetCard(Modifier.padding(bottom = 10.dp), flat = true) {
        val r = info
        when {
            r != null -> {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Top) {
                    Column(Modifier.weight(1f)) {
                        Text(r.str("source_label").ifEmpty { "Not signed in" }, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                        if (r.str("account").isNotEmpty()) Muted("Account: ${r.str("account")}")
                    }
                    val (badge, kind) = SettingsRules.youtubeBadge(r)
                    Badge(badge, kind = kind)
                }
                Muted(SettingsRules.youtubeWhen(r, now))
                FlowRow(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    for (k in listOf("handoff", "extension", "pasted")) Muted(SettingsRules.youtubeSourceLine(k, r.obj("sources")?.obj(k)))
                }
                Muted(r.str("diagnostics"), color = p.faint)
            }
            error.isNotEmpty() -> Text(error, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
            else -> Muted("Loading…")
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            SmallButton("Connect the browser extension", iconRes = R.drawable.ic_link, primary = true) {
                val had = info?.obj("extension")?.let { SchemaRules.truthy(it["token_at"]) } == true
                if (had) askToken = true else newToken()
            }
            SmallButton("Refresh now", iconRes = R.drawable.ic_refresh, enabled = !refreshing) {
                refreshing = true
                scope.launch {
                    try {
                        val res = withContext(Dispatchers.IO) { api.youtubeRefresh() }
                        info = res
                        val (msg, kind) = SettingsRules.youtubeRefreshToast(res)
                        Toasts.show(msg, kind)
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                    refreshing = false
                }
            }
            SmallButton("Sign out", iconRes = R.drawable.ic_close) { askSignOut = true }
        }
        if (token.isNotEmpty()) {
            SetCard(Modifier.padding(top = 4.dp), flat = true) {
                Text("Connect the Encore extension", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                Muted("1. Install it on a computer: download Encore-youtube-extension-<version>.zip from the releases page and unzip it. Chrome, Edge or Brave: Extensions → Developer mode → Load unpacked → the chrome folder, then Details → Allow in Incognito. Firefox: about:debugging → Load Temporary Add-on → firefox/manifest.json, then about:addons → Run in Private Windows → Allow.")
                Muted("2. Open its options and paste this Encore’s address and the token below.")
                Muted("3. Press Hand over a YouTube sign-in in the extension. A private window opens: sign in to Google there. Once YouTube Music shows you signed in, the extension sends the sign-in to Encore and closes the window without signing out.")
                Mono(token)
                SmallButton("Copy", iconRes = R.drawable.ic_content_copy) { copyToClipboard(context, token, sensitive = true) }
                Muted("Shown once. It can only send YouTube cookies to Encore; making a new one stops the old one. After the hand-off Encore owns that sign-in and refreshes it itself every 4 hours, also while your computer is off.", color = p.faint)
            }
        }
    }
    if (askSignOut) {
        ConfirmDialog("Sign out", "Forget the handed-over sign-in and the extension’s cookies? Pasted cookies stay (clear them below).", "Sign out", danger = true, onDismiss = { askSignOut = false }) {
            scope.serverCall(ok = "Signed out", after = { r: JsonObject -> info = r }) { api.youtubeSignOut() }
        }
    }
    if (askToken) {
        ConfirmDialog("Connect the browser extension", "Make a new token? The extension stops working until you enter the new one in its options.", "OK", onDismiss = { askToken = false }) { newToken() }
    }
}
