package app.encore.android.ui.screens

import android.content.Context
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import app.encore.android.AppGraph
import app.encore.android.R
import app.encore.android.data.api.AuthInfo
import app.encore.android.data.api.LoginResult
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Server picker, then sign-in (with the authenticator code when the account has 2FA). */
@Composable
fun AuthFlow(graph: AppGraph) {
    var server by remember { mutableStateOf<Pair<String, AuthInfo>?>(null) }
    val s = server
    if (s == null) {
        ServerScreen(graph) { url, info -> server = url to info }
    } else {
        SignInScreen(graph, s.first, s.second, onBack = { server = null })
    }
}

@Composable
private fun ServerScreen(graph: AppGraph, onFound: (String, AuthInfo) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val saved by graph.store.servers.collectAsState()
    var url by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    // Suggest the last server: ours, or the one the Tauri app saved (SharedPreferences "encore"/"server").
    LaunchedEffect(Unit) {
        if (url.isEmpty()) {
            val last = graph.store.lastServer()
            url = last.ifEmpty { legacyServer(context) }
        }
    }

    fun go(target: String) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                val found = withContext(Dispatchers.IO) {
                    val base = graph.api.probe(target)
                    base to graph.api.info(base)
                }
                onFound(found.first, found.second)
            } catch (e: Exception) {
                error = e.message ?: "Can't reach that server"
            } finally {
                busy = false
            }
        }
    }

    AuthPage {
        Text("Encore", style = MaterialTheme.typography.displaySmall, fontWeight = FontWeight.SemiBold)
        Spacer(Modifier.height(4.dp))
        Text("Connect to your server", color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = url,
            onValueChange = { url = it; error = null },
            label = { Text("Server address") },
            placeholder = { Text("music.example.com") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
            modifier = Modifier.fillMaxWidth(),
        )
        error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(16.dp))
        Button(onClick = { go(url) }, enabled = !busy && url.isNotBlank(), modifier = Modifier.fillMaxWidth()) {
            if (busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text("Continue")
        }
        if (saved.isNotEmpty()) {
            Spacer(Modifier.height(28.dp))
            Text("Saved servers", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(4.dp))
            for (s in saved) {
                Row(
                    Modifier.fillMaxWidth().clickable { url = s; go(s) }.padding(vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(s, modifier = Modifier.weight(1f))
                    IconButton(onClick = { scope.launch { graph.store.forgetServer(s) } }) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Forget")
                    }
                }
            }
        }
    }
}

@Composable
private fun SignInScreen(graph: AppGraph, server: String, info: AuthInfo, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var username by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }
    var needsCode by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun submit() {
        if (busy || username.isBlank() || password.isEmpty()) return
        busy = true
        error = null
        scope.launch {
            try {
                graph.store.awaitLoaded()
                val result = withContext(Dispatchers.IO) {
                    graph.api.login(server, info, username, password, if (needsCode) code else null, graph.store.deviceId, deviceName())
                }
                when (result) {
                    is LoginResult.Success -> {
                        graph.likes.clear()
                        graph.dislikes.clear()
                        graph.store.signIn(result.session)
                    }
                    is LoginResult.NeedsCode -> {
                        if (needsCode) error = result.message
                        needsCode = true
                    }
                    is LoginResult.Failed -> error = result.message
                }
            } catch (e: Exception) {
                error = e.message ?: "Sign-in failed"
            } finally {
                busy = false
            }
        }
    }

    AuthPage {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
            Column {
                Text("Sign in to ${info.appName}", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
                Text(server, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        Spacer(Modifier.height(24.dp))
        OutlinedTextField(
            value = username,
            onValueChange = { username = it },
            label = { Text("Username") },
            singleLine = true,
            enabled = !needsCode,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text, imeAction = ImeAction.Next),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = password,
            onValueChange = { password = it },
            label = { Text("Password") },
            singleLine = true,
            enabled = !needsCode,
            visualTransformation = PasswordVisualTransformation(),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
            modifier = Modifier.fillMaxWidth(),
        )
        if (needsCode) {
            Spacer(Modifier.height(12.dp))
            Text("Enter the 6-digit code from your authenticator app", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(
                value = code,
                onValueChange = { v -> code = v.filter { it.isDigit() }.take(8) },
                label = { Text("Code") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        error?.let {
            Spacer(Modifier.height(8.dp))
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        Spacer(Modifier.height(16.dp))
        Button(
            onClick = { submit() },
            enabled = !busy && username.isNotBlank() && password.isNotEmpty() && (!needsCode || code.length >= 6),
            modifier = Modifier.fillMaxWidth(),
        ) {
            if (busy) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp) else Text("Sign in")
        }
        if (needsCode) {
            TextButton(onClick = { needsCode = false; code = ""; error = null }) { Text("Use a different account") }
        }
        if (!info.appTokens) {
            Spacer(Modifier.height(16.dp))
            Text(
                "This server signs apps in with a 30-day session. Update the server for long-lived app sign-in.",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun AuthPage(content: @Composable () -> Unit) {
    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .safeDrawingPadding()
            .verticalScroll(rememberScrollState())
            .padding(24.dp),
        verticalArrangement = Arrangement.Top,
    ) {
        Spacer(Modifier.height(48.dp))
        content()
    }
}

private fun deviceName(): String {
    val maker = Build.MANUFACTURER ?: ""
    val model = Build.MODEL ?: "Android"
    return if (maker.isNotEmpty() && !model.startsWith(maker, ignoreCase = true)) "${maker.replaceFirstChar { it.uppercase() }} $model" else model
}

private fun legacyServer(context: Context): String =
    try { context.getSharedPreferences("encore", Context.MODE_PRIVATE).getString("server", null) ?: "" } catch (_: Exception) { "" }
