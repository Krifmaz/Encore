package app.encore.android.features.settings

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.PersistableBundle
import android.view.Window
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import app.encore.android.R
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// The pieces the Settings pages share: the page frame with its back arrow, the web's cards, switches, drop-downs,
// segmented choices, stat tiles and result lines, the browser tab for sign-ins, and the clipboard.

/** One Settings page: a back arrow and the title, then [content] in a scrolling column with the page gutter. */
@Composable
fun SettingsPage(
    title: String,
    onBack: () -> Unit,
    subtitle: String = "",
    /** false: the content brings its own side gutter (sections shared with other screens) */
    gutter: Boolean = true,
    actions: @Composable () -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    BackHandler(onBack = onBack)
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(top = 4.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
            Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            actions()
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = if (gutter) 16.dp else 0.dp)) {
            if (subtitle.isNotEmpty()) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(bottom = 12.dp))
            content()
            Spacer(Modifier.height(32.dp))
        }
    }
}

/** The web's `.card` (or `.card.flat` for the blocks inside a card): a bordered panel with its content in a column. */
@Composable
fun SetCard(modifier: Modifier = Modifier, flat: Boolean = false, content: @Composable ColumnScope.() -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(if (flat) 10.dp else 14.dp)
    Column(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(if (flat) p.panel2 else p.panel)
            .border(1.dp, p.border, shape)
            .padding(horizontal = 14.dp, vertical = if (flat) 12.dp else 14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
        content = content,
    )
}

/** A card's h2 with an optional small button on the right. */
@Composable
fun CardHead(title: String, modifier: Modifier = Modifier, trailing: @Composable () -> Unit = {}) {
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        trailing()
    }
}

/** The web's `.section-head` inside a page: an h2 and an optional line under it. */
@Composable
fun PageSection(title: String, subtitle: String = "", trailing: @Composable () -> Unit = {}) {
    Row(Modifier.fillMaxWidth().padding(top = 22.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            if (subtitle.isNotEmpty()) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted)
        }
        trailing()
    }
}

/** Small muted text (`.small.muted`). */
@Composable
fun Muted(text: String, modifier: Modifier = Modifier, color: Color = MaterialTheme.palette.muted) {
    if (text.isEmpty()) return
    Text(text, style = MaterialTheme.typography.bodySmall, color = color, modifier = modifier)
}

/**
 * Under a setting an environment variable sets, on the hand-written pages (as schema_form.rs `source_note`):
 * "🔒 Set by ENV in your compose/env – change it there (environment always wins)".
 */
@Composable
fun EnvLockedNote(env: String, modifier: Modifier = Modifier.padding(top = 4.dp)) {
    Text(
        androidx.compose.ui.text.buildAnnotatedString {
            append("🔒 Set by ")
            pushStyle(androidx.compose.ui.text.SpanStyle(fontFamily = FontFamily.Monospace))
            append(env.ifEmpty { "an environment variable" })
            pop()
            append(" in your compose/env – change it there (environment always wins)")
        },
        style = MaterialTheme.typography.bodySmall,
        color = EncoreColors.Info,
        modifier = modifier,
    )
}

/** Monospaced small text (`.mono`, `.code`). */
@Composable
fun Mono(text: String, modifier: Modifier = Modifier, color: Color = MaterialTheme.palette.text) {
    Text(text, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, color = color, modifier = modifier)
}

/** The web's `label.switch`: a switch with its label (and the help line under it). */
@Composable
fun SwitchRow(label: String, checked: Boolean, enabled: Boolean = true, help: String = "", bold: Boolean = false, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(enabled = enabled) { onChange(!checked) }.padding(vertical = 4.dp),
        verticalAlignment = if (help.isEmpty()) Alignment.CenterVertically else Alignment.Top,
    ) {
        Switch(checked = checked, onCheckedChange = { onChange(it) }, enabled = enabled)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f).alpha(if (enabled) 1f else 0.6f).padding(top = if (help.isEmpty()) 0.dp else 4.dp)) {
            Text(label, style = MaterialTheme.typography.bodyMedium, fontWeight = if (bold) FontWeight.SemiBold else null)
            if (help.isNotEmpty()) Muted(help)
        }
    }
}

