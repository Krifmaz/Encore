package app.encore.android.ui.components

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.delay

/**
 * The whole of a text a list cuts off (web-rs/src/details.rs): a dialog with the text wrapped and selectable, **Copy**
 * (the whole text) and Close. Used for log lines, transfer errors, torrent messages and app crashes. The toast
 * "Copied to clipboard" (or "Copy failed – select and copy manually") shows above the dialog, as toasts show above
 * every sheet and dialog (Toasts.kt).
 */
@Composable
fun DetailsDialog(title: String, text: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text(title.ifEmpty { "Details" }, maxLines = 2, overflow = TextOverflow.Ellipsis) },
        text = {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState())) {
                SelectionContainer {
                    Text(text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, modifier = Modifier.fillMaxWidth())
                }
            }
        },
        confirmButton = {
            TextButton(onClick = { copyToClipboard(context, text) }) {
                Text("Copy", color = p.text, fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Close", color = p.muted) } },
    )
}

/**
 * How the last Copy went, for a sheet that also says it on its own button: true, false, or null (none lately; clears
 * after 3 s). The toast says it too, above the sheet.
 */
class CopyState {
    var value: Boolean? by mutableStateOf(null)
}

@Composable
fun rememberCopied(): CopyState {
    val s = remember { CopyState() }
    val v = s.value
    LaunchedEffect(v) {
        if (v != null) {
            delay(3000)
            s.value = null
        }
    }
    return s
}

/** The Copy button's words in a sheet or dialog: "Copied to clipboard" for a moment after it worked. */
fun copyLabel(copied: Boolean?): String = if (copied == true) "Copied to clipboard" else "Copy"

/** A small Copy icon (the web's `details::copy_button`): copies [text] as it is. */
@Composable
fun CopyIconButton(text: String, modifier: Modifier = Modifier, size: Dp = 32.dp) {
    val context = LocalContext.current
    IconButton(onClick = { copyToClipboard(context, text) }, modifier = modifier.size(size)) {
        Icon(painterResource(R.drawable.ic_content_copy), contentDescription = "Copy", tint = MaterialTheme.palette.muted, modifier = Modifier.size(16.dp))
    }
}
