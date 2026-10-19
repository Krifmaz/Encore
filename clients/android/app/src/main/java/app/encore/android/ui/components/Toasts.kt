package app.encore.android.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Snackbar
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.receiveAsFlow

/**
 * Toasts like the web's `toast(msg, type)` (web/js/ui.js): a panel with a coloured left border that goes away
 * after about four seconds. Shared seam: any part calls [show] from any thread; [ToastHost] draws them once at
 * the root (EncoreRoot) through a SnackbarHost.
 *
 * `kind` is the web's type: "info" (the default, accent border), "ok" (green), "err" (red), "warn" (amber). An error
 * stays longer (about ten seconds) and has **Copy** (the whole message, for a bug report) and Close, as the web's error
 * toasts do; "Copy failed – select and copy manually" is the one error without a Copy of its own.
 */
object Toasts {
    class Toast(val message: String, val kind: String)

    /** The words when the clipboard refuses (the toast says what to do instead, so it has no Copy). */
    const val COPY_FAILED = "Copy failed – select and copy manually"

    /** An error toast with Copy and Close. */
    internal fun copyable(message: String, kind: String): Boolean = kind == "err" && message != COPY_FAILED

    private val channel = Channel<Toast>(capacity = 16, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    /** The pending toasts; collected by [ToastHost]. */
    val toasts = channel.receiveAsFlow()

    fun show(message: String, kind: String = "info") {
        if (message.isBlank()) return
        channel.trySend(Toast(message, kind))
    }
}

/** Draws [Toasts] at the bottom of its container; put it once in the root Box above the mini player. */
@Composable
fun ToastHost(modifier: Modifier = Modifier) {
    val hostState = remember { SnackbarHostState() }
    LaunchedEffect(Unit) {
        Toasts.toasts.collect { t ->
            // the kind rides along in the action label, which the custom Snackbar below reads and hides
            val long = Toasts.copyable(t.message, t.kind)
            hostState.showSnackbar(message = t.message, actionLabel = t.kind, duration = if (long) SnackbarDuration.Long else SnackbarDuration.Short, withDismissAction = false)
        }
    }
    val context = LocalContext.current
    SnackbarHost(hostState = hostState, modifier = modifier) { data ->
        val kind = data.visuals.actionLabel ?: "info"
        val copyable = Toasts.copyable(data.visuals.message, kind)
        val p = MaterialTheme.palette
        val edge: Color = when (kind) {
            "ok" -> EncoreColors.Ok
            "err" -> EncoreColors.Err
            "warn" -> EncoreColors.Warn
            else -> p.accent
        }
        Snackbar(
            containerColor = p.panel,
            contentColor = p.text,
            shape = RoundedCornerShape(12.dp),
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(4.dp).height(22.dp).clip(RoundedCornerShape(2.dp)).background(edge))
                Text(
                    data.visuals.message,
                    style = MaterialTheme.typography.bodyMedium,
                    modifier = Modifier.padding(start = 12.dp).weight(1f),
                )
                if (copyable) {
                    val message = data.visuals.message
                    TextButton(onClick = { data.dismiss(); copyToClipboard(context, message) }) {
                        Text("Copy", color = p.text, fontWeight = FontWeight.SemiBold)
                    }
                    IconButton(onClick = { data.dismiss() }, modifier = Modifier.size(32.dp)) {
                        Icon(painterResource(R.drawable.ic_close), contentDescription = "Close", tint = p.muted, modifier = Modifier.size(16.dp))
                    }
                }
            }
        }
    }
}