/** A labelled drop-down (the web's `<select>`). */
@Composable
fun Dropdown(label: String?, options: List<Pair<String, String>>, selected: String, enabled: Boolean = true, modifier: Modifier = Modifier, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    var open by remember { mutableStateOf(false) }
    Column(modifier.padding(vertical = 4.dp)) {
        if (label != null) Text(label, style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(bottom = 4.dp))
        Box {
            Row(
                Modifier
                    .clip(RoundedCornerShape(8.dp))
                    .background(p.panel2)
                    .border(1.dp, p.border, RoundedCornerShape(8.dp))
                    .clickable(enabled = enabled) { open = true }
                    .padding(horizontal = 12.dp, vertical = 10.dp)
                    .alpha(if (enabled) 1f else 0.6f),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(options.firstOrNull { it.first == selected }?.second ?: selected.ifEmpty { "–" }, Modifier.weight(1f, fill = false), style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.width(6.dp))
                Icon(painterResource(R.drawable.ic_expand_more), contentDescription = null, tint = p.muted)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                for ((v, l) in options) {
                    DropdownMenuItem(text = { Text(l, fontWeight = if (v == selected) FontWeight.SemiBold else null) }, onClick = { open = false; onSelect(v) })
                }
            }
        }
    }
}

/** The web's `.seg` / `.tabs`: a row of choices, one on. */
@Composable
fun SegRow(options: List<Pair<String, String>>, selected: String, enabled: Boolean = true, onSelect: (String) -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(p.panel2).padding(3.dp).alpha(if (enabled) 1f else 0.6f),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        for ((v, l) in options) {
            val on = v == selected
            Box(
                Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) p.panel else p.panel2)
                    .clickable(enabled = enabled) { onSelect(v) }
                    .padding(vertical = 8.dp, horizontal = 4.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(l, style = MaterialTheme.typography.labelLarge, color = if (on) p.text else p.muted, fontWeight = if (on) FontWeight.SemiBold else null, maxLines = 2)
            }
        }
    }
}

/** The web's `.stats`: big numbers with their words under them. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun StatTiles(stats: List<Pair<String, String>>) {
    val p = MaterialTheme.palette
    FlowRow(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for ((n, label) in stats) {
            Column(
                Modifier.widthIn(min = 140.dp).clip(RoundedCornerShape(12.dp)).background(p.panel2).border(1.dp, p.border, RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 10.dp),
            ) {
                Text(n, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                Text(label, style = MaterialTheme.typography.bodySmall, color = p.muted)
            }
        }
    }
}

/** A Test result or a warning (`.test-result ok|err`). */
@Composable
fun ResultBox(ok: Boolean, message: String, modifier: Modifier = Modifier) {
    val tone = if (ok) EncoreColors.Ok else EncoreColors.Err
    val shape = RoundedCornerShape(8.dp)
    Row(
        modifier.fillMaxWidth().clip(shape).background(tone.copy(alpha = 0.10f)).border(1.dp, tone.copy(alpha = 0.35f), shape).padding(horizontal = 10.dp, vertical = 8.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(painterResource(if (ok) R.drawable.ic_check else R.drawable.ic_close), contentDescription = null, tint = tone, modifier = Modifier.size(16.dp))
        Spacer(Modifier.width(8.dp))
        Text(message, style = MaterialTheme.typography.bodySmall)
    }
}

/** The status dot of a list row (`.dot ok|err`). */
@Composable
fun Dot(ok: Boolean, modifier: Modifier = Modifier) {
    Box(modifier.size(9.dp).clip(CircleShape).background(if (ok) EncoreColors.Ok else EncoreColors.Err))
}

/** A link-like text button (`a.small`). */
@Composable
fun TextLink(text: String, modifier: Modifier = Modifier, color: Color = MaterialTheme.palette.accent, enabled: Boolean = true, onClick: () -> Unit) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = color,
        modifier = modifier.clip(RoundedCornerShape(4.dp)).clickable(enabled = enabled, onClick = onClick).padding(vertical = 4.dp, horizontal = 2.dp).alpha(if (enabled) 1f else 0.5f),
    )
}

