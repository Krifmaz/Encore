package app.encore.android.features.settings

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.CrashLog
import app.encore.android.R
import app.encore.android.data.session.AuthKind
import app.encore.android.features.bugs.BugReportsEntry
import app.encore.android.features.offline.OfflineStorageSection
import app.encore.android.features.update.UpdatesSection
import app.encore.android.remote.RemoteDevice
import app.encore.android.remote.RemoteUi
import app.encore.android.ui.Nav
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/**
 * Settings → This device: what belongs to this phone and not to the account – the server it is signed in to, the
 * name other devices see it under and the Devices sheet, the songs downloaded here (Music/Encore), the app's own
 * updates, bug reports to the admin, and the crash reports waiting to be sent.
 */
@Composable
fun ThisDeviceScreen(nav: Nav, onBack: () -> Unit) {
    // the sections bring their own side gutter
    SettingsPage("This device", onBack, gutter = false) {
        Column {
            SignedInRow(nav)
            DeviceNameRow(nav)
            // songs saved on this phone: the room they take, per-album remove, Remove all (features/offline)
            OfflineStorageSection(nav)
            UpdatesSection()
            // the reports to the admin, with the count of those that have something new (features/bugs)
            BugReportsEntry(nav)
            CrashReports(nav)
        }
    }
}

/** The server this phone is signed in to, and how (Settings showed it under Account before 0.30). */
@Composable
private fun SignedInRow(nav: Nav) {
    val s = nav.graph.store.current ?: return
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
        Text(s.server, style = MaterialTheme.typography.bodyLarge)
        Text(
            if (s.kind == AuthKind.BEARER) "Signed in with an app token" else "Signed in with a web session (expires after 30 days)",
            style = MaterialTheme.typography.bodySmall,
            color = p.muted,
        )
    }
}

/**
 * The name your other devices see this phone under (the web's `encore.device.name`; empty = the default
 * "Android app (<model>)"), and the Devices sheet.
 */
@Composable
fun DeviceNameRow(nav: Nav) {
    val remote = nav.player.remote
    val custom by remote.customName.collectAsState()
    val p = MaterialTheme.palette
    var text by remember { mutableStateOf(custom) }
    Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
        OutlinedTextField(
            value = text,
            onValueChange = { v -> text = v.take(60); remote.setName(text) },
            label = { Text("Device name") },
            placeholder = { Text(RemoteDevice.defaultName()) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            "Shown on your other devices, and in \"was playing on\" when another device offers to continue your music",
            style = MaterialTheme.typography.bodySmall,
            color = p.muted,
            modifier = Modifier.padding(top = 4.dp),
        )
    }
    Row(
        Modifier.fillMaxWidth().clickable { RemoteUi.openDevices() }.padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Your devices")
            Text("Play here, send your music to another device, or use this phone as a remote", style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        Icon(painterResource(R.drawable.ic_devices), contentDescription = null, tint = p.muted)
    }
}

/** The crash reports this phone keeps until the server has them (admins read them under Diagnostics → App crashes). */
@Composable
private fun CrashReports(nav: Nav) {
    val p = MaterialTheme.palette
    var version by remember { mutableIntStateOf(0) }
    // the count lists a folder: read off the main thread
    var waiting by remember { mutableIntStateOf(0) }
    LaunchedEffect(version) {
        if (version > 0) delay(2000)
        waiting = withContext(Dispatchers.IO) { CrashLog.pendingCount() }
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
        Text("App crashes", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Muted("The phone app sends a report the next time it opens after it crashed or the system closed it.")
        if (waiting > 0) {
            Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Muted("Waiting to send: $waiting", Modifier.weight(1f), color = p.text)
                SmallButton("Send now", iconRes = R.drawable.ic_refresh) {
                    CrashLog.uploadPending(nav.graph.app, nav.graph)
                    version += 1
                }
            }
        }
    }
}
