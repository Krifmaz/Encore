package app.encore.android.features.update

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.encore.android.BuildConfig
import app.encore.android.R
import app.encore.android.features.update.UpdateRules.Phase
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatSize
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette

/**
 * Settings → This device → Updates (the web's "Updates" block of This computer, for the phone): the version, "Look
 * for a new version when Encore starts", the updater's line, Check for updates, Install <version> with the notes of
 * the release ("What changed in …") and View on GitHub. [Updater] does the work.
 */
@Composable
fun UpdatesSection(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val p = MaterialTheme.palette
    val st by Updater.state.collectAsState()
    LaunchedEffect(Unit) { Updater.load(context) }

    // back from Android's "Install unknown apps" page: the install goes on
    val activity = remember(context) { context.findActivity() }
    DisposableEffect(activity) {
        val lifecycle = activity?.lifecycle
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) Updater.onResume(context)
        }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer) }
    }

    Column(
        modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("Updates", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Text("Version ${BuildConfig.VERSION_NAME}", style = MaterialTheme.typography.bodySmall, color = p.muted)
        if (!Updater.canUpdateItself(context)) {
            Text(
                "This build can't update itself. New versions are on github.com/Krifmaz/Encore/releases.",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
            )
            SmallButton("Releases page") { openUrl(context, UpdateRules.RELEASES) }
            return@Column
        }

        Row(
            Modifier.fillMaxWidth().clickable { Updater.setAuto(context, !st.auto) },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(checked = st.auto, onCheckedChange = { Updater.setAuto(context, it) })
            Text("Look for a new version when Encore starts", Modifier.weight(1f))
        }

        val phase = st.phase
        val line = UpdateRules.describe(phase, ::formatSize)
        if (line.isNotEmpty()) {
            Text(
                line,
                style = MaterialTheme.typography.bodySmall,
                color = if (phase is Phase.Failed) EncoreColors.Err else p.muted,
            )
        }
        if (phase is Phase.Downloading) {
            if (phase.total > 0) {
                val frac = (phase.got.toFloat() / phase.total.toFloat()).coerceIn(0f, 1f)
                LinearProgressIndicator(progress = { frac }, modifier = Modifier.fillMaxWidth())
            } else {
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
            }
        }

        val busy = UpdateRules.busy(phase)
        val rel = st.found
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            SmallButton("Check for updates", modifier = Modifier.alpha(if (busy) 0.5f else 1f), enabled = !busy) {
                Updater.checkNow(context)
            }
            if (rel != null) {
                // while Android waits for the confirm screen, the button opens it again
                val canPress = !busy || (phase as? Phase.Installing)?.confirm == true
                SmallButton(
                    "Install ${rel.version}",
                    modifier = Modifier.alpha(if (canPress) 1f else 0.5f),
                    primary = true,
                    enabled = canPress,
                ) { Updater.install(context) }
            }
        }

        if (rel != null) {
            val day = releasedDay(rel.publishedAt)
            if (day.isNotEmpty()) Text("released $day", style = MaterialTheme.typography.bodySmall, color = p.faint)
            val notes = remember(rel.notes) { UpdateRules.notesText(rel.notes) }
            if (notes.isNotEmpty()) {
                var open by remember(rel.version) { mutableStateOf(false) }
                Row(
                    Modifier.clickable { open = !open }.padding(vertical = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("What changed in ${rel.version}", style = MaterialTheme.typography.bodySmall, color = p.muted)
                    Icon(
                        painterResource(R.drawable.ic_expand_more),
                        contentDescription = null,
                        tint = p.muted,
                        modifier = Modifier.size(18.dp).rotate(if (open) 180f else 0f),
                    )
                }
                if (open) {
                    Text(
                        notes + if (rel.notesCut) "\n…" else "",
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            SmallButton("View on GitHub") { openUrl(context, rel.page) }
        }
        if (Updater.isDevBuild) {
            Text(
                "A build without a release number: any release counts as newer.",
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
            )
        }
    }
}

/** GitHub's `published_at` (an ISO time) as a day in the phone's language; "" when missing. */
private fun releasedDay(iso: String): String {
    if (iso.isBlank()) return ""
    return try {
        formatDay(java.time.Instant.parse(iso).epochSecond.toDouble())
    } catch (_: Exception) {
        ""
    }
}

private fun openUrl(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
    }
}

private fun Context.findActivity(): ComponentActivity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is ComponentActivity) return c
        c = c.baseContext
    }
    return null
}