/** The seconds since 1970, ticking every [everyMs] so "3 min ago" lines move on. */
@Composable
fun rememberNow(everyMs: Long = 30_000L): Double {
    var now by remember { mutableDoubleStateOf(System.currentTimeMillis() / 1000.0) }
    LaunchedEffect(everyMs) {
        while (true) {
            delay(everyMs)
            now = System.currentTimeMillis() / 1000.0
        }
    }
    return now
}

/** Runs [onResume] each time the app comes back to the front (back from a browser tab). */
@Composable
fun OnResume(onResume: () -> Unit) {
    val context = LocalContext.current
    // the activity's lifecycle (as UpdatesSection does): it is resumed again when the browser tab closes
    val owner = remember(context) { context.findActivity() as? ComponentActivity }
    val latest by rememberUpdatedState(onResume)
    DisposableEffect(owner) {
        val lifecycle = owner?.lifecycle
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) latest() }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer) }
    }
}

/**
 * Whether the app is on screen (the activity is at least started), kept up to date: a poll waits on it so it asks
 * nothing while the app is in the background (a Compose effect is not paused when the activity stops). True where
 * there is no activity to follow.
 */
@Composable
fun rememberOnScreen(): State<Boolean> {
    val context = LocalContext.current
    val owner = remember(context) { context.findActivity() as? ComponentActivity }
    val onScreen = remember(owner) { mutableStateOf(owner?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.STARTED) ?: true) }
    DisposableEffect(owner) {
        val lifecycle = owner?.lifecycle
        val observer = LifecycleEventObserver { _, _ -> onScreen.value = lifecycle?.currentState?.isAtLeast(Lifecycle.State.STARTED) ?: true }
        lifecycle?.addObserver(observer)
        onDispose { lifecycle?.removeObserver(observer) }
    }
    return onScreen
}

/** Suspends until the app is on screen again ([rememberOnScreen]); returns at once when it is. */
suspend fun State<Boolean>.awaitOnScreen() {
    if (!value) snapshotFlow { value }.first { it }
}

/** Windows kept secure, with how many places asked for it (the flag goes when the last one lets go). */
private val secureHolds = java.util.WeakHashMap<Window, Int>()

/**
 * While [on] (a secret is shown as text), the window is kept out of screenshots and of the Recents thumbnail
 * (FLAG_SECURE), as a password field would be.
 */
@Composable
fun SecureWindow(on: Boolean) {
    if (!on) return
    val context = LocalContext.current
    DisposableEffect(context) {
        val window = context.findActivity()?.window
        if (window != null) {
            val n = secureHolds[window] ?: 0
            secureHolds[window] = n + 1
            if (n == 0) window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        }
        onDispose {
            if (window != null) {
                val left = (secureHolds[window] ?: 1) - 1
                if (left <= 0) {
                    secureHolds.remove(window)
                    window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
                } else {
                    secureHolds[window] = left
                }
            }
        }
    }
}

/**
 * Runs a blocking server call on the IO dispatcher from a button: [ok] is toasted when it worked and [after] gets
 * the answer; the server's words are toasted (or given to [onError]) when it did not.
 */
fun <T> CoroutineScope.serverCall(ok: String = "", onError: ((String) -> Unit)? = null, after: (T) -> Unit = {}, call: () -> T) {
    launch {
        try {
            val r = withContext(Dispatchers.IO) { call() }
            if (ok.isNotEmpty()) Toasts.show(ok, "ok")
            after(r)
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            val msg = e.message ?: "Something went wrong"
            if (onError != null) onError(msg) else Toasts.show(msg, "err")
        }
    }
}

/**
 * Opens a sign-in page in a browser tab over the app (a Custom Tab where the browser has them; any browser page
 * otherwise). androidx.browser is not a dependency: the Custom Tabs protocol is an ACTION_VIEW intent carrying the
 * session extra (a null binder asks for a tab without a session) and the toolbar colour, which is all
 * CustomTabsIntent.Builder().build() puts in. Only http(s) addresses are opened. False when nothing could open it.
 */
