package app.encore.android.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import app.encore.android.AppGraph
import app.encore.android.data.session.ThemeChoice
import app.encore.android.features.group.GroupListen
import app.encore.android.features.update.Updater
import app.encore.android.graph
import app.encore.android.ui.theme.EncoreTheme
import app.encore.android.ui.theme.isDarkTheme
import androidx.compose.foundation.isSystemInDarkTheme

class MainActivity : ComponentActivity() {
    private lateinit var appGraph: AppGraph
    private lateinit var player: PlayerConnection

    // Android 13+: the media notification needs the notification permission.
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        appGraph = graph
        player = PlayerConnection(applicationContext, appGraph.api)
        applySystemBars(appGraph.store.settings.value.theme != ThemeChoice.LIGHT)
        askForNotifications()
        // the app's own update: at most once a day, when "Look for a new version when Encore starts" is on
        Updater.checkAtStart(this)
        // a group listen link shared to the app (not again when the activity is only recreated)
        if (savedInstanceState == null) handleIntent(intent)
        setContent {
            val settings by appGraph.store.settings.collectAsState()
            // Match device follows the system's dark mode
            val dark = isDarkTheme(settings, isSystemInDarkTheme())
            LaunchedEffect(dark) { applySystemBars(dark) }
            EncoreTheme(settings) {
                EncoreRoot(appGraph, player)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleIntent(intent)
    }

    /** `https://<server>/#/group/<code>` (from ShareActivity): the root asks "Join <host>'s group listen?" once signed in. */
    private fun handleIntent(i: Intent?) {
        if (i == null) return
        val link = i.getStringExtra(EXTRA_GROUP_LINK) ?: i.dataString?.takeIf { it.contains("#/group/") } ?: return
        i.removeExtra(EXTRA_GROUP_LINK)
        GroupListen.pendingLink.value = link
    }

    override fun onStart() {
        super.onStart()
        player.connect()
        appGraph.likes.refresh()
    }

    override fun onStop() {
        // released so the service can stop when nothing plays; it keeps playing while it does
        player.release()
        super.onStop()
    }

    private fun applySystemBars(dark: Boolean) {
        val style = if (!dark) {
            SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        } else {
            SystemBarStyle.dark(Color.TRANSPARENT)
        }
        enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
    }

    companion object {
        /** A group listen link or code for the root to open (ShareActivity). */
        const val EXTRA_GROUP_LINK = "app.encore.android.GROUP_LINK"
    }

    private fun askForNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}
