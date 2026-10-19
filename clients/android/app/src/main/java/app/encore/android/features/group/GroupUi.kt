package app.encore.android.features.group

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Avatar
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.put

// The phone's group listen screens (0.31), with the web's words: the group sheet (code, invite link, members, the
// host's settings, Leave / End for everyone), Start a group listen, Join a group listen (a code or link), "Join
// <host>'s group listen?", the bar above the mini player ("Group · 3", "Paused here · Catch up"), the "Listening
// together now" row on Home, the devices sheet's "Group listens" section and Up next with "added by Sam".

/** A server path ("/api/users/3/avatar") as a URL of the signed-in server; full URLs stay. */
private fun avatarUrl(nav: Nav, path: String): String? = when {
    path.isEmpty() -> null
    path.startsWith("/") -> nav.graph.api.url(path)
    else -> path
}

/** The dialogs and the sheet, drawn once at the root (EncoreRoot). */
@Composable
fun GroupDialogs(nav: Nav) {
    val context = LocalContext.current
    LaunchedEffect(Unit) {
        GroupListen.init(context)
        GroupListen.restore()
    }
    val sheet by GroupListen.sheetOpen.collectAsState()
    val start by GroupListen.startOpen.collectAsState()
    val join by GroupListen.joinOpen.collectAsState()
    val invite by GroupListen.invite.collectAsState()
    val state by GroupListen.state.collectAsState()
    val s = state
    if (sheet && s != null) GroupSheet(nav, s, onDismiss = { GroupListen.sheetOpen.value = false })
    if (start) StartDialog(nav, onDismiss = { GroupListen.startOpen.value = false })
    if (join) JoinDialog(onDismiss = { GroupListen.joinOpen.value = false })
    invite?.let { inv -> InviteDialog(inv, onDismiss = { GroupListen.invite.value = null }) }
}

// ---- the bar above the mini player and the pill ---------------------------------------------------------------

/** The pill "Group · 3" (the full player's top bar): opens the group sheet. Nothing outside a group. */
@Composable
fun GroupPill(modifier: Modifier = Modifier) {
    val state by GroupListen.state.collectAsState()
    val s = state ?: return
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(99.dp)
    Row(
        modifier
            .clip(shape)
            .background(p.accent.copy(alpha = 0.16f))
            .border(1.dp, p.accent.copy(alpha = 0.45f), shape)
            .clickable { GroupListen.sheetOpen.value = true }
            .padding(horizontal = 10.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_group), contentDescription = null, tint = p.accent, modifier = Modifier.size(16.dp))
        Spacer(Modifier.width(6.dp))
        Text(GroupRules.pill(s), style = MaterialTheme.typography.labelMedium, color = p.text, maxLines = 1)
    }
}

/**
 * Above the mini player while in a group: the pill, whose group it is, and – after a pause here only – "Paused here"
 * with "Catch up".
 */
@Composable
fun GroupBar(nav: Nav) {
    val state by GroupListen.state.collectAsState()
    val paused by GroupListen.localPaused.collectAsState()
    val s = state ?: return
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(12.dp)
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 12.dp, vertical = 6.dp)
            .clip(shape)
            .background(p.bg2)
            .border(1.dp, p.border, shape)
            .clickable { GroupListen.sheetOpen.value = true }
            .padding(start = 8.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        GroupPill()
        Spacer(Modifier.width(10.dp))
        Text(
            if (paused) "Paused here" else if (s.youHost) "Your group listen" else "${s.hostName}'s group listen",
            style = MaterialTheme.typography.bodySmall,
            color = if (paused) p.text else p.muted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (paused) {
            Spacer(Modifier.width(8.dp))
            // the player follows the group again (GroupSync in the playback service)
            SmallButton("Catch up", primary = true) { GroupListen.catchUp() }
        }
    }
}

// ---- the group sheet ----------------------------------------------------------------------------------------

