package app.encore.android.ui

import android.Manifest
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
import app.encore.android.graph
import app.encore.android.ui.theme.EncoreTheme

class MainActivity : ComponentActivity() {
    private lateinit var appGraph: AppGraph
    private lateinit var player: PlayerConnection

    // Android 13+: the media notification needs the notification permission.
    private val notificationPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        appGraph = graph
        player = PlayerConnection(applicationContext, appGraph.api)
        applySystemBars(appGraph.store.settings.value.theme)
        askForNotifications()
        setContent {
            val settings by appGraph.store.settings.collectAsState()
            LaunchedEffect(settings.theme) { applySystemBars(settings.theme) }
            EncoreTheme(settings.theme) {
                EncoreRoot(appGraph, player)
            }
        }
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

    private fun applySystemBars(theme: ThemeChoice) {
        val style = if (theme == ThemeChoice.LIGHT) {
            SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        } else {
            SystemBarStyle.dark(Color.TRANSPARENT)
        }
        enableEdgeToEdge(statusBarStyle = style, navigationBarStyle = style)
    }

    private fun askForNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}
