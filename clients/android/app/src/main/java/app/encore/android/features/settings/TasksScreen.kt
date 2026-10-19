package app.encore.android.features.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.objectsOf
import app.encore.android.core.arr
import app.encore.android.core.str
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.RowDivider
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Scheduled tasks, above the page's Playlists & sync card (web-rs/src/settings/tasks.rs): every
 * background job with on/off, how often it runs, its last run and result and Run now; and how long finished
 * downloads are kept.
 */
@Composable
fun TaskList(api: SettingsApi) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val now = rememberNow()
    var version by remember { mutableIntStateOf(0) }
    var data by remember { mutableStateOf<JsonObject?>(null) }
    var failed by remember { mutableStateOf("") }
    var days by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(version) {
        try {
            val d = withContext(Dispatchers.IO) { api.tasks() }
            data = d
            failed = ""
            if (days == null) days = SchemaRules.textOf(d["cleanup_days"])
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            failed = e.message ?: "Something went wrong"
        }
    }
    fun patch(key: String, body: JsonObject, ok: String) {
        scope.serverCall(ok = ok, after = { _: Unit -> version += 1 }) { api.patchTask(key, body) }
    }
    SetCard(Modifier.padding(bottom = 16.dp)) {
        CardHead("Scheduled tasks") { SmallButton("Refresh", iconRes = R.drawable.ic_refresh) { version += 1 } }
        Muted("Background jobs Encore runs on its own. Switch them off, change how often they run, or run one now.")
        val d = data
        when {
            d == null && failed.isNotEmpty() -> Text(failed, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
            d == null -> Spinner(18.dp)
            else -> {
                val tasks = d.arr("tasks").objectsOf()
                tasks.forEachIndexed { i, t ->
                    if (i > 0) RowDivider(startIndent = 0.dp)
                    TaskRow(t, now, onToggle = { on -> patch(t.str("key"), JsonObject(mapOf("enabled" to JsonPrimitive(on))), if (on) "Enabled" else "Disabled") },
                        onEvery = { m -> patch(t.str("key"), JsonObject(mapOf("minutes" to SettingsRules.jsPlus(m))), "Saved") },
                        onRun = {
                            scope.launch {
                                try {
                                    withContext(Dispatchers.IO) { api.runTask(t.str("key")) }
                                    Toasts.show("Started", "ok")
                                    delay(1500)
                                    version += 1
                                } catch (e: CancellationException) {
                                    throw e
                                } catch (e: Exception) {
                                    Toasts.show(e.message ?: "Something went wrong", "err")
                                }
                            }
                        })
                }
                Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Muted("Clean up finished downloads older than")
                    OutlinedTextField(
                        value = days ?: "",
                        onValueChange = { days = it },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Done),
                        keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = {
                            val v = SettingsRules.jsPlus(days ?: "")
                            scope.serverCall(ok = "Saved") { api.putSettings(JsonObject(mapOf("task_cleanup_days" to v))) }
                        }),
                        modifier = Modifier.width(90.dp),
                    )
                    Muted("days")
                }
                SmallButton("Save", iconRes = R.drawable.ic_check) {
                    val v = SettingsRules.jsPlus(days ?: "")
                    scope.serverCall(ok = "Saved") { api.putSettings(JsonObject(mapOf("task_cleanup_days" to v))) }
                }
            }
        }
    }
}

@Composable
private fun TaskRow(t: JsonObject, now: Double, onToggle: (Boolean) -> Unit, onEvery: (String) -> Unit, onRun: () -> Unit) {
    val p = MaterialTheme.palette
    val locked = t.bool("locked")
    val running = t.bool("running")
    Column(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Switch(checked = t.bool("enabled"), onCheckedChange = onToggle, enabled = !locked)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(t.str("name") + if (locked) " 🔒" else "", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                Muted(t.str("desc"))
                Muted(SettingsRules.taskLine(t, now), color = if (running) p.accent else p.faint)
                if (t.str("error").isNotEmpty()) Text(t.str("error"), style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err)
                if (locked) Muted("Set by an environment variable", color = p.faint)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Muted("every")
            Dropdown(null, SettingsRules.everyOptions(t.dbl("minutes")), SettingsRules.num(t.dbl("minutes")), enabled = !locked) { onEvery(it) }
            SmallButton("Run now", iconRes = R.drawable.ic_play, enabled = !running) { onRun() }
        }
    }
}