/** The group listen: the code, the invite link, the members, the host's settings, Leave / End for everyone. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
private fun GroupSheet(nav: Nav, s: GroupState, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val paused by GroupListen.localPaused.collectAsState()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val link = GroupRules.inviteLink(s, nav.graph.api.base)
    var confirmEnd by remember { mutableStateOf(false) }
    // the host and the server's admins may remove a guest (client-shapes §3.16)
    var admin by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { admin = try { withContext(Dispatchers.IO) { nav.graph.api.me().isAdmin } } catch (_: Exception) { false } }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        LazyColumn(Modifier.fillMaxWidth().fillMaxHeight(0.85f)) {
            item {
                Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp)) {
                    Text("Group listen", style = MaterialTheme.typography.titleMedium)
                    Text(
                        if (s.youHost) "You host this group listen. Everyone hears the same songs at the same time, each on their own device."
                        else "${s.hostName} hosts this group listen. Everyone hears the same songs at the same time, each on their own device.",
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                    Spacer(Modifier.height(14.dp))
                    // the code, big enough to read out
                    Column(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(p.bg2).border(1.dp, p.border, RoundedCornerShape(12.dp)).padding(14.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        Text("Code", style = MaterialTheme.typography.labelMedium, color = p.muted)
                        Text(GroupRules.spaced(s.code), fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold, fontSize = 30.sp, color = p.text)
                        Spacer(Modifier.height(10.dp))
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            SmallButton("Copy invite link", iconRes = R.drawable.ic_content_copy) { copy(context, link) }
                            SmallButton("Share", iconRes = R.drawable.ic_share) { share(context, s, link) }
                        }
                    }
                    if (paused) {
                        Spacer(Modifier.height(10.dp))
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Paused here", style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                            SmallButton("Catch up", primary = true) { GroupListen.catchUp() }
                        }
                    }
                    Spacer(Modifier.height(16.dp))
                    Text("People (${s.count})", style = MaterialTheme.typography.titleSmall)
                    Spacer(Modifier.height(4.dp))
                }
            }
            items(s.members, key = { it.key }) { m -> MemberRow(nav, s, m, admin) }
            if (s.youHost) {
                item { HostSettings(s) }
            } else {
                item {
                    Text(
                        if (s.everyoneControls) "Everyone can control playback." else GroupRules.onlyHost(s) + ". You can add songs with Add to queue.",
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp),
                    )
                }
            }
            item {
                FlowRow(
                    Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 14.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    if (s.youHost) EncoreButton("End for everyone") { confirmEnd = true }
                    EncoreButton("Leave") { GroupListen.leave(); onDismiss() }
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
    if (confirmEnd) {
        AlertDialog(
            onDismissRequest = { confirmEnd = false },
            containerColor = p.panel,
            titleContentColor = p.text,
            textContentColor = p.muted,
            title = { Text("End the group listen?") },
            text = { Text("The music stops for everyone in it.") },
            confirmButton = {
                TextButton(onClick = { confirmEnd = false; GroupListen.end(); onDismiss() }) { Text("End for everyone", color = EncoreColors.Err, fontWeight = FontWeight.SemiBold) }
            },
            dismissButton = { TextButton(onClick = { confirmEnd = false }) { Text("Cancel", color = p.muted) } },
        )
    }
}

@Composable
private fun MemberRow(nav: Nav, s: GroupState, m: GroupMember, admin: Boolean) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Avatar(m.name, avatarUrl(nav, m.avatar), nav.graph.imageLoader, size = 36.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(m.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (m.host) Badge("Host", kind = "accent")
                if (m.guest) Badge("Guest")
                if (m.key == s.youMember) Badge("You")
            }
            if (m.deviceName.isNotEmpty()) Text(m.deviceName, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        // only guests can be removed; accounts leave by themselves
        if ((s.youHost || admin) && m.guest) {
            MoreButton(listOf(MenuEntry("Remove from the group", R.drawable.ic_close, danger = true) { GroupListen.removeMember(m) }))
        }
    }
}

/** The host's settings: Everyone can control, who can see it, guests (when the server allows them), the cap. */
@Composable
private fun HostSettings(s: GroupState) {
    val p = MaterialTheme.palette
    var limit by remember(s.id) { mutableIntStateOf(s.maxMembers) }
    // the cap is sent once the taps stop
    LaunchedEffect(limit) {
        delay(700L)
        if (limit != s.maxMembers) GroupListen.patch { put("max_members", limit) }
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp)) {
        Text("Settings", style = MaterialTheme.typography.titleSmall)
        SwitchRow("Everyone can control", "Anyone in the group can play, pause, skip and change the queue.", s.everyoneControls) { on ->
            GroupListen.patch { put("everyone_controls", on) }
        }
        Spacer(Modifier.height(6.dp))
        Text("Who can join", style = MaterialTheme.typography.bodyMedium)
        RadioRow("Anyone on this server can see and join", !s.linkOnly) { GroupListen.patch { put("visibility", "anyone") } }
        RadioRow("Only people with the link", s.linkOnly) { GroupListen.patch { put("visibility", "link") } }
        if (s.guestsPossible) {
            SwitchRow("Guests can join with the link", "People without an account join on the web with the invite link. They listen; they can't add songs or control playback.", s.guestsAllowed) { on ->
                GroupListen.patch { put("guests_allowed", on) }
            }
        }
        Spacer(Modifier.height(6.dp))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Limit to $limit people", style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            SmallButton("−", enabled = limit > maxOf(2, s.count)) { limit = (limit - 1).coerceAtLeast(maxOf(2, s.count)) }
            Spacer(Modifier.width(6.dp))
            SmallButton("+", enabled = limit < s.serverMax) { limit = (limit + 1).coerceAtMost(s.serverMax) }
        }
        Text("At most ${s.serverMax} on this server.", style = MaterialTheme.typography.bodySmall, color = p.muted)
    }
}

