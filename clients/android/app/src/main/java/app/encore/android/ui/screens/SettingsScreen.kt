package app.encore.android.ui.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.encore.android.BuildConfig
import app.encore.android.data.session.AuthKind
import app.encore.android.data.session.Session
import app.encore.android.data.session.ThemeChoice
import app.encore.android.playback.ResumeStore
import app.encore.android.ui.Nav
import app.encore.android.ui.components.SectionTitle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

@Composable
fun SettingsScreen(nav: Nav, session: Session) {
    val store = nav.graph.store
    val settings by store.settings.collectAsState()
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    Column(Modifier.fillMaxSize()) {
        BackBar("Settings", nav.back)
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            SectionTitle("Account")
            Column(Modifier.padding(horizontal = 16.dp)) {
                Text(session.username, style = MaterialTheme.typography.bodyLarge)
                Text(session.server, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(
                    if (session.kind == AuthKind.BEARER) "Signed in with an app token" else "Signed in with a web session (expires after 30 days)",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Spacer(Modifier.height(12.dp))
                OutlinedButton(onClick = {
                    nav.player.stopAndClear()
                    ResumeStore(context).clear()
                    scope.launch {
                        withContext(Dispatchers.IO) { nav.graph.api.logout() }
                        nav.graph.likes.clear()
                        store.signOut()
                    }
                }) { Text("Sign out") }
            }

            SectionTitle("Appearance")
            ThemeOption("Encore (charcoal and cream)", settings.theme == ThemeChoice.ENCORE) { store.setTheme(ThemeChoice.ENCORE) }
            ThemeOption("OLED black", settings.theme == ThemeChoice.OLED) { store.setTheme(ThemeChoice.OLED) }
            ThemeOption("Light", settings.theme == ThemeChoice.LIGHT) { store.setTheme(ThemeChoice.LIGHT) }

            SectionTitle("Playback")
            Row(
                Modifier.fillMaxWidth().clickable { store.setAutoplay(!settings.autoplay) }.padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text("Then shuffle library")
                    Text(
                        "When the queue ends, keep playing a shuffle of the whole library",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Switch(checked = settings.autoplay, onCheckedChange = { store.setAutoplay(it) })
            }

            SectionTitle("About")
            Text(
                "Encore for Android ${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun ThemeOption(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = onClick)
        Text(label)
    }
}
