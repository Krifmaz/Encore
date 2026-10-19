package app.encore.android.features.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.str
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject

/** How long the phone asks about a Tidal sign-in: two device-code steps of 5 minutes each, and some margin. */
private const val TIDAL_WAIT_MS = 11 * 60_000L

/** Failed calls in a row after which the phone stops asking (the server can't be reached). */
private const val TIDAL_MAX_FAILURES = 10

/**
 * Under the streamrip card: the Tidal account Encore downloads with (web-rs/src/settings/apps.rs tidal_block). Tidal
 * needs a one-time sign-in, not an API key: the server gets a link (device code, one step per streamrip "lane"),
 * the phone opens it in a browser tab over the app and asks the server every 3 s until the sign-in is done. When
 * Tidal blocks the server's address, the [tidal] section of a working streamrip config can be pasted instead.
 */
@Composable
fun TidalSignIn(api: SettingsApi) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<JsonObject?>(null) }
    var unavailable by remember { mutableStateOf(false) }
    var starting by remember { mutableStateOf(false) }
    var startError by remember { mutableStateOf("") }
    var polling by remember { mutableIntStateOf(0) }
    var opened by remember { mutableStateOf("") }
    var pasteOpen by remember { mutableStateOf(false) }
    var toml by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }

    fun take(r: JsonObject) {
        status = r
        if (r.str("message").contains("paste the tokens")) pasteOpen = true
        // each step has its own link: open the new one when the server moves to step 2
        val url = r.str("url")
        if (r.str("status") == "waiting" && url.isNotEmpty() && url != opened) {
            opened = url
            openSignInTab(context, url, p.bg)
        }
    }

    LaunchedEffect(Unit) {
        try {
            status = withContext(Dispatchers.IO) { api.tidalLoginStatus() }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            unavailable = true
        }
    }
    // one poll at a time: a new Sign in restarts it; it stops when the card closes, waits while the app is in the
    // background, and gives up after 11 minutes (two steps of 5 minutes and some margin) or 10 failed calls in a row
    val onScreen = rememberOnScreen()
    LaunchedEffect(polling) {
        if (polling == 0) return@LaunchedEffect
        val since = System.currentTimeMillis()
        var failures = 0
        while (true) {
            delay(3000)
            onScreen.awaitOnScreen()
            val timeUp = System.currentTimeMillis() - since > TIDAL_WAIT_MS
            val r = try {
                withContext(Dispatchers.IO) { api.tidalLoginStatus() }
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                failures++
                null
            }
            if (r != null) {
                failures = 0
                take(r)
                if (r.str("status") != "waiting") {
                    starting = false
                    break
                }
            }
            // still waiting (or not answering) after the last look: give up
            if (timeUp || failures >= TIDAL_MAX_FAILURES) {
                startError = "Sign-in failed"
                starting = false
                break
            }
        }
    }

    SetCard(Modifier.padding(top = 8.dp), flat = true) {
        val r = status
        val (line, signed) = if (r != null) SettingsRules.tidalState(r) else (if (unavailable) "Status unavailable" else "Checking…") to false
        Text("Tidal account", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Bold)
        Muted(line)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            SmallButton(if (signed) "Sign in again" else "Sign in to Tidal", iconRes = R.drawable.ic_link, enabled = !starting) {
                starting = true
                startError = ""
                scope.launch {
                    try {
                        val res = withContext(Dispatchers.IO) { api.tidalLoginStart() }
                        take(res)
                        if (res.str("status") == "waiting") polling += 1 else starting = false
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        startError = e.message ?: "Sign-in failed"
                        starting = false
                    }
                }
            }
            if (signed) {
                TextLink("Sign out of Tidal", color = p.muted) {
                    scope.serverCall(ok = "Signed out of Tidal", after = { res: JsonObject -> status = res; opened = "" }) { api.tidalTokensClear() }
                }
            }
        }
        val st = r?.str("status").orEmpty()
        when {
            startError.isNotEmpty() -> Badge(startError, kind = "err")
            starting && r?.str("status") != "waiting" -> Row(verticalAlignment = Alignment.CenterVertically) { Spinner(12.dp); Spacer(Modifier.width(6.dp)); Muted("Starting sign-in…") }
            st == "waiting" && r != null && r.str("url").isNotEmpty() -> {
                val url = r.str("url")
                // the link opens again from here (the tab may have been closed before approving)
                Text(
                    buildAnnotatedString {
                        append("${SettingsRules.tidalStepPrefix(r)}Open ")
                        withStyle(SpanStyle(color = p.accent)) { append(url) }
                        append(" on any device, approve in your Tidal account, then come back here.")
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.clickable { openSignInTab(context, url, p.bg) },
                )
                Row(verticalAlignment = Alignment.CenterVertically) { Spinner(12.dp); Spacer(Modifier.width(6.dp)); Muted("Waiting…") }
            }
            st == "done" && r != null -> Badge(r.str("message"), kind = "ok")
            st == "failed" && r != null -> Badge(r.str("message").ifEmpty { "Sign-in failed" }, kind = "err")
        }
        TextLink("Paste tokens instead", color = p.text) { pasteOpen = !pasteOpen }
        if (pasteOpen) {
            Muted("On a computer with Tidal access: install streamrip (pip install streamrip), run rip url https://tidal.com/browse/track/1, approve the link it prints, then paste the [tidal] section of its config file here (rip config path shows where it is).")
            OutlinedTextField(
                value = toml,
                onValueChange = { toml = it },
                placeholder = { Text("[tidal]\nuser_id = \"…\"\naccess_token = \"…\"") },
                minLines = 6,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
                textStyle = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.fillMaxWidth(),
            )
            SmallButton("Save tokens", iconRes = R.drawable.ic_check, enabled = !saving && toml.isNotBlank()) {
                saving = true
                scope.launch {
                    try {
                        val res = withContext(Dispatchers.IO) { api.tidalTokensSave(toml) }
                        status = res
                        toml = ""
                        Toasts.show("Tidal tokens saved", "ok")
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: "Something went wrong", "err")
                    }
                    saving = false
                }
            }
        }
    }
}