@Composable
private fun SwitchRow(label: String, help: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().clickable { onChange(!checked) }.padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyMedium)
            if (help.isNotEmpty()) Text(help, style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        Spacer(Modifier.width(10.dp))
        Switch(
            checked = checked,
            onCheckedChange = onChange,
            colors = SwitchDefaults.colors(checkedTrackColor = p.accent, checkedThumbColor = p.onAccent, uncheckedTrackColor = p.panel2, uncheckedThumbColor = p.muted, uncheckedBorderColor = p.border),
        )
    }
}

@Composable
private fun RadioRow(label: String, selected: Boolean, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick), verticalAlignment = Alignment.CenterVertically) {
        RadioButton(selected = selected, onClick = onClick, colors = RadioButtonDefaults.colors(selectedColor = p.accent, unselectedColor = p.muted))
        Text(label, style = MaterialTheme.typography.bodyMedium)
    }
}

private fun copy(context: Context, link: String) {
    try {
        val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
        cm?.setPrimaryClip(ClipData.newPlainText("Encore", link))
        Toasts.show("Invite link copied", "ok")
    } catch (_: Exception) {
        Toasts.show("Couldn't copy the link", "err")
    }
}

private fun share(context: Context, s: GroupState, link: String) {
    val send = Intent(Intent.ACTION_SEND).apply {
        type = "text/plain"
        putExtra(Intent.EXTRA_SUBJECT, "Group listen on Encore")
        putExtra(Intent.EXTRA_TEXT, "Listen with me on Encore (code ${s.code}):\n$link")
    }
    try {
        context.startActivity(Intent.createChooser(send, "Share invite link").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: Exception) {
        copy(context, link)
    }
}

// ---- start and join ------------------------------------------------------------------------------------------

@Composable
private fun StartDialog(nav: Nav, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val busy by GroupListen.busy.collectAsState()
    var linkOnly by remember { mutableStateOf(false) }
    var everyone by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text("Start a group listen") },
        text = {
            Column {
                Text(
                    "Everyone who joins hears the same songs at the same time, each on their own device. What plays here now becomes the group's queue.",
                    style = MaterialTheme.typography.bodySmall,
                )
                Spacer(Modifier.height(10.dp))
                RadioRow("Anyone on this server can see and join", !linkOnly) { linkOnly = false }
                RadioRow("Only people with the link", linkOnly) { linkOnly = true }
                SwitchRow("Everyone can control", "", everyone) { everyone = it }
            }
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = {
                val player = nav.player
                GroupListen.start(
                    visibility = if (linkOnly) "link" else "anyone",
                    everyoneControls = everyone,
                    queue = player.queue.value,
                    index = player.queueIndex.value,
                    positionSec = player.positionMs() / 1000.0,
                    playing = player.isPlaying.value,
                    onDone = onDismiss,
                )
            }) { Text(if (busy) "Starting…" else "Start", color = p.text, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** "Join a group listen": a code, or an invite link pasted whole. */
@Composable
private fun JoinDialog(onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val busy by GroupListen.busy.collectAsState()
    var text by remember { mutableStateOf("") }
    val code = GroupRules.codeFrom(text)
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text("Join a group listen") },
        text = {
            Column {
                Text("Type the code the host gave you, or paste the invite link.", style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(8.dp))
                OutlinedTextField(value = text, onValueChange = { text = it.take(300) }, singleLine = true, placeholder = { Text("Code") })
            }
        },
        confirmButton = {
            TextButton(enabled = code != null && !busy, onClick = {
                if (GroupRules.serverOf(text) != null) { GroupListen.openInvite(text); onDismiss() }
                else GroupListen.joinCode(code ?: return@TextButton, onDone = onDismiss)
            }) { Text(if (busy) "Joining…" else "Join", color = if (code != null) p.text else p.faint, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** An invite link opened on this phone: "Join <host>'s group listen?". */
@Composable
private fun InviteDialog(inv: GroupInvite, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val busy by GroupListen.busy.collectAsState()
    val host = inv.summary?.hostName
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(if (host != null) "Join $host's group listen?" else "Join this group listen?") },
        text = {
            val sub = inv.summary
            Text(
                (if (sub != null && sub.title.isNotEmpty()) "Playing now: ${sub.title}" + (if (sub.artist.isNotEmpty()) " – ${sub.artist}" else "") + ". " else "") +
                    "What plays here stops, and you hear the group's songs with everyone else.",
                style = MaterialTheme.typography.bodySmall,
            )
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = { GroupListen.joinCode(inv.code, onDone = onDismiss) }) {
                Text(if (busy) "Joining…" else "Join", color = p.text, fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

// ---- the server's list: Home and the devices sheet -------------------------------------------------------------

/** Asks the server's list every 20 s while shown (a server without group listens only every 10 minutes: refreshList). */
@Composable
private fun RefreshList() {
    val context = LocalContext.current
    LaunchedEffect(Unit) {
        GroupListen.init(context)
        while (true) {
            GroupListen.refreshList()
            delay(20_000L)
        }
    }
}

/**
 * Home's "Listening together now": a tile per live group listen this account may see (the host's avatar on the
 * song's cover, "Alex · 3 listening", Join). Nothing when there is none, or on a server without group listens.
 */
@Composable
fun ListeningTogetherRow(nav: Nav) {
    RefreshList()
    val live by GroupListen.live.collectAsState()
    val available by GroupListen.available.collectAsState()
    val current by GroupListen.state.collectAsState()
    if (available != true || live.isEmpty()) return
    val p = MaterialTheme.palette
    Column {
        SectionHead("Listening together now")
        LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            items(live, key = { it.id }) { g ->
                val inIt = current?.id == g.id
                Column(Modifier.width(150.dp).clip(RoundedCornerShape(12.dp)).clickable { open(g) }) {
                    Box {
                        Cover(
                            if (g.albumId.isNotEmpty()) nav.graph.api.coverUrl(g.albumId, 400) else null,
                            nav.graph.imageLoader,
                            size = 150.dp,
                            corner = 12.dp,
                            label = g.title.ifEmpty { g.hostName },
                        )
                        Box(Modifier.align(Alignment.BottomStart).offset(8.dp, (-8).dp).clip(CircleShape).border(2.dp, p.bg, CircleShape)) {
                            Avatar(g.hostName, avatarUrl(nav, g.hostAvatar), nav.graph.imageLoader, size = 36.dp)
                        }
                    }
                    Spacer(Modifier.height(9.dp))
                    Text(GroupRules.tileLine(g), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOf(g.title, g.artist).filter { it.isNotEmpty() }.joinToString(" – ").ifEmpty { if (g.playing) "Playing" else "Paused" },
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Spacer(Modifier.height(6.dp))
                    SmallButton(if (inIt) "Open" else "Join", primary = !inIt) { open(g) }
                }
            }
        }
    }
}

private fun open(g: GroupSummary) {
    if (GroupListen.state.value?.id == g.id) GroupListen.sheetOpen.value = true else GroupListen.joinId(g.id)
}

/**
 * The devices sheet's "Group listens": the group this phone is in, the others on the server, "Start a group listen"
 * and "Join with a code". Nothing on a server without group listens.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun GroupDevicesSection(nav: Nav, onDone: () -> Unit) {
    RefreshList()
    val live by GroupListen.live.collectAsState()
    val available by GroupListen.available.collectAsState()
    val current by GroupListen.state.collectAsState()
    if (available != true && current == null) return
    val p = MaterialTheme.palette
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp)) {
        Text("Group listens", style = MaterialTheme.typography.titleSmall)
        Spacer(Modifier.height(6.dp))
        val s = current
        if (s != null) {
            GroupLine(nav, s.hostName, s.hostAvatar, s.track?.title ?: "", s.count, if (s.youHost) "You host it" else "You're in it", "Open") {
                GroupListen.sheetOpen.value = true
                onDone()
            }
        }
        for (g in live) {
            if (g.id == s?.id) continue
            GroupLine(nav, g.hostName, g.hostAvatar, g.title, g.membersCount, if (g.mine) "You're in it on another device" else "", "Join") {
                GroupListen.joinId(g.id)
            }
        }
        if (s == null && live.isEmpty()) {
            Text("No group listens on this server right now.", style = MaterialTheme.typography.bodySmall, color = p.muted)
        }
        Spacer(Modifier.height(8.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (s == null) SmallButton("Start a group listen", iconRes = R.drawable.ic_group, primary = true) { GroupListen.startOpen.value = true }
            SmallButton("Join with a code") { GroupListen.joinOpen.value = true }
        }
    }
}

@Composable
private fun GroupLine(nav: Nav, host: String, avatar: String, title: String, count: Int, note: String, action: String, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
        Avatar(host, avatarUrl(nav, avatar), nav.graph.imageLoader, size = 36.dp)
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f)) {
            Text("$host · $count listening", fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val line = listOf(title, note).filter { it.isNotEmpty() }.joinToString(" · ")
            if (line.isNotEmpty()) Text(line, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        SmallButton(action, primary = action == "Join", onClick = onClick)
    }
}

/** The devices sheet while in a group: linking is off ("Leave the group listen to control another device"). */
@Composable
fun GroupLinkNote(modifier: Modifier = Modifier) {
    val state by GroupListen.state.collectAsState()
    if (state == null) return
    Text(
        "Leave the group listen to control another device",
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.palette.muted,
        modifier = modifier,
    )
}

// ---- Up next ----------------------------------------------------------------------------------------------------

/** Up next in a group: the group's queue with who added each song; the controls follow what this device may do. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GroupUpNextSheet(nav: Nav, s: GroupState, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val (index, _) = GroupRules.expected(s, GroupListen.serverNow())
    val upNext = if (index + 1 <= s.queue.lastIndex) s.queue.subList(index + 1, s.queue.size) else emptyList()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Row(Modifier.fillMaxWidth().padding(start = 22.dp, end = 12.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Up next", style = MaterialTheme.typography.titleMedium)
                Text(
                    if (s.youCanControl) "Everyone in the group can add songs with Add to queue." else GroupRules.onlyHost(s) + ". You can add songs with Add to queue.",
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                )
            }
            GroupPill()
        }
        LazyColumn(Modifier.fillMaxWidth().fillMaxHeight(0.7f)) {
            itemsIndexed(upNext, key = { _, e -> e.qid }) { k, e ->
                val i = index + 1 + k
                val q = e.track
                Row(
                    Modifier.fillMaxWidth().clickable { GroupListen.jump(e, i) }.padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Cover(
                        if (q.albumId.isNotEmpty()) nav.graph.api.coverUrl(q.albumId, 96) else null,
                        nav.graph.imageLoader,
                        size = 42.dp,
                        corner = 8.dp,
                        label = q.album.ifEmpty { q.title },
                    )
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Text(q.shownTitle(), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            listOf(q.shownArtist(), formatDuration((q.durationSec * 1000).toLong())).filter { it.isNotEmpty() }.joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = p.muted,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (e.addedBy.isNotEmpty()) {
                            Text("added by ${e.addedBy}", style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    if (s.youCanControl) {
                        if (k > 0) {
                            IconButton(onClick = { GroupListen.move(e, i, i - 1) }) {
                                Icon(painterResource(R.drawable.ic_arrow_upward), contentDescription = "Move up", tint = p.muted)
                            }
                        }
                        IconButton(onClick = { GroupListen.remove(e, i) }) {
                            Icon(painterResource(R.drawable.ic_close), contentDescription = "Remove", tint = p.muted)
                        }
                    }
                }
            }
            if (upNext.isEmpty()) {
                item {
                    Text(
                        "Nothing queued. Anyone in the group can add songs with Add to queue.",
                        style = MaterialTheme.typography.bodySmall,
                        color = p.muted,
                        modifier = Modifier.padding(horizontal = 22.dp, vertical = 16.dp),
                    )
                }
            }
            item { Spacer(Modifier.height(24.dp)) }
        }
    }
}