fun openSignInTab(context: Context, url: String, toolbar: Color? = null): Boolean {
    val uri = Uri.parse(url)
    val scheme = uri.scheme?.lowercase()
    if (scheme != "https" && scheme != "http") return false
    val intent = Intent(Intent.ACTION_VIEW, uri)
    intent.putExtras(Bundle().apply { putBinder(EXTRA_SESSION, null) })
    if (toolbar != null) intent.putExtra(EXTRA_TOOLBAR_COLOR, toolbar.toArgb())
    if (context.findActivity() == null) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    return try {
        context.startActivity(intent)
        true
    } catch (_: ActivityNotFoundException) {
        Toasts.show("No browser on this phone can open $url", "err")
        false
    } catch (_: SecurityException) {
        false
    }
}

private const val EXTRA_SESSION = "android.support.customtabs.extra.SESSION"
private const val EXTRA_TOOLBAR_COLOR = "android.support.customtabs.extra.TOOLBAR_COLOR"

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

/** Puts text on the clipboard ("Copied to clipboard"); [sensitive] hides it from the keyboard's clipboard preview. */
fun copyToClipboard(context: Context, text: String, sensitive: Boolean = false) {
    try {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: throw IllegalStateException()
        val clip = ClipData.newPlainText("Encore", text)
        // ClipDescription.EXTRA_IS_SENSITIVE (API 33); older versions ignore the extra
        if (sensitive) clip.description.extras = PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
        cm.setPrimaryClip(clip)
        Toasts.show("Copied to clipboard", "ok")
    } catch (_: Exception) {
        Toasts.show("Copy failed – select and copy manually", "err")
    }
}

/** The web's modal with a form: a title, the fields, Cancel / Close and the main button (none when [okLabel] is empty). */
@Composable
fun FormDialog(
    title: String,
    onDismiss: () -> Unit,
    okLabel: String = "",
    okEnabled: Boolean = true,
    cancelLabel: String = "Cancel",
    onOk: () -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val p = MaterialTheme.palette
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.text,
        title = { Text(title) },
        text = {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
        },
        confirmButton = {
            if (okLabel.isNotEmpty()) {
                androidx.compose.material3.TextButton(onClick = onOk, enabled = okEnabled) { Text(okLabel, color = p.text, fontWeight = FontWeight.SemiBold) }
            }
        },
        dismissButton = { androidx.compose.material3.TextButton(onClick = onDismiss) { Text(cancelLabel, color = p.muted) } },
    )
}

/** A text input of a form (`label.field`): its label above, the help line under it. */
@Composable
fun FormField(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    help: String = "",
    placeholder: String = "",
    password: Boolean = false,
    keyboard: androidx.compose.ui.text.input.KeyboardType = androidx.compose.ui.text.input.KeyboardType.Text,
    enabled: Boolean = true,
    minLines: Int = 1,
    mono: Boolean = false,
    modifier: Modifier = Modifier.fillMaxWidth(),
) {
    Column(modifier) {
        androidx.compose.material3.OutlinedTextField(
            value = value,
            onValueChange = onChange,
            label = if (label.isEmpty()) null else ({ Text(label) }),
            placeholder = if (placeholder.isEmpty()) null else ({ Text(placeholder, color = MaterialTheme.palette.faint) }),
            enabled = enabled,
            singleLine = minLines <= 1,
            minLines = minLines,
            visualTransformation = if (password) androidx.compose.ui.text.input.PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                keyboardType = if (password) androidx.compose.ui.text.input.KeyboardType.Password else keyboard,
                capitalization = androidx.compose.ui.text.input.KeyboardCapitalization.None,
                autoCorrectEnabled = !password && !mono && keyboard == androidx.compose.ui.text.input.KeyboardType.Text,
            ),
            textStyle = if (mono) MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace) else MaterialTheme.typography.bodyMedium,
            modifier = Modifier.fillMaxWidth(),
        )
        if (help.isNotEmpty()) Muted(help, Modifier.padding(top = 4.dp))
    }
}
