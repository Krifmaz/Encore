package app.encore.android.features.bugs

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.encore.android.BuildConfig
import app.encore.android.R
import app.encore.android.data.api.USER_AGENT
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** The phone's line for the web's browser line: the app's user agent (version, Android, model) and the screen in dp. */
internal fun deviceLine(context: Context): String {
    val c = context.resources.configuration
    return if (c.screenWidthDp > 0 && c.screenHeightDp > 0) "$USER_AGENT · ${c.screenWidthDp}x${c.screenHeightDp}" else USER_AGENT
}

/**
 * New report (the web's New report dialog): Bug · Not a bug · How do I…, a title, what happened, up to six pictures;
 * the page it was opened from ([page]), the app's version and the phone go along. Sent as one multipart upload to
 * POST /api/bug-reports; the new report opens on success.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewBugReportSheet(nav: Nav, page: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val picker = rememberAttachmentPicker()
    var kind by remember { mutableStateOf("bug") }
    var title by remember { mutableStateOf("") }
    var body by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    // the sheet stays while the report is on its way
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true, confirmValueChange = { it != SheetValue.Hidden || !sending })
    val version = BuildConfig.VERSION_NAME

    fun send() {
        val t = title.trim()
        val text = body.trim()
        when {
            t.isEmpty() -> { Toasts.show("Give the report a title", "err"); return }
            text.isEmpty() -> { Toasts.show("Say what happened", "err"); return }
            picker.busy -> return
        }
        sending = true
        val files = picker.files.toList()
        scope.launch {
            try {
                val id = withContext(Dispatchers.IO) {
                    nav.graph.bugReports.create(kind, t, text, page, version, deviceLine(context), files, context.contentResolver)
                }
                Toasts.show("Report sent. Thanks.", "ok")
                BugUnread.refresh(nav.graph)
                onDismiss()
                if (id > 0L) nav.open(Screen.BugReport(id))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(BugRules.errorWords(e), "err")
                sending = false
            }
        }
    }

    ModalBottomSheet(onDismissRequest = { if (!sending) onDismiss() }, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("New report", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(top = 8.dp))
            Segmented(BugRules.KINDS, kind) { kind = it }
            EncoreField(
                title,
                { title = it.take(BugRules.MAX_TITLE) },
                label = "Title",
                placeholder = "A short name for it",
                modifier = Modifier.fillMaxWidth(),
            )
            EncoreField(
                body,
                { body = it.take(BugRules.MAX_BODY) },
                label = "Description",
                placeholder = "What happened, what you expected, how to repeat it",
                singleLine = false,
                minLines = 6,
                imeAction = ImeAction.Default,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                SmallButton("Add images", iconRes = R.drawable.ic_add, enabled = !sending) { picker.add() }
                Text(
                    "Up to ${BugRules.MAX_FILES} files, 8 MB each.",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.weight(1f),
                )
            }
            AttachmentThumbs(picker, nav.graph.imageLoader)
            Text(BugRules.sentAlong(page, version), style = MaterialTheme.typography.bodySmall, color = p.muted)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Cancel", enabled = !sending) { onDismiss() }
                EncoreButton(if (sending) "Sending…" else "Send", primary = true, enabled = !sending && !picker.busy) { send() }
            }
        }
    }
}

/** The web's `.seg`: a few choices side by side, one on. */
@Composable
internal fun Segmented(options: List<Pair<String, String>>, selected: String, modifier: Modifier = Modifier, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    val outer = RoundedCornerShape(10.dp)
    Row(modifier.clip(outer).background(p.bg2).border(1.dp, p.border, outer).padding(3.dp), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        for ((key, label) in options) {
            val on = key == selected
            Box(
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) p.panel2 else Color.Transparent)
                    .clickable { onSelect(key) }
                    .padding(horizontal = 12.dp, vertical = 7.dp),
            ) {
                Text(label, style = MaterialTheme.typography.labelLarge, color = if (on) p.text else p.muted, maxLines = 1)
            }
        }
    }
}
